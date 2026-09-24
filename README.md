[![Crates.io](https://img.shields.io/crates/v/sqlite-wasm-rs.svg)](https://crates.io/crates/sqlite-wasm-rs)

`wasm32-unknown-unknown` bindings to the libsqlite3 library.

## Usage

```toml
[dependencies]
sqlite-wasm-rs = { version = "0.6", features = ["wasm-bindgen"] }
```

```toml
[dependencies]
# Encryption is supported by SQLite3MultipleCiphers
# See <https://utelle.github.io/SQLite3MultipleCiphers>
sqlite-wasm-rs = { version = "0.6", features = ["wasm-bindgen", "sqlite3mc"] }
```

```rust
use sqlite_wasm_rs as ffi;

fn open_db() {
    // open with memory vfs
    let mut db = std::ptr::null_mut();
    let ret = unsafe {
        ffi::sqlite3_open_v2(
            c"mem.db".as_ptr().cast(),
            &mut db as *mut _,
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
            std::ptr::null()
        )
    };
    assert_eq!(ffi::SQLITE_OK, ret);
    assert_eq!(unsafe { ffi::sqlite3_close(db) }, ffi::SQLITE_OK);
}
```

## About VFS

```toml
[dependencies]
sqlite-wasm-vfs = { version = "0.3", features = ["sahpool"] }
```

The following vfs have been implemented:

* [`memory`](./crates/rsqlite-vfs/src/memvfs.rs): as the default vfs, no additional conditions are required, store the database in memory.
* [`sahpool`](./crates/sqlite-wasm-vfs/src/sahpool.rs): ported from sqlite-wasm, store the database in opfs.

### How to implement a VFS

Here is an example showing how to implement a simple in-memory VFS, see [`implement-a-vfs`](./crates/rsqlite-vfs/examples/implement-a-vfs.rs) example.

```sh
cargo run -p rsqlite-vfs --example implement-a-vfs
```

## About multithreading

By default SQLite is compiled with `-DSQLITE_THREADSAFE=0`, so all SQLite calls must stay on one thread.

The `threadsafe` feature compiles SQLite with `-DSQLITE_THREADSAFE=1` and a mutex built on Wasm atomics, and makes the memory VFS shareable between workers. Workers that share one module and its memory need a nightly shared-memory build:

```sh
CFLAGS_wasm32_unknown_unknown=-matomics \
RUSTFLAGS="-Ctarget-feature=+atomics -Clink-args=--shared-memory -Clink-args=--import-memory \
  -Clink-args=--max-memory=1073741824 -Clink-args=--export=__wasm_init_tls -Clink-args=--export=__tls_size \
  -Clink-args=--export=__tls_align -Clink-args=--export=__tls_base -Clink-args=--export=__heap_base" \
cargo +nightly build --target wasm32-unknown-unknown -Z build-std=panic_abort,std --features wasm-bindgen,threadsafe
```

* In browsers, serve the page with `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`, so `crossOriginIsolated` is true. Without them, handing the shared memory to a worker fails with a `DataCloneError`. Node and Bun need no headers.
* Make the first SQLite call on one thread before other workers use SQLite.
* The browser main thread never waits. Mutexes spin there and `sqlite3_sleep` returns at once.
* An OPFS `sahpool` stays with the worker that installed it, and other workers get `SQLITE_MISUSE`.
* A worker terminated inside a SQLite call leaves its mutexes locked.

## Use without wasm-bindgen

No features are enabled by default, provide your own host functions. See [JS Host](./examples/host-js) or [C Host](./examples/host-c) example.

## Use custom SQLite sources

Point `SQLITE_WASM_RS_SOURCE_DIR` to your `sqlite3.c/.h` files (`sqlite3mc_amalgamation.c/.h` for `sqlite3mc`):

```sh
SQLITE_WASM_RS_SOURCE_DIR=/path/to/sqlite cargo build --target wasm32-unknown-unknown --features bindgen
```

## Minimum supported Rust version (MSRV)

The minimal officially supported rustc version is 1.81.0.

## Extensions

|Extension|About|
|-|-|
|[sqlite-vec](./extensions/sqlite-vec)|A vector search SQLite extension that runs anywhere!|

Contributions are welcome!

## Related Project

* [`diesel`](https://github.com/diesel-rs/diesel): A safe, extensible ORM and Query Builder for Rust.
* [`rusqlite`](https://github.com/rusqlite/rusqlite): Ergonomic bindings to SQLite for Rust.
* [`sqlite-wasm`](https://github.com/sqlite/sqlite-wasm): SQLite Wasm conveniently wrapped as an ES Module.
* [`sqlite-web-rs`](https://github.com/xmtp/sqlite-web-rs): A SQLite WebAssembly backend for Diesel.
* [`wa-sqlite`](https://github.com/rhashimoto/wa-sqlite): WebAssembly SQLite with support for browser storage extensions.
* [`SQLite3MultipleCiphers`](https://github.com/utelle/SQLite3MultipleCiphers): SQLite3 encryption extension with support for multiple ciphers.

## Friends

- [moli](https://github.com/lexmount/moli) - Best browser for AI Agent, written in pure Rust.
