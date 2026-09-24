//! Several threads, each with its own connection, sharing one memvfs database.
#![cfg(feature = "threadsafe")]

mod ffi {
    pub use libsqlite3_sys::*;
}
#[path = "support/scenarios.rs"]
mod scenarios;

use rsqlite_vfs::{memvfs, OsCallback, VfsResult};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
        let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        Ok(i64::try_from(elapsed.as_millis()).unwrap())
    }
}

fn install() {
    static INSTALL: Once = Once::new();
    // SAFETY: registration is serialized by `Once`, and memvfs stays installed for the whole binary.
    INSTALL.call_once(|| unsafe {
        memvfs::install(NativeOs, false).unwrap();
    });
}

/// Counts a writer as finished even when it panics, so readers never wait forever.
struct Finished<'a>(&'a AtomicUsize);

impl Drop for Finished<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}

#[test]
fn concurrent_increments_lose_no_update() {
    const THREADS: usize = 8;
    const INCREMENTS: usize = 200;

    install();
    let setup = scenarios::create_counter("increments.db");
    let line = scenarios::StartLine::new(THREADS);
    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| scenarios::increment("increments.db", INCREMENTS, &|| line.wait()));
        }
    });
    scenarios::assert_counter(&setup, THREADS * INCREMENTS);
    setup.close();
}

#[test]
fn readers_see_only_committed_states() {
    const WRITERS: usize = 4;
    const READERS: usize = 4;
    const PAIRS: usize = 250;

    install();
    let setup = scenarios::create_pairs("snapshots.db");
    let line = scenarios::StartLine::new(WRITERS + READERS);
    let writers_done = AtomicUsize::new(0);
    let start = || line.wait();
    std::thread::scope(|scope| {
        for writer in 0..WRITERS {
            let (start, writers_done) = (&start, &writers_done);
            scope.spawn(move || {
                let _done = Finished(writers_done);
                scenarios::write_pairs("snapshots.db", writer, PAIRS, start);
            });
        }
        for _ in 0..READERS {
            let (start, writers_done) = (&start, &writers_done);
            scope.spawn(move || {
                let done = || writers_done.load(Ordering::Acquire) == WRITERS;
                scenarios::read_pairs("snapshots.db", &done, start);
            });
        }
    });
    scenarios::assert_pairs(&setup, WRITERS, PAIRS);
    setup.close();
}
