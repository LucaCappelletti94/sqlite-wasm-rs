//! Workers sharing one SQLCipher build, each keyed connection on its own worker.
//!
//! Build with `SQLITE_WASM_RS_SOURCE_DIR` pointing at the SQLCipher amalgamation, and run with
//! `--test threadsafe_sqlcipher`. Setting `THREADSAFE_SQLCIPHER_NO_MUTEX` at compile time puts
//! SQLite's no-op mutexes back through `SQLITE_CONFIG_SINGLETHREAD`, where these tests must fail.

mod ffi {
    pub use sqlite_wasm_rs::*;
}
#[path = "../crates/rsqlite-vfs/tests/support/scenarios.rs"]
mod scenarios;
#[path = "support/workers.rs"]
mod workers;

use scenarios::StartLine;
use sqlite_wasm_rs::*;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

const TIMEOUT_MS: u32 = 120_000;
// A low work factor keeps each keyed open cheap, and every connection to one file must agree on it.
const KEY: &str = "PRAGMA key = 'first passphrase'; PRAGMA kdf_iter = 4000;";
const NEW_KEY: &str = "PRAGMA key = 'second passphrase'; PRAGMA kdf_iter = 4000;";

fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if option_env!("THREADSAFE_SQLCIPHER_NO_MUTEX").is_some() {
            // SAFETY: runs before any other SQLite call in this module.
            assert_eq!(
                unsafe { sqlite3_config(SQLITE_CONFIG_SINGLETHREAD) },
                SQLITE_OK
            );
        }
        // SAFETY: initialization has no precondition beyond the configuration above.
        assert_eq!(unsafe { sqlite3_initialize() }, SQLITE_OK);
    });
}

/// A raw keyed connection whose failures are values, so readers can tell errors from wrong data.
struct Raw(*mut sqlite3);

impl Raw {
    fn open(name: &str, key: &str) -> Self {
        let name = CString::new(name).unwrap();
        let mut db = std::ptr::null_mut();
        let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX;
        // SAFETY: the strings are NUL-terminated and `db` is a valid out pointer.
        let rc = unsafe { sqlite3_open_v2(name.as_ptr(), &mut db, flags, c"memvfs".as_ptr()) };
        assert_eq!(rc, SQLITE_OK);
        // SAFETY: `db` is open.
        unsafe { sqlite3_busy_timeout(db, 60_000) };
        let raw = Self(db);
        assert_eq!(raw.exec(key), SQLITE_OK);
        raw
    }

    fn exec(&self, sql: &str) -> i32 {
        let sql = CString::new(sql).unwrap();
        // SAFETY: the connection is open and `sql` is NUL-terminated.
        unsafe {
            sqlite3_exec(
                self.0,
                sql.as_ptr(),
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        }
    }

    /// Every row of `sql` as integers, or the first failing result code.
    fn rows(&self, sql: &CStr, columns: i32) -> Result<Vec<Vec<i64>>, i32> {
        let mut stmt = std::ptr::null_mut();
        // SAFETY: the connection is open, `sql` is NUL-terminated and `stmt` is a valid out pointer.
        let rc = unsafe {
            sqlite3_prepare_v2(self.0, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut())
        };
        if rc != SQLITE_OK {
            return Err(rc);
        }
        let mut rows = Vec::new();
        // SAFETY: `stmt` is prepared, read only while it holds a row, and finalized once.
        let rc = unsafe {
            let rc = loop {
                match sqlite3_step(stmt) {
                    SQLITE_ROW => rows.push(
                        (0..columns)
                            .map(|c| sqlite3_column_int64(stmt, c))
                            .collect(),
                    ),
                    rc => break rc,
                }
            };
            sqlite3_finalize(stmt);
            rc
        };
        if rc == SQLITE_DONE {
            Ok(rows)
        } else {
            Err(rc)
        }
    }

    fn assert_cipher_integrity(&self) {
        let failures = self.rows(c"PRAGMA cipher_integrity_check", 1).unwrap();
        assert!(
            failures.is_empty(),
            "cipher_integrity_check reported {} problems",
            failures.len()
        );
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        // SAFETY: every statement was finalized. A failed close only leaks on a path that already failed.
        unsafe { sqlite3_close(self.0) };
    }
}

/// Count and sum of the pairs table, whose sum is zero in every committed state.
fn pair_totals(connection: &Raw) -> Result<(i64, i64), i32> {
    let rows = connection.rows(c"SELECT count(*), coalesce(sum(v), 0) FROM t", 2)?;
    Ok((rows[0][0], rows[0][1]))
}

fn keyed_pairs(name: &str, pairs: i64) -> Raw {
    let connection = Raw::open(name, KEY);
    let sql = format!(
        "CREATE TABLE t(k INTEGER PRIMARY KEY, v INTEGER NOT NULL);
         WITH RECURSIVE n(k) AS (SELECT 1 UNION ALL SELECT k + 1 FROM n WHERE k < {pairs})
         INSERT INTO t SELECT k, k FROM n UNION ALL SELECT -k, -k FROM n;"
    );
    assert_eq!(connection.exec(&sql), SQLITE_OK);
    connection
}

#[wasm_bindgen_test]
fn sqlcipher_uses_libtomcrypt() {
    init();
    let connection = Raw::open(":memory:", KEY);
    // SAFETY: the returned text is copied before the statement is finalized.
    let provider = unsafe {
        let mut stmt = std::ptr::null_mut();
        let sql = c"PRAGMA cipher_provider";
        assert_eq!(
            sqlite3_prepare_v2(
                connection.0,
                sql.as_ptr(),
                -1,
                &mut stmt,
                std::ptr::null_mut()
            ),
            SQLITE_OK
        );
        assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
        let text = CStr::from_ptr(sqlite3_column_text(stmt, 0).cast())
            .to_string_lossy()
            .into_owned();
        sqlite3_finalize(stmt);
        text
    };
    assert_eq!(provider, "libtomcrypt");
}

#[wasm_bindgen_test]
async fn every_worker_draws_its_own_entropy() {
    const WORKERS: usize = 8;
    use sqlite_wasm_rs::vfs::transfer::DbTransfer;
    init();
    let start = Arc::new(StartLine::new(WORKERS));
    let tasks = (0..WORKERS)
        .map(|worker| {
            let start = start.clone();
            workers::task(move || {
                start.wait();
                // A new keyed database takes its salt from Fortuna, seeded by this worker's getentropy.
                let connection = keyed_pairs(&format!("entropy-{worker}.db"), 10);
                assert_eq!(pair_totals(&connection), Ok((20, 0)));
            })
        })
        .collect();
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();

    // SAFETY: memvfs was installed by SQLite's initialization and no connection is open.
    let util = unsafe { sqlite_wasm_rs::vfs::memvfs::MemVfsUtil::get() }.unwrap();
    let mut salts: Vec<Vec<u8>> = (0..WORKERS)
        .map(|worker| util.export_db(&format!("entropy-{worker}.db")).unwrap()[..16].to_vec())
        .collect();
    assert!(
        salts.iter().all(|salt| salt.iter().any(|&byte| byte != 0)),
        "an all-zero salt"
    );
    salts.sort();
    salts.dedup();
    assert_eq!(salts.len(), WORKERS, "two workers produced the same salt");
}

#[wasm_bindgen_test]
async fn concurrent_keyed_reads() {
    const WORKERS: usize = 8;
    const OPENS: usize = 25;
    const DB: &str = "sqlcipher-reads.db";
    init();
    let setup = keyed_pairs(DB, 500);
    let start = Arc::new(StartLine::new(WORKERS));
    let tasks = (0..WORKERS)
        .map(|_| {
            let start = start.clone();
            workers::task(move || {
                start.wait();
                // Opening and closing each time also churns SQLCipher's provider activation count.
                for _ in 0..OPENS {
                    let connection = Raw::open(DB, KEY);
                    assert_eq!(pair_totals(&connection), Ok((1000, 0)));
                }
            })
        })
        .collect();
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();
    assert_eq!(pair_totals(&setup), Ok((1000, 0)));
    setup.assert_cipher_integrity();
}

#[wasm_bindgen_test]
async fn keyed_increments_lose_no_update() {
    const WORKERS: usize = 8;
    const INCREMENTS: usize = 50;
    const DB: &str = "sqlcipher-increments.db";
    init();
    let setup = scenarios::create_counter(DB, &|c| c.exec(KEY));
    let start = Arc::new(StartLine::new(WORKERS));
    let tasks = (0..WORKERS)
        .map(|_| {
            let start = start.clone();
            workers::task(move || {
                scenarios::increment(DB, INCREMENTS, &|c| c.exec(KEY), &|| start.wait())
            })
        })
        .collect();
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();
    scenarios::assert_counter(&setup, WORKERS * INCREMENTS);
    setup.close();
    Raw::open(DB, KEY).assert_cipher_integrity();
}

#[wasm_bindgen_test]
async fn keyed_writers_with_readers() {
    const WRITERS: usize = 2;
    const READERS: usize = 4;
    const PAIRS: usize = 100;
    const DB: &str = "sqlcipher-writers.db";
    init();
    let setup = scenarios::create_pairs(DB, &|c| c.exec(KEY));
    let start = Arc::new(StartLine::new(WRITERS + READERS));
    let writers_done = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for writer in 0..WRITERS {
        let (start, writers_done) = (start.clone(), writers_done.clone());
        tasks.push(workers::task(move || {
            scenarios::write_pairs(DB, writer, PAIRS, &|c| c.exec(KEY), &|| start.wait());
            writers_done.fetch_add(1, Ordering::Release);
        }));
    }
    for _ in 0..READERS {
        let (start, writers_done) = (start.clone(), writers_done.clone());
        tasks.push(workers::task(move || {
            let done = || writers_done.load(Ordering::Acquire) == WRITERS;
            scenarios::read_pairs(DB, &done, &|c| c.exec(KEY), &|| start.wait());
        }));
    }
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();
    scenarios::assert_pairs(&setup, WRITERS, PAIRS);
    setup.close();
    Raw::open(DB, KEY).assert_cipher_integrity();
}

#[wasm_bindgen_test]
async fn rekey_while_others_read() {
    const READERS: usize = 6;
    const DB: &str = "sqlcipher-rekey.db";
    init();
    drop(keyed_pairs(DB, 2_000));
    let start = Arc::new(StartLine::new(READERS + 1));
    let rekeyed = Arc::new(AtomicBool::new(false));
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = Vec::new();
    for _ in 0..READERS {
        let (start, rekeyed, outcomes) = (start.clone(), rekeyed.clone(), outcomes.clone());
        tasks.push(workers::task(move || {
            start.wait();
            let (mut before, mut after, mut refused) = (0, 0, 0);
            while after < 5 {
                let done = rekeyed.load(Ordering::Acquire);
                let connection = Raw::open(DB, if done { NEW_KEY } else { KEY });
                match pair_totals(&connection) {
                    // A read is either the full committed table or a clean refusal, never other data.
                    Ok(totals) => {
                        assert_eq!(totals, (4_000, 0), "read other data during rekey");
                        if done {
                            after += 1
                        } else {
                            before += 1
                        }
                    }
                    Err(SQLITE_NOTADB) => refused += 1,
                    Err(rc) => panic!("read failed with {rc} during rekey"),
                }
            }
            outcomes.lock().unwrap().push((before, after, refused));
        }));
    }
    {
        let (start, rekeyed) = (start.clone(), rekeyed.clone());
        tasks.push(workers::task(move || {
            let connection = Raw::open(DB, KEY);
            start.wait();
            assert_eq!(
                connection.exec("PRAGMA rekey = 'second passphrase';"),
                SQLITE_OK
            );
            rekeyed.store(true, Ordering::Release);
        }));
    }
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();

    let outcomes = outcomes.lock().unwrap();
    assert!(outcomes.iter().all(|&(_, after, _)| after >= 5));
    let fresh = Raw::open(DB, NEW_KEY);
    assert_eq!(pair_totals(&fresh), Ok((4_000, 0)));
    fresh.assert_cipher_integrity();
    assert_eq!(pair_totals(&Raw::open(DB, KEY)), Err(SQLITE_NOTADB));
}
