//! Recursive SQLite mutexes on Wasm atomics, installed as SQLite's default by `shim/threadsafe.h`.
//!
//! A contended mutex spins briefly, then sleeps in `memory.atomic.wait32` on threads
//! that may block. The browser main thread may not, so it keeps spinning.

use crate::bindings::{
    sqlite3_mutex, sqlite3_mutex_methods, SQLITE_BUSY, SQLITE_MUTEX_FAST, SQLITE_MUTEX_RECURSIVE,
    SQLITE_MUTEX_STATIC_MAIN, SQLITE_MUTEX_STATIC_VFS3, SQLITE_OK,
};
use alloc::alloc::{alloc, dealloc, Layout};
use core::ffi::c_int;
use core::hint::spin_loop;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

/// Spins before sleeping, enough to cover most of SQLite's short critical sections.
const SPINS: u32 = 100;

const FREE: i32 = 0;
const LOCKED: i32 = 1;
const CONTENDED: i32 = 2;

struct Mutex {
    /// `FREE`, `LOCKED`, or `CONTENDED` when a thread may be asleep on it.
    state: AtomicI32,
    /// Holder's thread identity, zero when free.
    owner: AtomicUsize,
    /// Recursion depth, read and written only by the holder.
    depth: AtomicU32,
}

impl Mutex {
    const fn new() -> Self {
        Self {
            state: AtomicI32::new(FREE),
            owner: AtomicUsize::new(0),
            depth: AtomicU32::new(0),
        }
    }

    fn enter(&self) {
        let me = thread::id();
        if self.owner.load(Ordering::Relaxed) == me {
            self.depth.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.acquire();
        self.owner.store(me, Ordering::Relaxed);
        self.depth.store(1, Ordering::Relaxed);
    }

    fn try_enter(&self) -> bool {
        let me = thread::id();
        if self.owner.load(Ordering::Relaxed) == me {
            self.depth.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        if !self.try_acquire() {
            return false;
        }
        self.owner.store(me, Ordering::Relaxed);
        self.depth.store(1, Ordering::Relaxed);
        true
    }

    fn leave(&self) {
        if self.depth.fetch_sub(1, Ordering::Relaxed) > 1 {
            return;
        }
        self.owner.store(0, Ordering::Relaxed);
        if self.state.swap(FREE, Ordering::Release) == CONTENDED {
            thread::wake_one(&self.state);
        }
    }

    fn held(&self) -> bool {
        self.owner.load(Ordering::Relaxed) == thread::id()
    }

    fn try_acquire(&self) -> bool {
        self.state
            .compare_exchange(FREE, LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn acquire(&self) {
        for _ in 0..SPINS {
            if self.state.load(Ordering::Relaxed) == FREE && self.try_acquire() {
                return;
            }
            spin_loop();
        }
        if thread::can_block() {
            // Taking the lock as `CONTENDED` keeps later sleepers woken by `leave`.
            while self.state.swap(CONTENDED, Ordering::Acquire) != FREE {
                thread::wait(&self.state, CONTENDED);
            }
            return;
        }
        while !(self.state.load(Ordering::Relaxed) == FREE && self.try_acquire()) {
            spin_loop();
        }
    }
}

#[cfg(target_feature = "atomics")]
mod thread {
    use core::arch::wasm32::{memory_atomic_notify, memory_atomic_wait32};
    use core::sync::atomic::AtomicI32;

    #[thread_local]
    static MARKER: u8 = 0;

    /// Unique among live threads, since every worker owns a separate TLS block.
    pub(super) fn id() -> usize {
        core::ptr::addr_of!(MARKER) as usize
    }

    pub(super) fn can_block() -> bool {
        crate::host::can_block()
    }

    pub(super) fn wait(state: &AtomicI32, expected: i32) {
        // SAFETY: `state` is a live, aligned atomic in shared memory, and `can_block` confirmed this
        // thread may wait. A negative timeout waits until notified.
        unsafe { memory_atomic_wait32(state.as_ptr(), expected, -1) };
    }

    pub(super) fn wake_one(state: &AtomicI32) {
        // SAFETY: `state` is a live, aligned atomic in shared memory.
        unsafe { memory_atomic_notify(state.as_ptr(), 1) };
    }
}

#[cfg(not(target_feature = "atomics"))]
mod thread {
    use core::sync::atomic::AtomicI32;

    // Without shared memory the module has exactly one thread.
    pub(super) fn id() -> usize {
        1
    }

    pub(super) fn can_block() -> bool {
        false
    }

    pub(super) fn wait(_state: &AtomicI32, _expected: i32) {}

    pub(super) fn wake_one(_state: &AtomicI32) {}
}

// The difference of two small positive SQLite constants, twelve static mutexes.
const STATIC_COUNT: usize = (SQLITE_MUTEX_STATIC_VFS3 - SQLITE_MUTEX_STATIC_MAIN + 1) as usize;
static STATICS: [Mutex; STATIC_COUNT] = [const { Mutex::new() }; STATIC_COUNT];

/// # Safety
///
/// `mutex` came from [`mutex_alloc`] and has not been freed.
unsafe fn from_raw<'a>(mutex: *mut sqlite3_mutex) -> &'a Mutex {
    // SAFETY: the caller passes a live `Mutex` allocation or static, which is never mutated through `&`.
    unsafe { &*mutex.cast::<Mutex>() }
}

unsafe extern "C" fn mutex_init() -> c_int {
    SQLITE_OK
}

unsafe extern "C" fn mutex_end() -> c_int {
    SQLITE_OK
}

unsafe extern "C" fn mutex_alloc(kind: c_int) -> *mut sqlite3_mutex {
    match kind {
        SQLITE_MUTEX_FAST | SQLITE_MUTEX_RECURSIVE => {
            let layout = Layout::new::<Mutex>();
            // SAFETY: `Mutex` has a nonzero size.
            let mutex = unsafe { alloc(layout) }.cast::<Mutex>();
            if !mutex.is_null() {
                // SAFETY: `mutex` is a fresh allocation with the layout of `Mutex`.
                unsafe { mutex.write(Mutex::new()) };
            }
            mutex.cast()
        }
        _ => usize::try_from(kind - SQLITE_MUTEX_STATIC_MAIN)
            .ok()
            .and_then(|index| STATICS.get(index))
            .map_or(core::ptr::null_mut(), |mutex| {
                core::ptr::from_ref(mutex).cast_mut().cast()
            }),
    }
}

unsafe extern "C" fn mutex_free(mutex: *mut sqlite3_mutex) {
    // SAFETY: SQLite frees only the dynamic mutexes `mutex_alloc` allocated with this layout.
    unsafe { dealloc(mutex.cast(), Layout::new::<Mutex>()) };
}

unsafe extern "C" fn mutex_enter(mutex: *mut sqlite3_mutex) {
    // SAFETY: SQLite passes a live mutex from `mutex_alloc`.
    unsafe { from_raw(mutex) }.enter();
}

unsafe extern "C" fn mutex_try(mutex: *mut sqlite3_mutex) -> c_int {
    // SAFETY: SQLite passes a live mutex from `mutex_alloc`.
    if unsafe { from_raw(mutex) }.try_enter() {
        SQLITE_OK
    } else {
        SQLITE_BUSY
    }
}

unsafe extern "C" fn mutex_leave(mutex: *mut sqlite3_mutex) {
    // SAFETY: SQLite passes a live mutex from `mutex_alloc` that this thread entered.
    unsafe { from_raw(mutex) }.leave();
}

unsafe extern "C" fn mutex_held(mutex: *mut sqlite3_mutex) -> c_int {
    // SAFETY: SQLite passes a live, non-null mutex from `mutex_alloc`.
    c_int::from(unsafe { from_raw(mutex) }.held())
}

unsafe extern "C" fn mutex_notheld(mutex: *mut sqlite3_mutex) -> c_int {
    // SAFETY: SQLite passes a live, non-null mutex from `mutex_alloc`.
    c_int::from(!unsafe { from_raw(mutex) }.held())
}

static METHODS: sqlite3_mutex_methods = sqlite3_mutex_methods {
    xMutexInit: Some(mutex_init),
    xMutexEnd: Some(mutex_end),
    xMutexAlloc: Some(mutex_alloc),
    xMutexFree: Some(mutex_free),
    xMutexEnter: Some(mutex_enter),
    xMutexTry: Some(mutex_try),
    xMutexLeave: Some(mutex_leave),
    xMutexHeld: Some(mutex_held),
    xMutexNotheld: Some(mutex_notheld),
};

/// SQLite's default mutex methods, reached through the redirect in `shim/threadsafe.h`.
///
/// The table is static and immutable, so SQLite may copy it at any time.
#[no_mangle]
pub extern "C" fn rust_sqlite_wasm_mutex_methods() -> *const sqlite3_mutex_methods {
    &METHODS
}
