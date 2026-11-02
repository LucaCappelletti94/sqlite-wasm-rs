use js_sys::{futures::JsFuture, Array, Promise};
use rsqlite_vfs::VfsFilesManager;
use sqlite_wasm_rs::*;
use sqlite_wasm_vfs::jspi_idb::{install, run, JspiIdbCfgBuilder, JspiIdbError, JspiIdbUtil};
use std::{
    ffi::{CStr, CString},
    ptr,
};
use wasm_bindgen::{prelude::wasm_bindgen, JsValue};
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(module = "/tests/jspi_idb.js")]
extern "C" {
    #[wasm_bindgen(js_name = deleteDatabase)]
    fn delete_database(name: &str) -> Promise;
    #[wasm_bindgen(js_name = storedFiles)]
    fn stored_files(database: &str) -> Promise;
    #[wasm_bindgen(js_name = abortAtPut)]
    fn abort_at_put(after: u32) -> js_sys::Function;
    #[wasm_bindgen(js_name = failPuts)]
    fn fail_puts() -> js_sys::Function;
    #[wasm_bindgen(js_name = recordWrites)]
    fn record_writes() -> js_sys::Function;
    #[wasm_bindgen(js_name = strandJournal)]
    fn strand_journal(name: &str) -> js_sys::Function;
}

/// Restores a patched browser method when dropped.
struct Restore(js_sys::Function);

impl Drop for Restore {
    fn drop(&mut self) {
        self.0.call0(&JsValue::UNDEFINED).unwrap();
    }
}

fn database(vfs: &str) -> String {
    format!("sqlite-wasm-vfs-tests/{vfs}")
}

/// Installs VFS `name` over a fresh IndexedDB database of its own.
async fn vfs(name: &str) -> JspiIdbUtil {
    vfs_with_cache(name, 256).await
}

async fn vfs_with_cache(name: &str, cache_blocks: usize) -> JspiIdbUtil {
    JsFuture::from(delete_database(&database(name)))
        .await
        .unwrap();
    let cfg = JspiIdbCfgBuilder::new()
        .vfs_name(name)
        .database(&database(name))
        .cache_blocks(cache_blocks)
        .build();
    install::<WasmOsCallback>(&cfg, false).await.unwrap()
}

async fn files(vfs: &str) -> Vec<String> {
    let keys = JsFuture::from(stored_files(&database(vfs))).await.unwrap();
    Array::from(&keys)
        .iter()
        .filter_map(|key| key.as_string())
        .collect()
}

struct Db(*mut sqlite3);

impl Db {
    fn open(vfs: &str, name: &str) -> Result<Self, i32> {
        let name = CString::new(name).unwrap();
        let vfs = CString::new(vfs).unwrap();
        let mut db = ptr::null_mut();
        // SAFETY: both strings outlive the call and `db` is a valid out pointer.
        let code = unsafe {
            sqlite3_open_v2(
                name.as_ptr(),
                &mut db,
                SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
                vfs.as_ptr(),
            )
        };
        if code == SQLITE_OK {
            return Ok(Self(db));
        }
        // SAFETY: SQLite returns a handle even on failure, closed exactly once here.
        unsafe { sqlite3_close(db) };
        Err(code)
    }

    fn exec(&self, sql: &str) -> i32 {
        let sql = CString::new(sql).unwrap();
        // SAFETY: the connection is open and `sql` outlives the call.
        unsafe { sqlite3_exec(self.0, sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) }
    }

    fn ok(&self, sql: &str) {
        assert_eq!(self.exec(sql), SQLITE_OK, "{sql}: {}", self.error());
    }

    fn error(&self) -> String {
        // SAFETY: SQLite returns a NUL-terminated message valid until the next call.
        unsafe { CStr::from_ptr(sqlite3_errmsg(self.0)) }
            .to_string_lossy()
            .into_owned()
    }

    fn text(&self, sql: &str) -> String {
        let sql = CString::new(sql).unwrap();
        let mut statement = ptr::null_mut();
        // SAFETY: the connection is open, `sql` outlives preparation, and the
        // column text is copied before the statement is finalized.
        unsafe {
            let code =
                sqlite3_prepare_v2(self.0, sql.as_ptr(), -1, &mut statement, ptr::null_mut());
            assert_eq!(code, SQLITE_OK, "{}", self.error());
            assert_eq!(sqlite3_step(statement), SQLITE_ROW, "{}", self.error());
            let text = CStr::from_ptr(sqlite3_column_text(statement, 0).cast())
                .to_string_lossy()
                .into_owned();
            assert_eq!(sqlite3_finalize(statement), SQLITE_OK);
            text
        }
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        // SAFETY: the connection is open and has no unfinalized statements.
        assert_eq!(unsafe { sqlite3_close(self.0) }, SQLITE_OK);
    }
}

/// Inserts `expr` for each `value` from 1 to `count` into `t`.
fn insert(expr: &str, count: u32) -> String {
    format!(
        "WITH RECURSIVE s(value) AS (SELECT 1 UNION ALL SELECT value + 1 FROM s WHERE value < {count}) \
         INSERT INTO t SELECT {expr} FROM s"
    )
}

async fn in_run<R: 'static>(f: impl FnOnce() -> R + 'static) -> R {
    run(f).await.unwrap()
}

/// Runs `statements` on `app.db` of `vfs`.
async fn write(vfs: &str, statements: &[&str]) {
    let vfs = vfs.to_owned();
    let statements = statements
        .iter()
        .map(|sql| sql.to_string())
        .collect::<Vec<_>>();
    in_run(move || {
        let db = Db::open(&vfs, "app.db").unwrap();
        statements.iter().for_each(|sql| db.ok(sql));
    })
    .await;
}

/// Reopens `app.db` of `vfs`, runs `setup`, and checks `query` and integrity.
async fn reopened(vfs: &str, setup: &str, query: &str, expected: &str) {
    let (vfs, setup, query, expected) = (
        vfs.to_owned(),
        setup.to_owned(),
        query.to_owned(),
        expected.to_owned(),
    );
    in_run(move || {
        let db = Db::open(&vfs, "app.db").unwrap();
        if !setup.is_empty() {
            db.ok(&setup);
        }
        assert_eq!(db.text(&query), expected, "{vfs}: {query}");
        assert_eq!(db.text("PRAGMA integrity_check"), "ok");
    })
    .await;
}

#[wasm_bindgen_test]
async fn commits_persist_across_connections() {
    vfs("persist").await;
    write(
        "persist",
        &[
            "CREATE TABLE t(x TEXT)",
            &insert("printf('row %d', value)", 2000),
        ],
    )
    .await;
    assert_eq!(files("persist").await, ["app.db"]);
    reopened(
        "persist",
        "",
        "SELECT count(*) || x FROM t WHERE rowid = 1234",
        "1row 1234",
    )
    .await;
    reopened("persist", "", "SELECT count(*) FROM t", "2000").await;
}

#[wasm_bindgen_test]
async fn sqlite_outside_run_is_refused() {
    vfs("outside").await;
    assert_eq!(Db::open("outside", "app.db").err(), Some(SQLITE_MISUSE));
    write("outside", &["CREATE TABLE t(x)"]).await;
}

#[wasm_bindgen_test]
async fn commit_is_one_strict_transaction() {
    vfs("batch").await;
    write("batch", &["CREATE TABLE t(x)"]).await;
    let recorder = record_writes();
    write(
        "batch",
        &["BEGIN; INSERT INTO t VALUES (1); INSERT INTO t VALUES (2); COMMIT;"],
    )
    .await;
    // No journal file: the commit is one batch-atomic flush.
    let writes = recorder.call0(&JsValue::UNDEFINED).unwrap();
    assert_eq!(Array::from(&writes).to_vec(), [JsValue::from_str("strict")]);
}

#[wasm_bindgen_test]
async fn aborted_flush_keeps_last_commit() {
    vfs("abort").await;
    in_run(|| {
        let db = Db::open("abort", "app.db").unwrap();
        db.ok("CREATE TABLE t(x)");
        db.ok(&insert("value", 500));
        let failing = Restore(fail_puts());
        assert_ne!(db.exec("UPDATE t SET x = x + 1000"), SQLITE_OK);
        assert_ne!(db.exec(&insert("value", 3000)), SQLITE_OK);
        drop(failing);
        assert_eq!(
            db.text("SELECT count(*) || ' ' || sum(x) FROM t"),
            "500 125250"
        );
    })
    .await;
    reopened(
        "abort",
        "",
        "SELECT count(*) || ' ' || sum(x) FROM t",
        "500 125250",
    )
    .await;
}

#[wasm_bindgen_test]
async fn commit_survives_one_aborted_batch() {
    vfs("retry").await;
    write("retry", &["CREATE TABLE t(x)"]).await;
    // The batch transaction aborts midway, so SQLite retries through a journal.
    let abort = Restore(abort_at_put(2));
    write("retry", &[&insert("value", 3000)]).await;
    drop(abort);
    reopened("retry", "", "SELECT count(*) FROM t", "3000").await;
}

#[wasm_bindgen_test]
async fn journal_modes_round_trip() {
    for (index, mode) in ["DELETE", "TRUNCATE", "PERSIST", "MEMORY"]
        .into_iter()
        .enumerate()
    {
        let name = format!("journal-{index}");
        vfs_with_cache(&name, 0).await;
        let journal = format!("PRAGMA journal_mode={mode}");
        // A cache smaller than the transaction forces spills and a journal.
        let statements = [
            journal.as_str(),
            "PRAGMA cache_size=8",
            "CREATE TABLE t(x)",
            &insert("randomblob(300)", 1000),
            "BEGIN; DELETE FROM t WHERE rowid % 2 = 0; ROLLBACK;",
            "DELETE FROM t WHERE rowid > 500",
            "VACUUM",
        ];
        write(&name, &statements).await;
        reopened(&name, "", "SELECT count(*) FROM t", "500").await;
    }
}

#[wasm_bindgen_test]
async fn page_sizes_round_trip() {
    for (index, page_size) in [1024, 4096, 65536].into_iter().enumerate() {
        let name = format!("page-{index}");
        vfs_with_cache(&name, 4).await;
        let pragma = format!("PRAGMA page_size={page_size}");
        let statements = [
            pragma.as_str(),
            "CREATE TABLE t(x)",
            &insert("randomblob(700)", 500),
            "UPDATE t SET x = zeroblob(10) WHERE rowid % 3 = 0",
        ];
        write(&name, &statements).await;
        let query = "SELECT page_size || ' ' || (SELECT count(*) FROM t WHERE length(x) = 10) FROM pragma_page_size";
        reopened(&name, "", query, &format!("{page_size} 166")).await;
    }
}

#[wasm_bindgen_test]
async fn hot_journal_is_rolled_back() {
    vfs_with_cache("hot", 0).await;
    write(
        "hot",
        &[
            "PRAGMA cache_size=8",
            "CREATE TABLE t(x)",
            &insert("randomblob(300)", 300),
        ],
    )
    .await;
    in_run(|| {
        let db = Db::open("hot", "app.db").unwrap();
        db.ok("PRAGMA cache_size=8");
        // The transaction outgrows the cache, so it journals, then cannot finish.
        let _strand = Restore(strand_journal("app.db"));
        assert_ne!(db.exec("UPDATE t SET x = randomblob(301)"), SQLITE_OK);
    })
    .await;
    assert_eq!(files("hot").await, ["app.db", "app.db-journal"]);
    reopened(
        "hot",
        "",
        "SELECT count(*) FROM t WHERE length(x) = 300",
        "300",
    )
    .await;
    assert_eq!(files("hot").await, ["app.db"]);
}

#[wasm_bindgen_test]
async fn wal_needs_exclusive_locking() {
    vfs("wal").await;
    in_run(|| {
        let db = Db::open("wal", "app.db").unwrap();
        assert_eq!(db.text("PRAGMA journal_mode=WAL"), "delete");
        db.ok("PRAGMA locking_mode=EXCLUSIVE");
        assert_eq!(db.text("PRAGMA journal_mode=WAL"), "wal");
        db.ok("CREATE TABLE t(x)");
        db.ok(&insert("randomblob(500)", 400));
    })
    .await;
    reopened(
        "wal",
        "PRAGMA locking_mode=EXCLUSIVE",
        "SELECT count(*) FROM t",
        "400",
    )
    .await;
}

#[wasm_bindgen_test]
async fn connections_in_one_context_follow_sqlite_locking() {
    vfs("locks").await;
    in_run(|| {
        let writer = Db::open("locks", "app.db").unwrap();
        let reader = Db::open("locks", "app.db").unwrap();
        writer.ok("CREATE TABLE t(x)");

        writer.ok("BEGIN IMMEDIATE");
        assert_eq!(reader.exec("BEGIN IMMEDIATE"), SQLITE_BUSY);
        assert_eq!(reader.text("SELECT count(*) FROM t"), "0");
        writer.ok("INSERT INTO t VALUES (1)");

        // A reader holding SHARED keeps the writer from committing.
        reader.ok("BEGIN");
        assert_eq!(reader.text("SELECT count(*) FROM t"), "0");
        assert_eq!(writer.exec("COMMIT"), SQLITE_BUSY);
        reader.ok("COMMIT");
        writer.ok("COMMIT");
        assert_eq!(reader.text("SELECT count(*) FROM t"), "1");
    })
    .await;
}

#[wasm_bindgen_test]
async fn open_files_cannot_be_removed() {
    let util = vfs("manage").await;
    in_run(move || {
        let db = Db::open("manage", "app.db").unwrap();
        db.ok("CREATE TABLE t(x)");
        assert!(matches!(
            util.remove("app.db"),
            Err(JspiIdbError::FileInUse(_))
        ));
        assert!(matches!(util.clear(), Err(JspiIdbError::FilesInUse)));
        drop(db);
        assert!(util.remove("app.db").unwrap());
        assert!(!util.contains("app.db").unwrap());
    })
    .await;
}

#[wasm_bindgen_test]
async fn one_vfs_per_indexeddb_database() {
    let util = vfs("first").await;
    let second = JspiIdbCfgBuilder::new()
        .vfs_name("second")
        .database(util.database())
        .build();
    let installed = install::<WasmOsCallback>(&second, false).await;
    assert!(matches!(installed, Err(JspiIdbError::DatabaseInUse(_))));
}
