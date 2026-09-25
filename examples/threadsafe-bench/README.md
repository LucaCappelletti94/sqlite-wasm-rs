# threadsafe-bench

Measures how SQLite scales across workers that share one module and its memory, with the `threadsafe` feature.

```sh
./build.sh                        # pkg/threadsafe, pkg/single (SQLITE_THREADSAFE=0), pkg/cipher (SQLite3MC), pkg/tcache
node run-native.mjs               # Node worker_threads
bun run-native.mjs                # Bun Web Workers
node run-browser.mjs chrome       # also firefox or webkit (Playwright's WebKit build)
python3 analyze.py                # results/summary.csv, results/report.md and SVG figures
```

`build.sh` needs nightly with `rust-src` and a `wasm-bindgen` CLI matching the crate version (set `WASM_BINDGEN` to its path).

Each point runs a fixed number of operations split evenly across 1 to 32 workers, released together by a start line in shared memory, and reports the span from the first start to the last end. Every point runs one warm-up round and seven measured rounds.

Runner options:

- `--variants=threadsafe,cipher,single,tcache` picks builds. `tcache` swaps in per-thread allocation caches to measure the global allocator lock.
- `--plan=0,1,2,3,4,5,0n,2n` picks modalities: shared memvfs, memvfs per worker, `:memory:` per worker, OPFS pool per worker, and the two SQLite3MC ones. A trailing `n` selects NOMUTEX connections.
- `--workloads=`, `--counts=`, `--rounds=` narrow the matrix, and `--tag=` names the output file of a partial run.
- `--no-memstatus` (Node and Bun) turns SQLite's memory statistics off.
- `--reserve-mb=` (browsers) grows the heap once before workers start. Playwright's WebKit build froze workers while shared memory grew, and 1024 avoided it.
- `--stress=ROUNDS` (browsers) hammers one SQLite mutex from 32 workers and fails on a lost increment.
- `--quick` runs a small matrix to check the setup.
