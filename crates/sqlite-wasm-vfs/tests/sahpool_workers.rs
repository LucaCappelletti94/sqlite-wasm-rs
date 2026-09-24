//! A pool's sync access handles live in the installing worker, so other workers must be refused.
#![cfg(target_feature = "atomics")]

#[path = "../../../tests/support/workers.rs"]
mod workers;

use sqlite_wasm_rs::*;
use sqlite_wasm_vfs::sahpool::{install, OpfsSAHPoolCfgBuilder};
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);

const VFS: &CStr = c"sahpool-owner";
const OTHER_VFS: &CStr = c"sahpool-other";

fn open(name: &str, vfs: &CStr) -> (i32, *mut sqlite3) {
    let name = CString::new(name).unwrap();
    let mut db = std::ptr::null_mut();
    // SAFETY: both strings are NUL-terminated and outlive the call, and `db` is a valid out pointer.
    let rc = unsafe {
        sqlite3_open_v2(
            name.as_ptr(),
            &mut db,
            SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
            vfs.as_ptr(),
        )
    };
    (rc, db)
}

/// Prepares and steps once, returning the first failing code or the step result and its value.
fn exec(db: *mut sqlite3, sql: &CStr) -> i32 {
    // SAFETY: `db` is an open connection and `sql` is NUL-terminated.
    unsafe {
        sqlite3_exec(
            db,
            sql.as_ptr(),
            None,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    }
}

async fn install_pool(vfs: &CStr) {
    let name = vfs.to_str().unwrap();
    let config = OpfsSAHPoolCfgBuilder::new()
        .vfs_name(name)
        .directory(&format!("sqlite-wasm-vfs-tests/{name}"))
        .clear_on_init(true)
        .build();
    install::<WasmOsCallback>(&config, false).await.unwrap();
}

fn first_integer(db: *mut sqlite3, sql: &CStr) -> (i32, i64) {
    let mut stmt = std::ptr::null_mut();
    // SAFETY: `db` is an open connection, `sql` is NUL-terminated and `stmt` is a valid out pointer.
    let rc = unsafe { sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) };
    if rc != SQLITE_OK {
        return (rc, 0);
    }
    // SAFETY: `stmt` was prepared above, read only while it holds a row, and finalized once.
    unsafe {
        let rc = sqlite3_step(stmt);
        let value = if rc == SQLITE_ROW {
            sqlite3_column_int64(stmt, 0)
        } else {
            0
        };
        sqlite3_finalize(stmt);
        (rc, value)
    }
}

struct Connection(*mut sqlite3);

// SAFETY: the owner awaits the other worker, so the connection is never used by two threads at once.
unsafe impl Send for Connection {}
// SAFETY: as for `Send`, uses of the connection never overlap in time.
unsafe impl Sync for Connection {}

#[wasm_bindgen_test]
async fn other_workers_are_refused() {
    install_pool(VFS).await;
    let (rc, db) = open("owner.db", VFS);
    assert_eq!(rc, SQLITE_OK);
    assert_eq!(
        exec(db, c"CREATE TABLE t(x); INSERT INTO t VALUES (42);"),
        SQLITE_OK
    );

    let owner = Arc::new(Connection(db));
    let open_rc = Arc::new(AtomicI32::new(SQLITE_OK));
    let query = Arc::new(std::sync::Mutex::new((SQLITE_OK, 0)));
    let other = {
        let (owner, open_rc, query) = (owner.clone(), open_rc.clone(), query.clone());
        workers::async_task(move || async move {
            // This worker's own pool fills its handle table the way the owner's pool filled the owner's.
            install_pool(OTHER_VFS).await;
            let (rc, own) = open("other.db", OTHER_VFS);
            assert_eq!(rc, SQLITE_OK);
            assert_eq!(
                exec(own, c"CREATE TABLE t(x); INSERT INTO t VALUES (7);"),
                SQLITE_OK
            );
            // SAFETY: no statement is pending on `own`.
            assert_eq!(unsafe { sqlite3_close(own) }, SQLITE_OK);

            let (rc, other) = open("owner.db", VFS);
            open_rc.store(rc, Ordering::Relaxed);
            // SAFETY: SQLite returns a handle to close even when opening fails.
            unsafe { sqlite3_close(other) };
            *query.lock().unwrap() = first_integer(owner.0, c"SELECT x FROM t");
        })
    };
    workers::spawn(vec![other], 60_000).await.unwrap();

    // Refused as misuse, where an unguarded pool reports a disk I/O error from the foreign handle.
    assert_eq!(open_rc.load(Ordering::Relaxed), SQLITE_MISUSE);
    assert_eq!(*query.lock().unwrap(), (SQLITE_MISUSE, 0));
    assert_eq!(first_integer(db, c"SELECT x FROM t"), (SQLITE_ROW, 42));
    // SAFETY: every statement was finalized and the other worker has finished.
    assert_eq!(unsafe { sqlite3_close(db) }, SQLITE_OK);
}
