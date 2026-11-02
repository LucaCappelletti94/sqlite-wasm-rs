# sqlite-wasm-vfs

SQLite VFS implementations for `wasm32-unknown-unknown`.

`sahpool` stores databases in OPFS using sync access handles. `jspi-idb` stores them in IndexedDB on engines with JS Promise Integration, including Firefox private windows where OPFS is refused.

```toml
[dependencies]
sqlite-wasm-rs = { version = "0.6", features = ["wasm-bindgen"] }
sqlite-wasm-vfs = { version = "0.3", features = ["sahpool"] }
```

Install it as SQLite's default VFS:

```rust
use sqlite_wasm_rs::WasmOsCallback;
use sqlite_wasm_vfs::sahpool::{install, OpfsSAHPoolCfg};

async fn install_vfs() {
    install::<WasmOsCallback>(&OpfsSAHPoolCfg::default(), true)
        .await
        .unwrap();
}
```

Requires a secure context and a dedicated worker. Concurrent connections to the same database and WAL are not supported.

With `jspi-idb`, every SQLite call that can reach the VFS runs inside `jspi_idb::run`:

```rust
use sqlite_wasm_rs::{self as ffi, WasmOsCallback};
use sqlite_wasm_vfs::jspi_idb::{install, run, JspiIdbCfg};

async fn open_db() {
    install::<WasmOsCallback>(&JspiIdbCfg::default(), true)
        .await
        .unwrap();
    run(|| unsafe {
        let mut db = std::ptr::null_mut();
        assert_eq!(ffi::sqlite3_open(c"app.db".as_ptr(), &mut db), ffi::SQLITE_OK);
        assert_eq!(ffi::sqlite3_close(db), ffi::SQLITE_OK);
    })
    .await
    .unwrap();
}
```

A commit is on disk once SQLite reports it, and connections in several windows and workers can share a database. A module built with `jspi-idb` does not instantiate on engines without JSPI, so ship a second build without it as the fallback.

[API documentation](https://docs.rs/sqlite-wasm-vfs)
