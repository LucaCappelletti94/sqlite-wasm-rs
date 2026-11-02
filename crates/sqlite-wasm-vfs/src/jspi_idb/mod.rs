//! Durable IndexedDB VFS for engines with JS Promise Integration (JSPI).
//!
//! SQLite runs on a JSPI stack, so a VFS callback waits for IndexedDB before it
//! returns. A commit SQLite reports is an IndexedDB transaction that completed
//! with `durability: "strict"`. Works in windows and workers, Firefox private
//! windows included, which refuse OPFS.
//!
//! Every SQLite call that may reach this VFS must happen inside [`run`].
//!
//! ```no_run
//! use sqlite_wasm_rs as ffi;
//! use sqlite_wasm_vfs::jspi_idb::{install, run, JspiIdbCfg};
//!
//! async fn open_db() {
//!     install::<ffi::WasmOsCallback>(&JspiIdbCfg::default(), true).await.unwrap();
//!     run(|| unsafe {
//!         let mut db = std::ptr::null_mut();
//!         assert_eq!(ffi::sqlite3_open(c"app.db".as_ptr(), &mut db), ffi::SQLITE_OK);
//!         assert_eq!(ffi::sqlite3_close(db), ffi::SQLITE_OK);
//!     })
//!     .await
//!     .unwrap();
//! }
//! ```
//!
//! - A module with this feature needs JSPI and Wasm exceptions (Chrome 137, Firefox 153)
//!   and fails to instantiate elsewhere, so a fallback is a second build without it.
//!   Disable `wasm-opt` or pass it `--enable-exceptions`. Shared-memory threads are unsupported.
//! - Files are 4 KiB blocks. Writes stay in memory until a sync, batch commit, unlock or
//!   close stores them in one IndexedDB transaction, which lands whole or not at all.
//!   SQLite3MC builds have no batch atomic writes, so their commits use the journal.
//! - Windows and workers share a database through Web Locks, waiting at most
//!   [`JspiIdbCfgBuilder::lock_timeout`] before `SQLITE_BUSY`. WAL needs
//!   `PRAGMA locking_mode=EXCLUSIVE`. Web Locks need a secure context.

mod context;
mod file;
mod idb;
mod lock;

pub use context::run;

use file::{FileState, IdbFile};
use idb::{Durability, Idb, Meta};
use js_sys::Reflect;
use lock::LockManager;
use rsqlite_vfs::{
    ffi::{
        sqlite3_file, SQLITE_FCNTL_BEGIN_ATOMIC_WRITE, SQLITE_FCNTL_COMMIT_ATOMIC_WRITE,
        SQLITE_FCNTL_ROLLBACK_ATOMIC_WRITE, SQLITE_NOTFOUND, SQLITE_OK,
    },
    register_vfs, AccessMode, FileKind, LockLevel, OpenAccess, OpenOptions, OpenRequest,
    OpenedFile, OsCallback, RegisterVfsError, SQLiteIoMethods, SQLiteVfs, SQLiteVfsFile,
    VfsAppData, VfsError, VfsErrorCode, VfsFile, VfsResult, VfsStore,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{c_int, c_void},
    rc::Rc,
    time::Duration,
};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::Event;

pub(crate) const BLOCK_SIZE: usize = 4096;

fn describe(value: &JsValue) -> String {
    let property = |name: &str| Reflect::get(value, &name.into()).ok()?.as_string();
    match (property("name"), property("message")) {
        (Some(name), Some(message)) => format!("{name}: {message}"),
        (_, Some(message)) => message,
        _ => value.as_string().unwrap_or_else(|| format!("{value:?}")),
    }
}

/// Converts a browser error, reporting a full quota as `SQLITE_FULL`.
pub(crate) fn js_vfs_error(code: VfsErrorCode, operation: &str, value: JsValue) -> VfsError {
    let name = Reflect::get(&value, &"name".into())
        .ok()
        .and_then(|name| name.as_string());
    let code = if name.as_deref() == Some("QuotaExceededError") {
        VfsErrorCode::Full
    } else {
        code
    };
    VfsError::new(code, format!("{operation}: {}", describe(&value)).into())
}

/// Builds a [`JspiIdbCfg`]. Validation occurs during [`install`].
pub struct JspiIdbCfgBuilder(JspiIdbCfg);

impl JspiIdbCfgBuilder {
    pub fn new() -> Self {
        Self(JspiIdbCfg::default())
    }

    /// SQLite VFS name, `jspi-idb` by default. Nonempty and NUL-free.
    pub fn vfs_name(mut self, name: &str) -> Self {
        self.0.vfs_name = name.into();
        self
    }

    /// IndexedDB database holding the files, `sqlite-wasm-vfs-jspi-idb` by default.
    pub fn database(mut self, name: &str) -> Self {
        self.0.database = name.into();
        self
    }

    /// Longest wait for a lock before `SQLITE_BUSY`, five seconds by default.
    pub fn lock_timeout(mut self, timeout: Duration) -> Self {
        self.0.lock_timeout = timeout;
        self
    }

    /// Longest IndexedDB operation before it aborts as an I/O error, thirty seconds by default.
    pub fn io_timeout(mut self, timeout: Duration) -> Self {
        self.0.io_timeout = timeout;
        self
    }

    pub fn build(self) -> JspiIdbCfg {
        self.0
    }
}

impl Default for JspiIdbCfgBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// VFS configuration, created with [`JspiIdbCfgBuilder`] or [`Self::default`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JspiIdbCfg {
    vfs_name: String,
    database: String,
    lock_timeout: Duration,
    io_timeout: Duration,
}

impl Default for JspiIdbCfg {
    fn default() -> Self {
        Self {
            vfs_name: "jspi-idb".into(),
            database: "sqlite-wasm-vfs-jspi-idb".into(),
            lock_timeout: Duration::from_secs(5),
            io_timeout: Duration::from_secs(30),
        }
    }
}

/// VFS errors. Match variants, not the human-readable display text.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum JspiIdbError {
    #[error(transparent)]
    Backend(#[from] VfsError),
    #[error(transparent)]
    Vfs(#[from] RegisterVfsError),
    #[error("JSPI is unavailable: WebAssembly.promising is missing")]
    JspiUnsupported,
    #[error("IndexedDB is unavailable in this context")]
    IndexedDbUnavailable,
    #[error("Web Locks are unavailable, and navigator.locks needs a secure context")]
    WebLocksUnavailable,
    #[error("{operation}: {message}")]
    Js {
        operation: &'static str,
        /// Browser error name and message.
        message: String,
        value: JsValue,
    },
    #[error("timed out waiting to {0}")]
    Timeout(&'static str),
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(&'static str),
}

impl JspiIdbError {
    fn js(operation: &'static str, value: JsValue) -> Self {
        Self::Js {
            operation,
            message: describe(&value),
            value,
        }
    }
}

/// Platform services that sleep by suspending inside [`run`].
struct JspiOs<C>(C);

impl<C: OsCallback> OsCallback for JspiOs<C> {
    fn sleep(&self, duration: Duration) {
        if !context::sleep(duration) {
            self.0.sleep(duration);
        }
    }

    fn random(&self, buf: &mut [u8]) -> usize {
        self.0.random(buf)
    }

    fn epoch_timestamp_in_ms(&self) -> VfsResult<i64> {
        self.0.epoch_timestamp_in_ms()
    }
}

/// State of one installed VFS, shared by its handles.
pub(crate) struct Inner {
    os: Box<dyn OsCallback>,
    pub(crate) idb: Idb,
    pub(crate) locks: LockManager,
    pub(crate) lock_prefix: String,
    pub(crate) lock_timeout: Duration,
    /// Files open in this context, by name.
    files: RefCell<HashMap<String, Rc<RefCell<FileState>>>>,
    last_error: RefCell<Option<VfsError>>,
    /// Closes the connection when another context deletes or upgrades the database.
    _on_version_change: Closure<dyn FnMut(Event)>,
}

fn cant_open(message: String) -> VfsError {
    VfsError::new(VfsErrorCode::CantOpen, message.into())
}

impl Inner {
    fn meta(&self, name: &str, code: VfsErrorCode) -> VfsResult<Option<Meta>> {
        self.idb
            .meta(name)
            .map_err(|err| js_vfs_error(code, "read file metadata", err))
    }

    fn open_file(self: &Rc<Self>, name: &str, options: OpenOptions) -> VfsResult<IdbFile> {
        context::check_entry()?;
        let exists = || cant_open(format!("file already exists: {name:?}"));
        let open = self.files.borrow().get(name).cloned();
        let state = match open {
            Some(state) if state.borrow().exists() => {
                if options.exclusive() {
                    return Err(exists());
                }
                state
            }
            open => {
                let meta = match self.meta(name, VfsErrorCode::CantOpen)? {
                    Some(_) if options.exclusive() => return Err(exists()),
                    Some(meta) => meta,
                    None if !options.create() => {
                        return Err(cant_open(format!("file not found: {name:?}")))
                    }
                    None => {
                        let created = self.idb.create(name).map_err(|err| {
                            js_vfs_error(VfsErrorCode::CantOpen, "create file", err)
                        })?;
                        if created {
                            Meta::default()
                        } else if options.exclusive() {
                            return Err(exists());
                        } else {
                            self.meta(name, VfsErrorCode::CantOpen)?
                                .ok_or_else(|| cant_open(format!("file vanished: {name:?}")))?
                        }
                    }
                };
                if let Some(state) = open {
                    state.borrow_mut().reset(Some(meta));
                    state
                } else {
                    let state = FileState::new(name, meta);
                    let state = Rc::new(RefCell::new(state));
                    self.files.borrow_mut().insert(name.into(), state.clone());
                    state
                }
            }
        };
        state.borrow_mut().handles += 1;
        Ok(IdbFile::new(
            self.clone(),
            state,
            options.access() == OpenAccess::ReadOnly,
        ))
    }

    fn close_file(&self, mut file: IdbFile) -> VfsResult<()> {
        let mut result = file.unlock(LockLevel::None);
        let last = {
            let mut state = file.state.borrow_mut();
            state.handles -= 1;
            state.handles == 0
        };
        if last {
            if result.is_ok() {
                result = context::check_entry()
                    .and_then(|()| file.flush(Durability::Default, VfsErrorCode::IoClose));
            }
            self.files
                .borrow_mut()
                .retain(|_, state| !Rc::ptr_eq(state, &file.state));
        }
        result
    }

    fn exists(&self, name: &str) -> VfsResult<bool> {
        if self
            .files
            .borrow()
            .get(name)
            .is_some_and(|state| state.borrow().exists())
        {
            return Ok(true);
        }
        Ok(self.meta(name, VfsErrorCode::IoAccess)?.is_some())
    }

    fn delete(&self, name: &str, durability: Durability) -> VfsResult<bool> {
        let existed = self
            .idb
            .delete(name, durability)
            .map_err(|err| js_vfs_error(VfsErrorCode::IoDelete, "delete file", err))?;
        if let Some(state) = self.files.borrow().get(name) {
            state.borrow_mut().reset(None);
        }
        Ok(existed)
    }
}

struct JspiIdbStore;

impl VfsStore for JspiIdbStore {
    type File = IdbFile;
    type AppData = Rc<Inner>;

    fn record_error(data: &Self::AppData, error: VfsError) {
        data.last_error.replace(Some(error));
    }

    fn last_error(data: &Self::AppData) -> Option<VfsError> {
        data.last_error.borrow().clone()
    }

    fn open_file(data: &Self::AppData, request: OpenRequest<'_>) -> VfsResult<OpenedFile<IdbFile>> {
        let options = request.options;
        let stored = matches!(
            options.kind(),
            Some(FileKind::MainDb | FileKind::MainJournal | FileKind::SuperJournal | FileKind::Wal)
        );
        let file = match request.filename {
            Some(name) if stored => data.open_file(name.path(), options)?,
            _ => IdbFile::new(
                data.clone(),
                Rc::new(RefCell::new(FileState::temporary())),
                false,
            ),
        };
        Ok(OpenedFile {
            file,
            access: options.access(),
        })
    }

    fn close_file(
        data: &Self::AppData,
        name: Option<&str>,
        file: IdbFile,
        options: OpenOptions,
    ) -> VfsResult<()> {
        if file.state.borrow().is_temporary() {
            return Ok(());
        }
        let closed = data.close_file(file);
        match name {
            Some(name) if options.delete_on_close() => {
                closed.and(Self::delete_file(data, name, false))
            }
            _ => closed,
        }
    }

    fn access(data: &Self::AppData, name: &str, _mode: AccessMode) -> VfsResult<bool> {
        context::check_entry()?;
        data.exists(name)
    }

    fn full_pathname(_data: &Self::AppData, name: &str) -> VfsResult<String> {
        Ok(name.into())
    }

    fn delete_file(data: &Self::AppData, name: &str, sync_dir: bool) -> VfsResult<()> {
        context::check_entry()?;
        let durability = if sync_dir {
            Durability::Strict
        } else {
            Durability::Default
        };
        if data.delete(name, durability)? {
            Ok(())
        } else {
            Err(VfsError::new(
                VfsErrorCode::IoDeleteNoEntry,
                format!("file not found: {name:?}").into(),
            ))
        }
    }
}

struct JspiIdbIoMethods;

impl SQLiteIoMethods for JspiIdbIoMethods {
    type Store = JspiIdbStore;

    /// Maps SQLite's atomic write batches onto one IndexedDB transaction.
    unsafe extern "C" fn xFileControl(
        file: *mut sqlite3_file,
        op: c_int,
        _arg: *mut c_void,
    ) -> c_int {
        if !matches!(
            op,
            SQLITE_FCNTL_BEGIN_ATOMIC_WRITE
                | SQLITE_FCNTL_COMMIT_ATOMIC_WRITE
                | SQLITE_FCNTL_ROLLBACK_ATOMIC_WRITE
        ) {
            return SQLITE_NOTFOUND;
        }
        // SAFETY: SQLite passes a live file opened by this VFS's xOpen, and
        // serializes callbacks on it, so no other borrow of the handle exists.
        let file = unsafe { &mut *SQLiteVfsFile::from_file(file) };
        // SAFETY: the file belongs to this VFS, whose app data is a live
        // `VfsAppData<Rc<Inner>>` for as long as files are open.
        let data = unsafe { VfsAppData::<Rc<Inner>>::get(file.vfs()) };
        // SAFETY: this VFS opens every file with an `IdbFile` handle.
        let handle = unsafe { file.handle_mut::<IdbFile>() };
        let result = match op {
            SQLITE_FCNTL_BEGIN_ATOMIC_WRITE => handle.begin_batch(),
            SQLITE_FCNTL_COMMIT_ATOMIC_WRITE => handle.commit_batch(),
            _ => {
                handle.rollback_batch();
                Ok(())
            }
        };
        match result {
            Ok(()) => SQLITE_OK,
            Err(error) => {
                let code = error.raw_code();
                JspiIdbStore::record_error(data, error);
                code
            }
        }
    }
}

struct JspiIdbVfs;

impl SQLiteVfs<JspiIdbIoMethods> for JspiIdbVfs {
    type Os = dyn OsCallback;

    fn os(data: &Rc<Inner>) -> &Self::Os {
        &*data.os
    }
}

fn validate(cfg: &JspiIdbCfg) -> Result<(), JspiIdbError> {
    let invalid = |reason| Err(JspiIdbError::InvalidConfiguration(reason));
    if cfg.vfs_name.is_empty() || cfg.vfs_name.contains('\0') {
        return invalid("the VFS name must be nonempty and NUL-free");
    }
    if cfg.database.is_empty() {
        return invalid("the IndexedDB database name must be nonempty");
    }
    if cfg.lock_timeout.is_zero() || cfg.io_timeout.is_zero() {
        return invalid("timeouts must be nonzero");
    }
    Ok(())
}

/// Installs the VFS. Needs no JSPI context.
///
/// `default_vfs = true` makes it SQLite's default VFS, and the registration
/// lasts for the lifetime of the module.
///
/// # Errors
///
/// An invalid `cfg`, a context without JSPI, IndexedDB or Web Locks, and
/// failures of IndexedDB or SQLite registration, a name already in use included.
pub async fn install<C: OsCallback + Default + 'static>(
    cfg: &JspiIdbCfg,
    default_vfs: bool,
) -> Result<(), JspiIdbError> {
    validate(cfg)?;
    context::check_jspi()?;
    let locks = Reflect::get(&js_sys::global(), &"navigator".into())
        .and_then(|navigator| Reflect::get(&navigator, &"locks".into()))
        .ok()
        .and_then(|locks| locks.dyn_into::<LockManager>().ok())
        .ok_or(JspiIdbError::WebLocksUnavailable)?;

    let db = idb::open(&cfg.database, cfg.io_timeout).await?;
    let on_version_change = Closure::<dyn FnMut(Event)>::new({
        let db = db.clone();
        move |_event| db.close()
    });
    db.set_onversionchange(Some(on_version_change.as_ref().unchecked_ref()));

    let inner = Rc::new(Inner {
        os: Box::new(JspiOs(C::default())),
        idb: Idb {
            db: db.clone(),
            bound: cfg.io_timeout,
        },
        locks,
        lock_prefix: format!("sqlite-wasm-vfs/jspi-idb/{}", cfg.database),
        lock_timeout: cfg.lock_timeout,
        files: RefCell::new(HashMap::new()),
        last_error: RefCell::new(None),
        _on_version_change: on_version_change,
    });

    // SAFETY: the default VFS constructor and file layout agree with
    // `JspiIdbIoMethods` and `JspiIdbStore`, whose app data is `Rc<Inner>`.
    // The registration is never freed, and every callback stays on this thread.
    let registration =
        unsafe { register_vfs::<JspiIdbIoMethods, JspiIdbVfs>(&cfg.vfs_name, inner, default_vfs) };
    match registration {
        Ok(registration) => {
            registration.into_raw();
        }
        Err(err) => {
            db.set_onversionchange(None);
            db.close();
            return Err(err.into());
        }
    }
    Ok(())
}
