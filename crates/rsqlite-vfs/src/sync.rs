//! Sharing primitives for memvfs, single-threaded by default and guarded by SQLite's mutexes with `threadsafe`.
//!
//! SQLite's mutexes keep the crate `no_std` and let the host decide how a waiting thread blocks.

#[cfg(not(feature = "threadsafe"))]
mod imp {
    use crate::{RegisterVfsError, VfsError, VfsResult};
    use alloc::rc::Rc;
    use core::cell::{RefCell, RefMut};

    pub(crate) use alloc::rc::Rc as Shared;

    /// A value borrowed exclusively on one thread.
    pub(crate) struct Locked<T>(RefCell<T>);

    impl<T> Locked<T> {
        pub(crate) fn new(value: T) -> VfsResult<Self> {
            Ok(Self(RefCell::new(value)))
        }

        pub(crate) fn lock(&self) -> RefMut<'_, T> {
            self.0.borrow_mut()
        }
    }

    /// The last VFS error, shared by every handle to one memvfs.
    #[derive(Clone)]
    pub(crate) struct LastError(Rc<RefCell<Option<VfsError>>>);

    impl LastError {
        pub(crate) fn new() -> Self {
            Self(Rc::default())
        }

        pub(crate) fn set(&self, error: VfsError) {
            self.0.replace(Some(error));
        }

        pub(crate) fn get(&self) -> Option<VfsError> {
            self.0.borrow().clone()
        }
    }

    /// Serializes memvfs installation, which on one thread needs no lock.
    pub(crate) struct RegistrationGuard;

    impl RegistrationGuard {
        pub(crate) fn enter() -> Result<Self, RegisterVfsError> {
            Ok(Self)
        }
    }
}

#[cfg(feature = "threadsafe")]
mod imp {
    use crate::ffi::{
        sqlite3_compileoption_used, sqlite3_initialize, sqlite3_mutex, sqlite3_mutex_alloc,
        sqlite3_mutex_enter, sqlite3_mutex_free, sqlite3_mutex_leave, sqlite3_threadsafe,
        SQLITE_MUTEX_FAST, SQLITE_MUTEX_STATIC_VFS2, SQLITE_MUTEX_STATIC_VFS3, SQLITE_OK,
    };
    use crate::{RegisterVfsError, VfsError, VfsErrorCode, VfsResult};
    use core::cell::{RefCell, UnsafeCell};
    use core::marker::PhantomData;
    use core::ops::{Deref, DerefMut};
    use core::ptr::NonNull;

    pub(crate) use alloc::sync::Arc as Shared;

    /// A value guarded by a `SQLITE_MUTEX_FAST` mutex from SQLite's configured mutex methods.
    pub(crate) struct Locked<T> {
        mutex: NonNull<sqlite3_mutex>,
        value: UnsafeCell<T>,
    }

    // SAFETY: `value` is only reachable through `lock`, which holds `mutex`, so sharing or
    // moving a `Locked<T>` hands the value between threads, which `T: Send` permits.
    unsafe impl<T: Send> Send for Locked<T> {}
    // SAFETY: as for `Send`, every access to `value` is serialized by `mutex`.
    unsafe impl<T: Send> Sync for Locked<T> {}

    impl<T> Locked<T> {
        pub(crate) fn new(value: T) -> VfsResult<Self> {
            // SAFETY: allocating a fast mutex has no precondition, SQLite initializes itself first if needed.
            let mutex = NonNull::new(unsafe { sqlite3_mutex_alloc(SQLITE_MUTEX_FAST) })
                .ok_or_else(|| {
                    VfsError::new(VfsErrorCode::NoMemory, "unable to allocate a mutex".into())
                })?;
            Ok(Self {
                mutex,
                value: UnsafeCell::new(value),
            })
        }

        pub(crate) fn lock(&self) -> Guard<'_, T> {
            // SAFETY: `mutex` came from `sqlite3_mutex_alloc` and lives until `drop`.
            unsafe { sqlite3_mutex_enter(self.mutex.as_ptr()) };
            Guard {
                locked: self,
                _not_send: PhantomData,
            }
        }
    }

    impl<T> Drop for Locked<T> {
        fn drop(&mut self) {
            // SAFETY: `&mut self` proves no guard is alive, and the mutex is freed exactly once.
            unsafe { sqlite3_mutex_free(self.mutex.as_ptr()) };
        }
    }

    /// Holds the mutex until dropped on the thread that entered it.
    pub(crate) struct Guard<'a, T> {
        locked: &'a Locked<T>,
        // SQLite requires the entering thread to leave the mutex.
        _not_send: PhantomData<*const ()>,
    }

    impl<T> Deref for Guard<'_, T> {
        type Target = T;

        fn deref(&self) -> &T {
            // SAFETY: this guard holds the mutex, so no other reference to the value exists.
            unsafe { &*self.locked.value.get() }
        }
    }

    impl<T> DerefMut for Guard<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            // SAFETY: this guard holds the mutex and `&mut self` excludes other borrows through it.
            unsafe { &mut *self.locked.value.get() }
        }
    }

    impl<T> Drop for Guard<'_, T> {
        fn drop(&mut self) {
            // SAFETY: this thread entered the mutex in `Locked::lock` and leaves it once.
            unsafe { sqlite3_mutex_leave(self.locked.mutex.as_ptr()) };
        }
    }

    std::thread_local! {
        static LAST_ERROR: RefCell<Option<VfsError>> = const { RefCell::new(None) };
    }

    /// The calling thread's last VFS error, since `xGetLastError` names no connection.
    #[derive(Clone)]
    pub(crate) struct LastError;

    impl LastError {
        pub(crate) fn new() -> Self {
            Self
        }

        pub(crate) fn set(&self, error: VfsError) {
            LAST_ERROR.with(|slot| slot.replace(Some(error)));
        }

        pub(crate) fn get(&self) -> Option<VfsError> {
            LAST_ERROR.with(|slot| slot.borrow().clone())
        }
    }

    /// Holds `SQLITE_MUTEX_STATIC_VFS2`, SQLite's static mutex for extension VFSes, to serialize memvfs installation.
    pub(crate) struct RegistrationGuard {
        mutex: NonNull<sqlite3_mutex>,
        // SQLite requires the entering thread to leave the mutex.
        _not_send: PhantomData<*const ()>,
    }

    impl RegistrationGuard {
        /// Initializes before locking to permit recursive installation from `sqlite3_os_init`.
        pub(crate) fn enter() -> Result<Self, RegisterVfsError> {
            // SAFETY: initialization has no precondition, and SQLite serializes concurrent calls on its own mutexes.
            let code = unsafe { sqlite3_initialize() };
            if code != SQLITE_OK {
                return Err(RegisterVfsError::RegisterVfs(
                    VfsErrorCode::from_raw(code).expect("SQLite must return a valid error code"),
                ));
            }
            // SAFETY: initialization set up the mutex subsystem, and static mutexes are never freed.
            let mutex = NonNull::new(unsafe { sqlite3_mutex_alloc(SQLITE_MUTEX_STATIC_VFS2) })
                .ok_or(RegisterVfsError::RegisterVfs(VfsErrorCode::NoMemory))?;
            // SAFETY: `mutex` is a live static mutex, and memvfs never enters it while holding it.
            unsafe { sqlite3_mutex_enter(mutex.as_ptr()) };
            Ok(Self {
                mutex,
                _not_send: PhantomData,
            })
        }
    }

    impl Drop for RegistrationGuard {
        fn drop(&mut self) {
            // SAFETY: this guard is not `Send`, so the thread that entered the mutex in `enter` leaves it once.
            unsafe { sqlite3_mutex_leave(self.mutex.as_ptr()) };
        }
    }

    /// Rejects disabled, no-op or unverifiable mutex implementations.
    pub(crate) fn ensure_real_mutexes() -> Result<(), RegisterVfsError> {
        // SAFETY: these queries read immutable SQLite build settings.
        unsafe {
            if sqlite3_threadsafe() == 0 {
                return Err(RegisterVfsError::SingleThreaded);
            }
            if sqlite3_compileoption_used(c"MUTEX_NOOP".as_ptr()) != 0
                && sqlite3_compileoption_used(c"DEBUG".as_ptr()) != 0
            {
                return Err(RegisterVfsError::UnverifiedMutexes);
            }
        }
        // Release no-op mutexes share one dummy, including the OS_OTHER fallback.
        // SAFETY: static mutex allocation initializes the subsystem and never frees its mutexes.
        let distinct = unsafe {
            let first = sqlite3_mutex_alloc(SQLITE_MUTEX_STATIC_VFS2);
            !first.is_null() && first != sqlite3_mutex_alloc(SQLITE_MUTEX_STATIC_VFS3)
        };
        if distinct {
            Ok(())
        } else {
            Err(RegisterVfsError::SingleThreaded)
        }
    }
}

#[cfg(feature = "threadsafe")]
pub(crate) use imp::ensure_real_mutexes;
pub(crate) use imp::{LastError, Locked, RegistrationGuard, Shared};
