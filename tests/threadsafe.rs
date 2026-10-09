//! Dedicated workers sharing this module and its memory, driving SQLite concurrently.

#[path = "support/scenarios.rs"]
mod scenarios;
#[path = "support/workers.rs"]
mod workers;

use scenarios::{init, Connection, StartLine};
use sqlite_wasm_rs::{
    sqlite3_mutex_alloc, sqlite3_mutex_enter, sqlite3_mutex_leave, sqlite3_mutex_try,
    sqlite3_threadsafe, SQLITE_BUSY, SQLITE_MUTEX_STATIC_APP1, SQLITE_OK,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

const TIMEOUT_MS: u32 = 120_000;

fn spin_until(flag: &AtomicBool) {
    while !flag.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }
}

#[wasm_bindgen_test]
fn sqlite_is_threadsafe() {
    // SAFETY: querying the compile-time threading mode has no precondition.
    assert_eq!(unsafe { sqlite3_threadsafe() }, 1);
}

/// Also fails if SQLite stops calling `sqlite3DefaultMutex()`, since its no-op mutex lets the try succeed.
#[wasm_bindgen_test]
async fn mutex_excludes_other_workers_and_recurses() {
    #[derive(Default)]
    struct Flags {
        held: AtomicBool,
        probed: AtomicBool,
        released: AtomicBool,
    }

    init();
    let flags = Arc::new(Flags::default());
    let holder = {
        let flags = flags.clone();
        move || {
            // SAFETY: static mutexes are allocated by SQLite for the whole process lifetime.
            let mutex = unsafe { sqlite3_mutex_alloc(SQLITE_MUTEX_STATIC_APP1) };
            // SAFETY: `mutex` is a valid static mutex, entered twice and left twice on this worker.
            unsafe {
                sqlite3_mutex_enter(mutex);
                sqlite3_mutex_enter(mutex);
            }
            flags.held.store(true, Ordering::Release);
            spin_until(&flags.probed);
            // SAFETY: this worker entered the mutex twice above.
            unsafe { sqlite3_mutex_leave(mutex) };
            // One level is still held, so the prober must keep waiting through this pause.
            std::thread::sleep(Duration::from_millis(50));
            flags.released.store(true, Ordering::Release);
            // SAFETY: this worker still holds the second level.
            unsafe { sqlite3_mutex_leave(mutex) };
        }
    };
    let prober = {
        let flags = flags.clone();
        move || {
            // SAFETY: static mutexes are allocated by SQLite for the whole process lifetime.
            let mutex = unsafe { sqlite3_mutex_alloc(SQLITE_MUTEX_STATIC_APP1) };
            spin_until(&flags.held);
            // SAFETY: `mutex` is valid, and a failed try leaves nothing to release.
            assert_eq!(unsafe { sqlite3_mutex_try(mutex) }, SQLITE_BUSY);
            flags.probed.store(true, Ordering::Release);
            // SAFETY: `mutex` is valid, entered once here and left once below on this worker.
            unsafe { sqlite3_mutex_enter(mutex) };
            assert!(
                flags.released.load(Ordering::Acquire),
                "entered while another worker held the mutex"
            );
            // SAFETY: entered above on this worker.
            unsafe { sqlite3_mutex_leave(mutex) };
            // SAFETY: the mutex is free again, so a try succeeds and is balanced by a leave.
            assert_eq!(unsafe { sqlite3_mutex_try(mutex) }, SQLITE_OK);
            // SAFETY: entered by the successful try above.
            unsafe { sqlite3_mutex_leave(mutex) };
        }
    };
    workers::spawn(
        vec![workers::task(holder), workers::task(prober)],
        TIMEOUT_MS,
    )
    .await
    .unwrap();
}

#[wasm_bindgen_test]
async fn concurrent_increments_lose_no_update() {
    const WORKERS: usize = 8;
    const INCREMENTS: usize = 100;
    const DB: &str = "workers-increments.db";

    init();
    let setup = Connection::open(DB);
    scenarios::create_counter(&setup);
    let start = Arc::new(StartLine::new(WORKERS));
    let tasks = (0..WORKERS)
        .map(|_| {
            let start = start.clone();
            workers::task(move || {
                scenarios::increment(&|| Connection::open(DB), INCREMENTS, &start)
            })
        })
        .collect();
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();
    scenarios::assert_counter(&setup, WORKERS * INCREMENTS);
}

#[wasm_bindgen_test]
async fn readers_see_only_committed_states() {
    const WRITERS: usize = 4;
    const READERS: usize = 4;
    const PAIRS: usize = 100;
    const DB: &str = "workers-snapshots.db";

    init();
    let setup = Connection::open(DB);
    scenarios::create_pairs(&setup);
    let start = Arc::new(StartLine::new(WRITERS + READERS));
    let writers_done = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for writer in 0..WRITERS {
        let (start, writers_done) = (start.clone(), writers_done.clone());
        tasks.push(workers::task(move || {
            scenarios::write_pairs(&|| Connection::open(DB), writer, PAIRS, &start);
            writers_done.fetch_add(1, Ordering::Release);
        }));
    }
    for _ in 0..READERS {
        let (start, writers_done, reads) = (start.clone(), writers_done.clone(), reads.clone());
        tasks.push(workers::task(move || {
            let done = || writers_done.load(Ordering::Acquire) == WRITERS;
            let count = scenarios::read_pairs(&|| Connection::open(DB), &done, &start);
            reads.fetch_add(count, Ordering::Relaxed);
        }));
    }
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();
    scenarios::assert_pairs(&setup, WRITERS, PAIRS);
    assert!(
        reads.load(Ordering::Relaxed) > READERS,
        "readers never overlapped the writers"
    );
}

#[wasm_bindgen_test]
async fn workers_share_one_serialized_connection() {
    const WORKERS: usize = 4;
    const UPDATES: usize = 100;
    const DB: &str = "workers-shared-connection.db";

    init();
    let shared = Arc::new(Connection::open(DB));
    scenarios::create_counter(&shared);
    let start = Arc::new(StartLine::new(WORKERS));
    let tasks = (0..WORKERS)
        .map(|_| {
            let (shared, start) = (shared.clone(), start.clone());
            workers::task(move || {
                start.wait();
                for _ in 0..UPDATES {
                    // One statement runs atomically under the connection mutex.
                    shared.exec("UPDATE counter SET n = n + 1");
                }
            })
        })
        .collect();
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();
    scenarios::assert_counter(&shared, WORKERS * UPDATES);
}

#[cfg(feature = "sqlite3mc")]
#[wasm_bindgen_test]
async fn encrypted_increments_lose_no_update() {
    const WORKERS: usize = 8;
    const INCREMENTS: usize = 100;
    const FILE: &str = "workers-encrypted.db";
    // The URI routes through SQLite3MC's cipher wrapper around memvfs, which holds the key.
    const DB: &str = "file:workers-encrypted.db?vfs=multipleciphers-memvfs";
    const KEY: &str = "PRAGMA key = 'threadsafe passphrase';";

    fn open() -> Connection {
        let connection = Connection::open(DB);
        connection.exec(KEY);
        connection
    }

    init();
    let setup = open();
    scenarios::create_counter(&setup);
    let start = Arc::new(StartLine::new(WORKERS));
    let tasks = (0..WORKERS)
        .map(|_| {
            let start = start.clone();
            workers::task(move || scenarios::increment(&open, INCREMENTS, &start))
        })
        .collect();
    workers::spawn(tasks, TIMEOUT_MS).await.unwrap();
    scenarios::assert_counter(&setup, WORKERS * INCREMENTS);

    // Read without the key, the pages stay ciphertext, so the workers really wrote through the cipher.
    let plain = Connection::open(FILE);
    assert_eq!(
        plain.try_query_row::<1>("SELECT n FROM counter"),
        Err(sqlite_wasm_rs::SQLITE_NOTADB)
    );
}
