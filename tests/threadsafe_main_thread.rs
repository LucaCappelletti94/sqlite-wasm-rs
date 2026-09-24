//! The browser main thread may not block, so SQLite must spin there instead of waiting.

#[path = "support/workers.rs"]
mod workers;

use sqlite_wasm_rs::{
    sqlite3_mutex_alloc, sqlite3_mutex_enter, sqlite3_mutex_leave, sqlite3_sleep,
    SQLITE_MUTEX_STATIC_APP2,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(
    inline_js = "export function delay(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }"
)]
extern "C" {
    fn delay(ms: u32) -> js_sys::Promise;
}

#[wasm_bindgen_test]
fn sleep_returns_on_the_main_thread() {
    // SAFETY: sleeping has no precondition.
    unsafe { sqlite3_sleep(1) };
}

#[wasm_bindgen_test]
async fn main_thread_spins_for_a_mutex_held_by_a_worker() {
    #[derive(Default)]
    struct Flags {
        held: AtomicBool,
        released: AtomicBool,
    }
    let flags = Arc::new(Flags::default());
    let holder = {
        let flags = flags.clone();
        move || {
            // SAFETY: static mutexes are allocated by SQLite for the whole process lifetime.
            let mutex = unsafe { sqlite3_mutex_alloc(SQLITE_MUTEX_STATIC_APP2) };
            // SAFETY: `mutex` is valid, entered here and left below on this worker.
            unsafe { sqlite3_mutex_enter(mutex) };
            flags.held.store(true, Ordering::Release);
            std::thread::sleep(Duration::from_millis(100));
            flags.released.store(true, Ordering::Release);
            // SAFETY: entered above on this worker.
            unsafe { sqlite3_mutex_leave(mutex) };
        }
    };
    let done = workers::spawn(vec![workers::task(holder)], 60_000);
    while !flags.held.load(Ordering::Acquire) {
        JsFuture::from(delay(1)).await.unwrap();
    }
    // SAFETY: static mutexes are allocated by SQLite for the whole process lifetime.
    let mutex = unsafe { sqlite3_mutex_alloc(SQLITE_MUTEX_STATIC_APP2) };
    // SAFETY: `mutex` is valid, entered here and left below on the main thread.
    unsafe { sqlite3_mutex_enter(mutex) };
    assert!(
        flags.released.load(Ordering::Acquire),
        "entered while the worker held the mutex"
    );
    // SAFETY: entered above on this thread.
    unsafe { sqlite3_mutex_leave(mutex) };
    done.await.unwrap();
}
