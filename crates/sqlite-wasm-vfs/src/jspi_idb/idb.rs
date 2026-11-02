//! Store `files` maps a name to `{ size, generation }`, and store `blocks` maps
//! `[name, index]` to `BLOCK_SIZE` bytes. Every operation is one transaction.

use super::context::{self, WaitError};
use super::{JspiIdbError, BLOCK_SIZE};
use js_sys::{Array, Object, Promise, Reflect, Uint8Array};
use std::time::Duration;
use wasm_bindgen::{closure::Closure, prelude::wasm_bindgen, JsCast, JsValue};
use web_sys::{
    Event, IdbDatabase, IdbFactory, IdbKeyRange, IdbOpenDbRequest, IdbRequest, IdbTransaction,
    IdbTransactionMode,
};

#[wasm_bindgen]
extern "C" {
    /// `IDBDatabase` with the `transaction` options web-sys marks unstable.
    #[wasm_bindgen(js_name = IDBDatabase)]
    type DatabaseWithOptions;

    #[wasm_bindgen(method, catch, js_name = transaction)]
    fn transaction_with_options(
        this: &DatabaseWithOptions,
        stores: &Array,
        mode: IdbTransactionMode,
        options: &Object,
    ) -> Result<IdbTransaction, JsValue>;
}

const FILES: &str = "files";
const BLOCKS: &str = "blocks";

/// Largest integer a JavaScript number holds exactly.
pub(crate) const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Persisted metadata of one file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Meta {
    pub(crate) size: u64,
    /// Incremented by every flush, so other contexts can tell their data is stale.
    pub(crate) generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Durability {
    /// Complete once on persistent storage, where the browser honours it.
    Strict,
    Default,
}

fn number(value: u64) -> JsValue {
    debug_assert!(
        value <= MAX_SAFE_INTEGER,
        "{value} is not exact as a JS number"
    );
    // Exact below 2^53, which every caller's bound guarantees.
    JsValue::from_f64(value as f64)
}

fn integer(value: &JsValue) -> Option<u64> {
    let value = value.as_f64()?;
    // Only exact non-negative integers pass, so the cast is lossless.
    (value.fract() == 0.0 && (0.0..=MAX_SAFE_INTEGER as f64).contains(&value))
        .then_some(value as u64)
}

fn block_key(name: &str, index: u64) -> JsValue {
    Array::of2(&JsValue::from_str(name), &number(index)).into()
}

/// Keys of every block of `name` from index `from` on.
fn block_range(name: &str, from: u64) -> Result<JsValue, JsValue> {
    let upper = Array::of2(&JsValue::from_str(name), &JsValue::from_f64(f64::INFINITY));
    IdbKeyRange::bound(&block_key(name, from), &upper).map(JsValue::from)
}

fn meta_value(meta: Meta) -> JsValue {
    let object = Object::new();
    let _ = Reflect::set(&object, &"size".into(), &number(meta.size));
    let _ = Reflect::set(&object, &"generation".into(), &number(meta.generation));
    object.into()
}

fn parse_meta(value: JsValue) -> Result<Meta, JsValue> {
    let field = |name: &str| {
        Reflect::get(&value, &name.into())
            .ok()
            .as_ref()
            .and_then(integer)
    };
    match (field("size"), field("generation")) {
        (Some(size), Some(generation)) => Ok(Meta { size, generation }),
        _ => Err(JsValue::from_str("malformed file metadata in IndexedDB")),
    }
}

fn parse_block(value: JsValue) -> Result<Box<[u8]>, JsValue> {
    let bytes = value
        .dyn_into::<Uint8Array>()
        .map_err(|_| JsValue::from_str("malformed block in IndexedDB"))?
        .to_vec();
    if bytes.len() != BLOCK_SIZE {
        return Err(JsValue::from_str("block of the wrong size in IndexedDB"));
    }
    Ok(bytes.into_boxed_slice())
}

/// Opens or creates the database during installation.
pub(crate) async fn open(name: &str, bound: Duration) -> Result<IdbDatabase, JspiIdbError> {
    const OPERATION: &str = "open the IndexedDB database";
    let factory = Reflect::get(&js_sys::global(), &"indexedDB".into())
        .ok()
        .and_then(|factory| factory.dyn_into::<IdbFactory>().ok())
        .ok_or(JspiIdbError::IndexedDbUnavailable)?;
    let request: IdbOpenDbRequest = factory
        .open_with_u32(name, 1)
        .map_err(|err| JspiIdbError::js(OPERATION, err))?;

    let upgrade = Closure::<dyn FnMut(Event)>::new({
        let request = request.clone();
        move |_event| {
            let Ok(db) = request.result() else { return };
            let db = db.unchecked_into::<IdbDatabase>();
            for store in [FILES, BLOCKS] {
                // A failure aborts the upgrade and surfaces as the open error.
                let _ = db.create_object_store(store);
            }
        }
    });
    request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
    let opened = Promise::new(&mut |resolve, reject| {
        request.set_onsuccess(Some(&resolve));
        request.set_onerror(Some(&reject));
    });

    let outcome = context::wait_async(&opened, bound).await;
    request.set_onupgradeneeded(None);
    match outcome {
        Ok(_) => {}
        Err(WaitError::TimedOut) => return Err(JspiIdbError::Timeout(OPERATION)),
        Err(WaitError::Rejected(_)) => {
            let error = request.error().ok().flatten().map(JsValue::from);
            return Err(JspiIdbError::js(OPERATION, error.unwrap_or_default()));
        }
    }
    Ok(request
        .result()
        .map_err(|err| JspiIdbError::js(OPERATION, err))?
        .unchecked_into())
}

/// Issues requests on a read-write transaction, aborting it if any request
/// cannot be issued, so a partial set never commits.
fn issue<T>(
    tx: &IdbTransaction,
    requests: impl FnOnce() -> Result<T, JsValue>,
) -> Result<T, JsValue> {
    requests().inspect_err(|_| {
        let _ = tx.abort();
    })
}

/// One installed database and the bound on each of its transactions.
pub(crate) struct Idb {
    pub(crate) db: IdbDatabase,
    pub(crate) bound: Duration,
}

impl Idb {
    fn transaction(
        &self,
        stores: &[&str],
        mode: IdbTransactionMode,
        durability: Durability,
    ) -> Result<IdbTransaction, JsValue> {
        let names = stores
            .iter()
            .map(|store| JsValue::from_str(store))
            .collect::<Array>();
        let options = Object::new();
        let durability = match durability {
            Durability::Strict => "strict",
            Durability::Default => "default",
        };
        Reflect::set(&options, &"durability".into(), &durability.into())?;
        self.db
            .unchecked_ref::<DatabaseWithOptions>()
            .transaction_with_options(&names, mode, &options)
    }

    /// Suspends until `tx` commits, aborting it if the bound elapses first.
    fn commit(&self, tx: &IdbTransaction) -> Result<(), JsValue> {
        let done = Promise::new(&mut |resolve, reject| {
            tx.set_oncomplete(Some(&resolve));
            tx.set_onabort(Some(&reject));
        });
        match context::wait(&done, self.bound) {
            Ok(_) => Ok(()),
            Err(WaitError::Rejected(reason)) => {
                Err(tx.error().map(JsValue::from).unwrap_or(reason))
            }
            Err(WaitError::TimedOut) => {
                // Abort throws if the transaction already committed, which is then durable.
                let _ = tx.abort();
                Err(JsValue::from_str("IndexedDB transaction timed out"))
            }
        }
    }

    /// Reads `keys` of `store` in one transaction, `None` where a key is absent.
    fn get_many<T>(
        &self,
        store: &str,
        keys: impl Iterator<Item = JsValue>,
        parse: impl Fn(JsValue) -> Result<T, JsValue>,
    ) -> Result<Vec<Option<T>>, JsValue> {
        let tx = self.transaction(&[store], IdbTransactionMode::Readonly, Durability::Default)?;
        let object_store = tx.object_store(store)?;
        let requests = keys
            .map(|key| object_store.get(&key))
            .collect::<Result<Vec<IdbRequest>, _>>()?;
        self.commit(&tx)?;
        requests
            .iter()
            .map(|request| {
                let value = request.result()?;
                (!value.is_undefined()).then(|| parse(value)).transpose()
            })
            .collect()
    }

    pub(crate) fn meta(&self, name: &str) -> Result<Option<Meta>, JsValue> {
        Ok(self.metas(&[name])?.pop().flatten())
    }

    pub(crate) fn metas(&self, names: &[&str]) -> Result<Vec<Option<Meta>>, JsValue> {
        self.get_many(FILES, names.iter().map(|&name| name.into()), parse_meta)
    }

    pub(crate) fn blocks(
        &self,
        name: &str,
        indices: &[u64],
    ) -> Result<Vec<Option<Box<[u8]>>>, JsValue> {
        self.get_many(
            BLOCKS,
            indices.iter().map(|&index| block_key(name, index)),
            parse_block,
        )
    }

    /// Creates an empty file, returning `false` if it already exists.
    pub(crate) fn create(&self, name: &str) -> Result<bool, JsValue> {
        let tx = self.transaction(&[FILES], IdbTransactionMode::Readwrite, Durability::Default)?;
        issue(&tx, || {
            tx.object_store(FILES)?
                .add_with_key(&meta_value(Meta::default()), &name.into())
        })?;
        match self.commit(&tx) {
            Ok(()) => Ok(true),
            Err(error)
                if Reflect::get(&error, &"name".into())
                    .ok()
                    .and_then(|n| n.as_string())
                    .as_deref()
                    == Some("ConstraintError") =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// Stores `blocks`, deletes every other block from `truncate_from` on, and
    /// records `meta`, all in one transaction.
    pub(crate) fn flush(
        &self,
        name: &str,
        blocks: Vec<(u64, Uint8Array)>,
        truncate_from: Option<u64>,
        meta: Meta,
        durability: Durability,
    ) -> Result<(), JsValue> {
        let tx = self.transaction(&[FILES, BLOCKS], IdbTransactionMode::Readwrite, durability)?;
        issue(&tx, || {
            let store = tx.object_store(BLOCKS)?;
            if let Some(from) = truncate_from {
                store.delete(&block_range(name, from)?)?;
            }
            for (index, bytes) in &blocks {
                store.put_with_key(bytes, &block_key(name, *index))?;
            }
            tx.object_store(FILES)?
                .put_with_key(&meta_value(meta), &name.into())
        })?;
        self.commit(&tx)
    }

    /// Deletes a file and its blocks, returning whether it existed.
    pub(crate) fn delete(&self, name: &str, durability: Durability) -> Result<bool, JsValue> {
        let tx = self.transaction(&[FILES, BLOCKS], IdbTransactionMode::Readwrite, durability)?;
        let existing = issue(&tx, || {
            let files = tx.object_store(FILES)?;
            let existing = files.get(&name.into())?;
            files.delete(&name.into())?;
            tx.object_store(BLOCKS)?.delete(&block_range(name, 0)?)?;
            Ok(existing)
        })?;
        self.commit(&tx)?;
        Ok(!existing.result()?.is_undefined())
    }

    pub(crate) fn names(&self) -> Result<Vec<String>, JsValue> {
        let tx = self.transaction(&[FILES], IdbTransactionMode::Readonly, Durability::Default)?;
        let request = tx.object_store(FILES)?.get_all_keys()?;
        self.commit(&tx)?;
        Array::from(&request.result()?)
            .iter()
            .map(|key| {
                key.as_string()
                    .ok_or_else(|| JsValue::from_str("malformed file name in IndexedDB"))
            })
            .collect()
    }

    pub(crate) fn clear(&self) -> Result<(), JsValue> {
        let tx = self.transaction(
            &[FILES, BLOCKS],
            IdbTransactionMode::Readwrite,
            Durability::Strict,
        )?;
        issue(&tx, || {
            tx.object_store(FILES)?.clear()?;
            tx.object_store(BLOCKS)?.clear()
        })?;
        self.commit(&tx)
    }
}
