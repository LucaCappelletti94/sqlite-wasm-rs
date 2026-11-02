//! The JSPI stack SQLite runs on, and the only place the VFS suspends.

use super::JspiIdbError;
use js_sys::{futures::JsFuture, Array, Function, Object, Promise, Reflect};
use rsqlite_vfs::{VfsError, VfsErrorCode, VfsResult};
use std::{
    cell::{Cell, OnceCell, RefCell},
    rc::Rc,
    time::Duration,
};
use wasm_bindgen::{JsCast, JsValue};

const ENTRY: &str = "__sqlite_wasm_vfs_jspi_idb_run";

type Task = Box<dyn FnOnce()>;

thread_local! {
    /// A `run` closure is on the stack, running or suspended.
    static ENTERED: Cell<bool> = const { Cell::new(false) };
    /// The running closure is suspended while other code executes.
    static SUSPENDED: Cell<bool> = const { Cell::new(false) };
    /// Settles once every queued run has finished.
    static QUEUE: RefCell<Option<Promise>> = const { RefCell::new(None) };
    static PROMISING_ENTRY: OnceCell<Function> = const { OnceCell::new() };
}

/// Keeps the export alone, so wasm-bindgen's experimental-JSPI deprecation is silenced nowhere else.
mod entry {
    // reason: wasm-bindgen marks its `jspi` attribute deprecated as experimental.
    #![allow(deprecated)]

    use super::{Task, ENTERED};
    use wasm_bindgen::prelude::wasm_bindgen;

    /// Runs the `Box<Task>` that `run` leaked at address `task`.
    #[wasm_bindgen(jspi, js_name = "__sqlite_wasm_vfs_jspi_idb_run", skip_typescript)]
    pub fn jspi_idb_run(task: usize) {
        // SAFETY: `run` passes the address of a `Box<Task>` it leaked and never
        // touches again, and the queue enters this export once per address.
        let task = unsafe { Box::from_raw(task as *mut Task) };
        ENTERED.set(true);
        task();
        ENTERED.set(false);
    }
}

fn promising_entry() -> Result<Function, JspiIdbError> {
    PROMISING_ENTRY.with(|cell| {
        if let Some(entry) = cell.get() {
            return Ok(entry.clone());
        }
        let promising = Reflect::get(&js_sys::global(), &"WebAssembly".into())
            .and_then(|wasm| Reflect::get(&wasm, &"promising".into()))
            .ok()
            .and_then(|value| value.dyn_into::<Function>().ok())
            .ok_or(JspiIdbError::JspiUnsupported)?;
        let export = Reflect::get(&wasm_bindgen::exports(), &ENTRY.into())
            .map_err(|err| JspiIdbError::js("look up the JSPI entry export", err))?;
        let entry = promising
            .call1(&JsValue::NULL, &export)
            .map_err(|err| JspiIdbError::js("wrap the JSPI entry export", err))?
            .unchecked_into::<Function>();
        Ok(cell.get_or_init(|| entry).clone())
    })
}

/// Fails unless the engine can enter [`run`].
pub(crate) fn check_jspi() -> Result<(), JspiIdbError> {
    promising_entry().map(drop)
}

/// Runs `f` where SQLite may use this VFS, after every earlier run finishes.
///
/// Every SQLite call that can reach the VFS, opening and closing connections
/// included, must happen inside `f`. Calls made elsewhere, or while `f` waits
/// on IndexedDB or a lock, fail with `SQLITE_MISUSE`. Dropping the future does
/// not cancel `f`.
///
/// # Errors
///
/// [`JspiIdbError::JspiUnsupported`] without `WebAssembly.promising`, and
/// [`JspiIdbError::Js`] if the JSPI stack fails, a panic inside `f` included.
pub async fn run<R: 'static>(f: impl FnOnce() -> R + 'static) -> Result<R, JspiIdbError> {
    let entry = promising_entry()?;
    let output = Rc::new(RefCell::new(None));
    let task: Task = Box::new({
        let output = output.clone();
        move || *output.borrow_mut() = Some(f())
    });
    // Pointer to integer for the JS call, which `jspi_idb_run` casts back.
    let task = Box::into_raw(Box::new(task)) as usize;
    let start = entry.bind1(&JsValue::NULL, &JsValue::from(task));

    let previous = QUEUE.with_borrow_mut(|queue| {
        queue
            .take()
            .unwrap_or_else(|| Promise::resolve(&JsValue::UNDEFINED))
    });
    let then = Reflect::get(&previous, &"then".into())
        .expect("a Promise has a then method")
        .unchecked_into::<Function>();
    let fiber: Promise = then
        .call1(&previous, &start)
        .expect("Promise.prototype.then does not throw")
        .unchecked_into();
    let settled = Promise::all_settled(&Array::of1(&fiber));
    QUEUE.with_borrow_mut(|queue| *queue = Some(settled));

    JsFuture::from(fiber)
        .await
        .map_err(|err| JspiIdbError::js("run on the JSPI stack", err))?;
    let result = output.borrow_mut().take();
    Ok(result.expect("a fulfilled run stored its output"))
}

/// Refuses VFS work outside a running [`run`] closure.
pub(crate) fn check_entry() -> VfsResult<()> {
    let message = if SUSPENDED.get() {
        "SQLite was entered while a jspi_idb::run closure waited. Run all SQLite calls through jspi_idb::run"
    } else if !ENTERED.get() {
        "the jspi-idb VFS is only usable inside sqlite_wasm_vfs::jspi_idb::run"
    } else {
        return Ok(());
    };
    Err(VfsError::new(VfsErrorCode::Misuse, message.into()))
}

/// How a bounded wait ended without fulfilment.
pub(crate) enum WaitError {
    Rejected(JsValue),
    TimedOut,
}

fn global_fn(name: &str) -> Function {
    Reflect::get(&js_sys::global(), &name.into())
        .expect("timers exist in windows and workers")
        .unchecked_into()
}

/// A race between a promise and a timer that resolves with a private marker.
struct Bounded {
    race: Promise,
    marker: JsValue,
    timer: JsValue,
}

impl Bounded {
    fn new(promise: &Promise, bound: Duration) -> Self {
        let marker = JsValue::from(Object::new());
        let delay = JsValue::from(u32::try_from(bound.as_millis()).unwrap_or(u32::MAX));
        let mut timer = JsValue::UNDEFINED;
        let timeout = Promise::new(&mut |resolve, _reject| {
            timer = global_fn("setTimeout")
                .call3(&JsValue::NULL, &resolve, &delay, &marker)
                .expect("setTimeout accepts a function");
        });
        let race = Promise::race(&Array::of2(promise, &timeout));
        Self {
            race,
            marker,
            timer,
        }
    }

    fn finish(self, settled: Result<JsValue, JsValue>) -> Result<JsValue, WaitError> {
        let _ = global_fn("clearTimeout").call1(&JsValue::NULL, &self.timer);
        match settled {
            Ok(value) if value == self.marker => Err(WaitError::TimedOut),
            Ok(value) => Ok(value),
            Err(reason) => Err(WaitError::Rejected(reason)),
        }
    }
}

/// Suspends the running closure until `promise` settles or `bound` elapses.
/// Callers must have passed [`check_entry`] in the same VFS call.
// reason: wasm-bindgen marks `jspi_block_on_promise` deprecated as experimental.
#[allow(deprecated)]
pub(crate) fn wait(promise: &Promise, bound: Duration) -> Result<JsValue, WaitError> {
    debug_assert!(ENTERED.get() && !SUSPENDED.get(), "wait outside a run");
    let bounded = Bounded::new(promise, bound);
    SUSPENDED.set(true);
    let settled = js_sys::futures::jspi_block_on_promise(&bounded.race);
    SUSPENDED.set(false);
    bounded.finish(settled)
}

/// Awaits `promise` from ordinary async code, for at most `bound`.
pub(crate) async fn wait_async(promise: &Promise, bound: Duration) -> Result<JsValue, WaitError> {
    let bounded = Bounded::new(promise, bound);
    let settled = JsFuture::from(bounded.race.clone()).await;
    bounded.finish(settled)
}

/// Suspends for `duration`, or returns `false` where suspending is not allowed.
pub(crate) fn sleep(duration: Duration) -> bool {
    if check_entry().is_err() {
        return false;
    }
    let _ = wait(&Promise::new(&mut |_, _| {}), duration);
    true
}
