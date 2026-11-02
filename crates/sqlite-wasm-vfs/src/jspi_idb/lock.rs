//! Web Locks held across JSPI suspensions.

use super::context::{self, WaitError};
use js_sys::{Array, Function, Object, Promise, Reflect};
use std::time::Duration;
use wasm_bindgen::{closure::Closure, prelude::wasm_bindgen, JsCast, JsValue};

#[wasm_bindgen]
extern "C" {
    /// `navigator.locks`. Bound here because web-sys marks it unstable.
    #[wasm_bindgen(js_name = LockManager)]
    pub(crate) type LockManager;

    #[wasm_bindgen(method, catch)]
    fn request(
        this: &LockManager,
        name: &str,
        options: &Object,
        callback: &Function,
    ) -> Result<Promise, JsValue>;
}

/// A granted Web Lock, released on drop.
pub(crate) struct HeldLock {
    release: Function,
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        // Resolving the callback's promise releases the lock.
        let _ = self.release.call0(&JsValue::NULL);
    }
}

/// Why a lock was not granted.
pub(crate) enum LockError {
    /// Another holder kept it past the bound.
    Busy,
    /// The Web Locks API failed.
    Js(JsValue),
}

fn abort_after(bound: Duration) -> Result<JsValue, JsValue> {
    let abort_signal = Reflect::get(&js_sys::global(), &"AbortSignal".into())?;
    let timeout = Reflect::get(&abort_signal, &"timeout".into())?.unchecked_into::<Function>();
    let millis = u32::try_from(bound.as_millis()).unwrap_or(u32::MAX);
    timeout.call1(&abort_signal, &millis.into())
}

/// Requests `name` exclusively, suspending at most `bound` until it is granted.
pub(crate) fn acquire(
    locks: &LockManager,
    name: &str,
    bound: Duration,
) -> Result<HeldLock, LockError> {
    let mut release = None;
    let held = Promise::new(&mut |resolve, _reject| release = Some(resolve));
    let release = release.expect("Promise executors run synchronously");

    let mut granted = None;
    let grant = Promise::new(&mut |resolve, _reject| granted = Some(resolve));
    let granted = granted.expect("Promise executors run synchronously");

    // Called once the lock is granted. The lock lasts until `held` settles.
    let callback = Closure::<dyn FnMut(JsValue) -> JsValue>::new({
        let held = held.clone();
        move |lock: JsValue| {
            let _ = granted.call1(&JsValue::NULL, &lock);
            held.clone().into()
        }
    });

    let options = Object::new();
    let signal = abort_after(bound).map_err(LockError::Js)?;
    Reflect::set(&options, &"signal".into(), &signal).map_err(LockError::Js)?;
    let request = locks
        .request(name, &options, callback.as_ref().unchecked_ref())
        .map_err(LockError::Js)?;
    // The extra second bounds only a browser that never delivers the abort.
    let outcome = context::wait(
        &Promise::race(&Array::of2(&grant, &request)),
        bound + Duration::from_secs(1),
    );

    let refused = |error| {
        // Releases a lock the browser grants after we stop waiting.
        let _ = release.call0(&JsValue::NULL);
        Err(error)
    };
    let result = match outcome {
        Ok(_) => Ok(HeldLock {
            release: release.clone(),
        }),
        Err(WaitError::TimedOut) => refused(LockError::Busy),
        Err(WaitError::Rejected(reason)) => {
            let name = Reflect::get(&reason, &"name".into())
                .ok()
                .and_then(|name| name.as_string());
            match name.as_deref() {
                Some("TimeoutError" | "AbortError") => refused(LockError::Busy),
                _ => refused(LockError::Js(reason)),
            }
        }
    };
    // The browser may still call the callback until the request settles.
    let keep_alive = Closure::once_into_js(move || drop(callback));
    let then = Reflect::get(&request, &"then".into())
        .expect("a Promise has a then method")
        .unchecked_into::<Function>();
    let _ = then.call2(&request, &keep_alive, &keep_alive);
    result
}
