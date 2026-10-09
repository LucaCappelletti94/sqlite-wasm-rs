//! Workers sharing one SQLCipher build, each keyed connection on its own worker.

#[path = "support/scenarios.rs"]
mod scenarios;
#[path = "support/workers.rs"]
mod workers;

use scenarios::{init, Connection, StartLine};
use sqlite_wasm_rs::vfs::memvfs::MemVfsUtil;
use sqlite_wasm_rs::vfs::transfer::DbTransfer;
use sqlite_wasm_rs::SQLITE_NOTADB;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

const TIMEOUT_MS: u32 = 120_000;
// A low work factor keeps each keyed open cheap, and every connection to one file must agree on it.
const KEY: &str = "PRAGMA key = 'first passphrase'; PRAGMA kdf_iter = 4000;";
const NEW_KEY: &str = "PRAGMA key = 'second passphrase'; PRAGMA kdf_iter = 4000;";

fn open_keyed(name: &str, key: &str) -> Connection {
    let connection = Connection::open(name);
    connection.exec(key);
    connection
}

/// Count and sum of the pairs table, or the result code of a refused read.
fn pair_totals(connection: &Connection) -> Result<[i64; 2], i32> {
    connection
        .try_query_row("SELECT count(*), coalesce(sum(v), 0) FROM t")
        .map(|row| row.expect("an aggregate returns one row"))
}

fn assert_cipher_integrity(connection: &Connection) {
    // The pragma returns one row per problem.
    assert_eq!(
        connection.try_query_row::<1>("PRAGMA cipher_integrity_check"),
        Ok(None)
    );
}

/// Creates `name` keyed with `KEY`, holding `pairs` pairs that sum to zero.
fn keyed_pairs(name: &str, pairs: i64) -> Connection {
    let connection = open_keyed(name, KEY);
    scenarios::create_pairs(&connection);
    connection.exec(&format!(
        "WITH RECURSIVE n(k) AS (SELECT 1 UNION ALL SELECT k + 1 FROM n WHERE k < {pairs})
         INSERT INTO t SELECT k, k FROM n UNION ALL SELECT -k, -k FROM n;"
    ));
    connection
}

#[wasm_bindgen_test]
async fn every_worker_draws_its_own_entropy() {
    const WORKERS: usize = 8;

    init();
    let start = Arc::new(StartLine::new(WORKERS));
    let tasks = (0..WORKERS)
        .map(|worker| {
            let start = start.clone();
            workers::task(move || {
                start.wait();
                // A new keyed database takes its salt from Fortuna, seeded by this worker's getentropy.
                let connection = keyed_pairs(&format!("entropy-{worker}.db"), 10);
                assert_eq!(pair_totals(&connection), Ok([20, 0]));
            })
        })
        .collect();
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();

    // SAFETY: SQLite's initialization installed memvfs, and no connection is open.
    let util = unsafe { MemVfsUtil::get() }.unwrap();
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
                    assert_eq!(pair_totals(&open_keyed(DB, KEY)), Ok([1000, 0]));
                }
            })
        })
        .collect();
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();
    assert_eq!(pair_totals(&setup), Ok([1000, 0]));
    assert_cipher_integrity(&setup);
}

#[wasm_bindgen_test]
async fn keyed_increments_lose_no_update() {
    const WORKERS: usize = 8;
    const INCREMENTS: usize = 50;
    const DB: &str = "sqlcipher-increments.db";

    init();
    let setup = open_keyed(DB, KEY);
    scenarios::create_counter(&setup);
    let start = Arc::new(StartLine::new(WORKERS));
    let tasks = (0..WORKERS)
        .map(|_| {
            let start = start.clone();
            workers::task(move || scenarios::increment(&|| open_keyed(DB, KEY), INCREMENTS, &start))
        })
        .collect();
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();
    scenarios::assert_counter(&setup, WORKERS * INCREMENTS);
    assert_cipher_integrity(&setup);
}

#[wasm_bindgen_test]
async fn keyed_writers_with_readers() {
    const WRITERS: usize = 2;
    const READERS: usize = 4;
    const PAIRS: usize = 100;
    const DB: &str = "sqlcipher-writers.db";

    init();
    let setup = open_keyed(DB, KEY);
    scenarios::create_pairs(&setup);
    let start = Arc::new(StartLine::new(WRITERS + READERS));
    let writers_done = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for writer in 0..WRITERS {
        let (start, writers_done) = (start.clone(), writers_done.clone());
        tasks.push(workers::task(move || {
            scenarios::write_pairs(&|| open_keyed(DB, KEY), writer, PAIRS, &start);
            writers_done.fetch_add(1, Ordering::Release);
        }));
    }
    for _ in 0..READERS {
        let (start, writers_done) = (start.clone(), writers_done.clone());
        tasks.push(workers::task(move || {
            let done = || writers_done.load(Ordering::Acquire) == WRITERS;
            scenarios::read_pairs(&|| open_keyed(DB, KEY), &done, &start);
        }));
    }
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();
    scenarios::assert_pairs(&setup, WRITERS, PAIRS);
    assert_cipher_integrity(&setup);
}

#[wasm_bindgen_test]
async fn rekey_while_others_read() {
    const READERS: usize = 6;
    const DB: &str = "sqlcipher-rekey.db";

    init();
    drop(keyed_pairs(DB, 2_000));
    let start = Arc::new(StartLine::new(READERS + 1));
    let rekeyed = Arc::new(AtomicBool::new(false));
    let mut tasks = Vec::new();
    for _ in 0..READERS {
        let (start, rekeyed) = (start.clone(), rekeyed.clone());
        tasks.push(workers::task(move || {
            start.wait();
            let mut reads_after_rekey = 0;
            while reads_after_rekey < 5 {
                let done = rekeyed.load(Ordering::Acquire);
                match pair_totals(&open_keyed(DB, if done { NEW_KEY } else { KEY })) {
                    // A read is either the full committed table or a clean refusal, never other data.
                    Ok(totals) => {
                        assert_eq!(totals, [4_000, 0], "read other data during rekey");
                        reads_after_rekey += usize::from(done);
                    }
                    Err(SQLITE_NOTADB) => assert!(!done, "the new key was refused after the rekey"),
                    Err(rc) => panic!("read failed with {rc} during rekey"),
                }
            }
        }));
    }
    {
        let (start, rekeyed) = (start.clone(), rekeyed.clone());
        tasks.push(workers::task(move || {
            let connection = open_keyed(DB, KEY);
            start.wait();
            connection.exec("PRAGMA rekey = 'second passphrase';");
            rekeyed.store(true, Ordering::Release);
        }));
    }
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();

    let fresh = open_keyed(DB, NEW_KEY);
    assert_eq!(pair_totals(&fresh), Ok([4_000, 0]));
    assert_cipher_integrity(&fresh);
    assert_eq!(pair_totals(&open_keyed(DB, KEY)), Err(SQLITE_NOTADB));
}
