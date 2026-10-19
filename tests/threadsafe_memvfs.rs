//! Workers whose first SQLite use is a race on the safe memvfs accessors.

#[path = "support/workers.rs"]
mod workers;

use core::time::Duration;
use sqlite_wasm_rs::vfs::memvfs::{self, MemVfsUtil};
use sqlite_wasm_rs::vfs::transfer::DbTransfer;
use sqlite_wasm_rs::vfs::{OsCallback, VfsFilesManager, VfsResult};
use std::sync::{Arc, Barrier};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

struct UnusedOs;

impl OsCallback for UnusedOs {
    fn sleep(&self, _: Duration) {}

    fn random(&self, buf: &mut [u8]) -> usize {
        buf.fill(0);
        buf.len()
    }

    fn epoch_timestamp_in_ms(&self) -> VfsResult<i64> {
        Ok(0)
    }
}

#[wasm_bindgen_test]
async fn workers_race_first_lookup_and_install() {
    const WORKERS: usize = 8;

    let start = Arc::new(Barrier::new(WORKERS));
    let tasks = (0..WORKERS)
        .map(|worker| {
            let start = start.clone();
            workers::task(move || {
                start.wait();
                let util = if worker % 2 == 0 {
                    MemVfsUtil::get().unwrap()
                } else {
                    memvfs::install(UnusedOs, false).unwrap()
                };
                util.import_db_unchecked(&format!("race-{worker}.db"), &[42; 512])
                    .unwrap();
            })
        })
        .collect();
    workers::spawn(tasks).await.unwrap();

    let util = MemVfsUtil::get().unwrap();
    let mut names = util.names().unwrap();
    names.sort();
    let mut expected: Vec<_> = (0..WORKERS)
        .map(|worker| format!("race-{worker}.db"))
        .collect();
    expected.sort();
    assert_eq!(names, expected);
    util.clear().unwrap();
}
