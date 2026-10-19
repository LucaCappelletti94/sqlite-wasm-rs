//! Threads racing to install and look up memvfs through the safe accessors, before SQLite is initialized.
#![cfg(feature = "threadsafe")]
#![forbid(unsafe_code)]

use core::time::Duration;
use libsqlite3_sys as _;
use rsqlite_vfs::memvfs::{self, MemVfsUtil};
use rsqlite_vfs::transfer::DbTransfer;
use rsqlite_vfs::{OsCallback, VfsFilesManager, VfsResult};
use std::sync::Barrier;

struct NativeOs;

impl OsCallback for NativeOs {
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn random(&self, buf: &mut [u8]) -> usize {
        buf.fill(0x5a);
        buf.len()
    }

    fn epoch_timestamp_in_ms(&self) -> VfsResult<i64> {
        Ok(0)
    }
}

#[test]
fn threads_share_one_memvfs_through_safe_accessors() {
    const THREADS: usize = 16;

    let start = Barrier::new(THREADS);
    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            let start = &start;
            scope.spawn(move || {
                start.wait();
                let installed = memvfs::install(NativeOs, false).unwrap();
                let name = format!("thread-{thread}.db");
                installed.import_db_unchecked(&name, &[42; 512]).unwrap();
                assert_eq!(
                    MemVfsUtil::get().unwrap().export_db(&name).unwrap(),
                    [42; 512]
                );
            });
        }
    });

    let util = MemVfsUtil::get().unwrap();
    let mut names = util.names().unwrap();
    names.sort();
    let mut expected: Vec<_> = (0..THREADS)
        .map(|thread| format!("thread-{thread}.db"))
        .collect();
    expected.sort();
    assert_eq!(names, expected);
}
