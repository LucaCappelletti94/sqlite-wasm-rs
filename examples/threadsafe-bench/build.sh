#!/usr/bin/env bash
# Builds the benchmark variants into pkg/<variant>, all on shared memory.
set -euo pipefail
cd "$(dirname "$0")"

export CFLAGS_wasm32_unknown_unknown="-matomics -mbulk-memory"
export RUSTFLAGS="-Ctarget-feature=+atomics -Clink-args=--shared-memory \
  -Clink-args=--max-memory=4294967296 -Clink-args=--import-memory \
  -Clink-args=--export=__wasm_init_tls -Clink-args=--export=__tls_size \
  -Clink-args=--export=__tls_align -Clink-args=--export=__tls_base -Clink-args=--export=__heap_base"
# The CLI must match the wasm-bindgen crate version in Cargo.lock.
WASM_BINDGEN=${WASM_BINDGEN:-wasm-bindgen}

# threadsafe: SQLITE_THREADSAFE=1, single: the default SQLITE_THREADSAFE=0, cipher: threadsafe with SQLite3MC,
# sqlcipher: threadsafe with SQLCipher, tcache and sqlcipher-tcache: per-thread allocation caches, to measure
# the global allocator lock.
for variant in threadsafe:threadsafe single: cipher:threadsafe,sqlite3mc tcache:threadsafe,thread-cache-alloc \
  sqlcipher:threadsafe,sqlcipher sqlcipher-tcache:threadsafe,sqlcipher,thread-cache-alloc; do
  name=${variant%%:*}
  features=${variant#*:}
  cargo +nightly build --release --target wasm32-unknown-unknown -Z build-std=panic_abort,std \
    ${features:+--features "$features"} --target-dir "target/$name"
  "$WASM_BINDGEN" --target web --out-dir "pkg/$name" \
    "target/$name/wasm32-unknown-unknown/release/threadsafe_bench.wasm"
done
