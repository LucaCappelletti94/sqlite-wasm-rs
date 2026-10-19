#![cfg(feature = "threadsafe")]

use libsqlite3_sys::{sqlite3_compileoption_used, sqlite3_threadsafe};
use rsqlite_vfs::memvfs::{MemVfsError, MemVfsUtil};
use rsqlite_vfs::RegisterVfsError;

#[test]
fn safe_lookup_rejects_single_threaded_sqlite_builds() {
    // SAFETY: compile-option queries read immutable SQLite build settings.
    let single_threaded = unsafe {
        sqlite3_threadsafe() == 0 || sqlite3_compileoption_used(c"MUTEX_NOOP".as_ptr()) != 0
    };
    let result = MemVfsUtil::get();
    if single_threaded {
        assert!(matches!(
            result,
            Err(MemVfsError::Registration(
                RegisterVfsError::SingleThreaded | RegisterVfsError::UnverifiedMutexes
            ))
        ));
    } else {
        assert!(matches!(result, Err(MemVfsError::NotInstalled)));
    }
}
