//! Workloads that measure how SQLite scales across workers sharing one module and its memory.
//!
//! `orchestrator.mjs` drives these exports from every worker of a pool.
#![cfg_attr(feature = "thread-cache-alloc", feature(thread_local))]

use sqlite_wasm_rs::*;

#[cfg(feature = "thread-cache-alloc")]
mod thread_cache;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicU32, Ordering};
use wasm_bindgen::prelude::*;

const ROWS: i64 = 100_000;
const RANGE: i64 = 1_000;
const SORT_RANGE: i64 = 2_000;
const VOCABULARY: i64 = 1_024;
const INSERT_BATCH: usize = 50;
const KEY: &CStr = c"PRAGMA key = 'threadsafe-bench';";

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(message: &str);
}

/// Sends panic messages to the console, since a panicking worker otherwise only reports `unreachable`.
fn report_panics() {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| std::panic::set_hook(Box::new(|info| console_error(&info.to_string()))));
}

#[wasm_bindgen(
    inline_js = "export function now() { return performance.timeOrigin + performance.now(); }"
)]
extern "C" {
    /// Milliseconds on a clock every worker shares, unlike `performance.now()` alone.
    fn now() -> f64;
}

/// The shared memory, handed to every worker the orchestrator starts.
#[wasm_bindgen]
pub fn shared_memory() -> JsValue {
    wasm_bindgen::memory()
}

/// Turns SQLite's memory statistics off before any worker initializes SQLite.
#[wasm_bindgen]
pub fn disable_memstatus() {
    // SAFETY: called on the orchestrating thread before any SQLite call, as `sqlite3_config` requires.
    let rc = unsafe { sqlite3_config(SQLITE_CONFIG_MEMSTATUS, 0) };
    assert_eq!(rc, SQLITE_OK);
}

#[wasm_bindgen]
pub fn sqlite_threadsafe() -> i32 {
    // SAFETY: reading the compile-time threading mode has no precondition.
    unsafe { sqlite3_threadsafe() }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Modality {
    MemvfsShared,
    MemvfsPerWorker,
    MemoryPerWorker,
    SahpoolPerWorker,
    CipherShared,
    CipherPerWorker,
    /// SQLCipher, keyed through the codec on plain memvfs, with its default 256 000 KDF iterations.
    SqlcipherShared,
}

impl Modality {
    fn from_code(code: u32) -> Self {
        match code {
            0 => Self::MemvfsShared,
            1 => Self::MemvfsPerWorker,
            2 => Self::MemoryPerWorker,
            3 => Self::SahpoolPerWorker,
            4 => Self::CipherShared,
            5 => Self::CipherPerWorker,
            6 => Self::SqlcipherShared,
            _ => panic!("unknown modality {code}"),
        }
    }

    fn filename(self, worker: u32) -> String {
        match self {
            Self::MemvfsShared => "shared.db".into(),
            Self::MemvfsPerWorker => format!("worker-{worker}.db"),
            Self::MemoryPerWorker => ":memory:".into(),
            Self::SahpoolPerWorker => format!("worker-{worker}.db"),
            // The cipher wrapper around memvfs holds the key, and a URI selects it.
            Self::CipherShared => "file:cipher-shared.db?vfs=multipleciphers-memvfs".into(),
            Self::CipherPerWorker => format!("file:cipher-{worker}.db?vfs=multipleciphers-memvfs"),
            Self::SqlcipherShared => "sqlcipher.db".into(),
        }
    }

    fn vfs(self, worker: u32) -> Option<CString> {
        match self {
            Self::MemvfsShared | Self::MemvfsPerWorker | Self::SqlcipherShared => {
                Some(c"memvfs".into())
            }
            Self::SahpoolPerWorker => Some(CString::new(sahpool_name(worker)).unwrap()),
            Self::MemoryPerWorker | Self::CipherShared | Self::CipherPerWorker => None,
        }
    }

    fn shared(self) -> bool {
        matches!(
            self,
            Self::MemvfsShared | Self::CipherShared | Self::SqlcipherShared
        )
    }

    fn encrypted(self) -> bool {
        matches!(
            self,
            Self::CipherShared | Self::CipherPerWorker | Self::SqlcipherShared
        )
    }

    /// Per-worker memvfs files, removed at teardown so memory does not pile up across modalities.
    fn memvfs_file(self, worker: u32) -> Option<String> {
        match self {
            Self::MemvfsPerWorker => Some(format!("worker-{worker}.db")),
            Self::CipherPerWorker => Some(format!("cipher-{worker}.db")),
            _ => None,
        }
    }
}

fn sahpool_name(worker: u32) -> String {
    format!("bench-sahpool-{worker}")
}

struct Connection {
    db: *mut sqlite3,
}

impl Connection {
    fn open(modality: Modality, worker: u32, nomutex: bool) -> Self {
        let name = CString::new(modality.filename(worker)).unwrap();
        let vfs = modality.vfs(worker);
        let mutex = if nomutex {
            SQLITE_OPEN_NOMUTEX
        } else {
            SQLITE_OPEN_FULLMUTEX
        };
        let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_URI | mutex;
        let mut db = std::ptr::null_mut();
        // SAFETY: the strings are NUL-terminated and outlive the call, and `db` is a valid out pointer.
        let rc = unsafe {
            sqlite3_open_v2(
                name.as_ptr(),
                &mut db,
                flags,
                vfs.as_ref().map_or(std::ptr::null(), |vfs| vfs.as_ptr()),
            )
        };
        let connection = Self { db };
        assert_eq!(rc, SQLITE_OK, "open {name:?}: {}", connection.error());
        // SAFETY: `db` is open, and writers must wait for each other rather than fail.
        unsafe { sqlite3_busy_timeout(db, 600_000) };
        if modality.encrypted() {
            connection.exec(KEY);
        }
        connection
    }

    fn error(&self) -> String {
        // SAFETY: SQLite returns a NUL-terminated message owned by the connection, copied at once.
        unsafe { CStr::from_ptr(sqlite3_errmsg(self.db)) }
            .to_string_lossy()
            .into_owned()
    }

    fn exec(&self, sql: &CStr) {
        // SAFETY: the connection is open and `sql` is NUL-terminated.
        let rc = unsafe {
            sqlite3_exec(
                self.db,
                sql.as_ptr(),
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, SQLITE_OK, "{sql:?}: {}", self.error());
    }

    fn prepare(&self, sql: &CStr) -> Statement {
        let mut stmt = std::ptr::null_mut();
        // SAFETY: the connection is open, `sql` is NUL-terminated and `stmt` is a valid out pointer.
        let rc = unsafe {
            sqlite3_prepare_v2(self.db, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut())
        };
        assert_eq!(rc, SQLITE_OK, "{sql:?}: {}", self.error());
        Statement(stmt)
    }

    fn populate(&self) {
        let exists = self.prepare(c"SELECT count(*) FROM sqlite_schema WHERE name = 't'");
        if exists.sum_rows(&[]) > 0 {
            return;
        }
        let sql = format!(
            "BEGIN;
             CREATE TABLE t(k INTEGER PRIMARY KEY, v INTEGER NOT NULL, s TEXT NOT NULL);
             CREATE VIRTUAL TABLE f USING fts5(body);
             CREATE TABLE w(id INTEGER PRIMARY KEY, worker INTEGER NOT NULL, v INTEGER NOT NULL);
             WITH RECURSIVE n(k) AS (SELECT 1 UNION ALL SELECT k + 1 FROM n WHERE k < {ROWS})
             INSERT INTO t SELECT k, (k * 7919) % 10007, printf('row-%08d', k) FROM n;
             INSERT INTO f(rowid, body) SELECT k, printf('w%d w%d w%d w%d w%d w%d w%d w%d',
                 k * 7 % {VOCABULARY}, k * 13 % {VOCABULARY}, k * 31 % {VOCABULARY}, k * 61 % {VOCABULARY},
                 k * 97 % {VOCABULARY}, k * 131 % {VOCABULARY}, k * 181 % {VOCABULARY}, k * 211 % {VOCABULARY})
             FROM t;
             COMMIT;"
        );
        self.exec(&CString::new(sql).unwrap());
    }

    fn warm(&self) {
        let statement = self.prepare(c"SELECT count(*), sum(v), sum(length(s)) FROM t");
        statement.sum_rows(&[]);
        let statement = self.prepare(c"SELECT count(*) FROM f WHERE f MATCH 'w1'");
        statement.sum_rows(&[]);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: every statement is finalized before its connection is dropped.
        unsafe { sqlite3_close(self.db) };
    }
}

struct Statement(*mut sqlite3_stmt);

impl Statement {
    /// Binds `params`, steps every row and sums the first column.
    fn sum_rows(&self, params: &[i64]) -> i64 {
        let mut sum = 0i64;
        // SAFETY: the statement is prepared, parameters are bound by 1-based index and it is reset after use.
        unsafe {
            for (index, value) in params.iter().enumerate() {
                sqlite3_bind_int64(self.0, i32::try_from(index + 1).unwrap(), *value);
            }
            loop {
                match sqlite3_step(self.0) {
                    SQLITE_ROW => sum = sum.wrapping_add(sqlite3_column_int64(self.0, 0)),
                    SQLITE_DONE => break,
                    rc => panic!("step failed with {rc}"),
                }
            }
            sqlite3_reset(self.0);
        }
        sum
    }
}

impl Drop for Statement {
    fn drop(&mut self) {
        // SAFETY: the statement was prepared and is finalized exactly once.
        unsafe { sqlite3_finalize(self.0) };
    }
}

thread_local! {
    // Connections outlive single runs so their caches stay warm and private databases survive.
    static CONNECTIONS: RefCell<HashMap<(Modality, bool), Connection>> = RefCell::new(HashMap::new());
    // The serving worker keeps its statements, so the baseline pays no preparation per request.
    static SERVING: RefCell<Option<Prepared>> = const { RefCell::new(None) };
}

fn with_connection<T>(modality: Modality, nomutex: bool, f: impl FnOnce(&Connection) -> T) -> T {
    CONNECTIONS.with(|connections| {
        let connections = connections.borrow();
        f(connections
            .get(&(modality, nomutex))
            .expect("setup runs before any workload"))
    })
}

/// Opens this worker's connection, creating and filling its database first when needed.
#[wasm_bindgen]
pub fn setup(modality: u32, worker: u32, nomutex: bool) {
    report_panics();
    let modality = Modality::from_code(modality);
    let connection = Connection::open(modality, worker, nomutex);
    connection.populate();
    connection.warm();
    CONNECTIONS.with(|connections| {
        connections
            .borrow_mut()
            .insert((modality, nomutex), connection)
    });
}

/// Installs this worker's own OPFS pool, then opens its database in it.
#[wasm_bindgen]
pub async fn setup_sahpool(worker: u32) -> Result<(), JsValue> {
    use sqlite_wasm_vfs::sahpool::{install, OpfsSAHPoolCfgBuilder};
    let name = sahpool_name(worker);
    let config = OpfsSAHPoolCfgBuilder::new()
        .vfs_name(&name)
        .directory(&format!("threadsafe-bench/{name}"))
        .clear_on_init(true)
        .initial_capacity(8)
        .build();
    install::<WasmOsCallback>(&config, false)
        .await
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    setup(3, worker, false);
    Ok(())
}

/// Closes this worker's connection and drops its per-worker memvfs file.
#[wasm_bindgen]
pub fn teardown(modality: u32, worker: u32, nomutex: bool) {
    use sqlite_wasm_rs::vfs::VfsFilesManager;
    let modality = Modality::from_code(modality);
    SERVING.with(|serving| serving.borrow_mut().take());
    CONNECTIONS.with(|connections| connections.borrow_mut().remove(&(modality, nomutex)));
    if let Some(file) = modality.memvfs_file(worker) {
        // SAFETY: memvfs was installed by SQLite's initialization and every connection to `file` is closed.
        let util = unsafe { sqlite_wasm_rs::vfs::memvfs::MemVfsUtil::get() }.unwrap();
        util.remove(&file).unwrap();
        util.remove(&format!("{file}-journal")).unwrap();
    }
}

const START_LINES: usize = 1 << 16;
static START_LINES_ARRIVED: [AtomicU32; START_LINES] = [const { AtomicU32::new(0) }; START_LINES];

/// Releases the `parties` workers of one measurement point together, spinning so no worker needs its event loop.
#[wasm_bindgen]
pub fn start_line(point: u32, parties: u32) {
    let arrived = &START_LINES_ARRIVED[usize::try_from(point).unwrap() % START_LINES];
    arrived.fetch_add(1, Ordering::AcqRel);
    while arrived.load(Ordering::Acquire) < parties {
        std::hint::spin_loop();
    }
}

struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: i64) -> i64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Keeps the high bits, which carry the generator's period.
        let bound = u64::try_from(bound).unwrap();
        i64::try_from((self.0 >> 33) % bound).unwrap()
    }
}

#[derive(Clone, Copy)]
enum Workload {
    Point,
    Range,
    Scan,
    Sort,
    Fts,
    Insert,
    Mix,
    /// Opens a keyed connection and reads its schema, which runs the key derivation.
    KeyedOpen,
    /// Scans the table through a 16-page cache, so every page is read and decrypted again.
    ColdScan,
}

impl Workload {
    fn from_code(code: u32) -> Self {
        match code {
            0 => Self::Point,
            1 => Self::Range,
            2 => Self::Scan,
            3 => Self::Sort,
            4 => Self::Fts,
            5 => Self::Insert,
            6 => Self::Mix,
            7 => Self::KeyedOpen,
            8 => Self::ColdScan,
            _ => panic!("unknown workload {code}"),
        }
    }
}

struct Prepared {
    point: Statement,
    range: Statement,
    scan: Statement,
    sort: Statement,
    fts: Statement,
    insert: Statement,
    update: Statement,
    cold_scan: Statement,
}

impl Prepared {
    fn new(connection: &Connection) -> Self {
        Self {
            point: connection.prepare(c"SELECT v FROM t WHERE k = ?1"),
            range: connection.prepare(c"SELECT sum(v) FROM t WHERE k BETWEEN ?1 AND ?1 + 999"),
            scan: connection.prepare(c"SELECT count(*) + sum(k) FROM t GROUP BY v % 97"),
            sort: connection.prepare(
                c"SELECT v FROM t WHERE k BETWEEN ?1 AND ?1 + 1999 ORDER BY v DESC, s LIMIT 10",
            ),
            fts: connection.prepare(c"SELECT count(*) FROM f WHERE f MATCH ?1"),
            insert: connection.prepare(c"INSERT INTO w(worker, v) VALUES (?1, ?2)"),
            update: connection.prepare(c"UPDATE t SET v = v + 1 WHERE k = ?1"),
            cold_scan: connection.prepare(c"SELECT sum(length(s)) FROM t"),
        }
    }

    fn one(&self, connection: &Connection, workload: Workload, rng: &mut Lcg, worker: i64) -> i64 {
        match workload {
            Workload::Point => self.point.sum_rows(&[1 + rng.below(ROWS)]),
            Workload::Range => self.range.sum_rows(&[1 + rng.below(ROWS - RANGE)]),
            Workload::Scan => self.scan.sum_rows(&[]),
            Workload::Sort => self.sort.sum_rows(&[1 + rng.below(ROWS - SORT_RANGE)]),
            Workload::Fts => {
                let word = CString::new(format!("w{}", rng.below(VOCABULARY))).unwrap();
                // SAFETY: the statement is prepared and SQLite copies the transient text.
                unsafe {
                    sqlite3_bind_text(self.fts.0, 1, word.as_ptr(), -1, SQLITE_TRANSIENT());
                }
                self.fts.sum_rows(&[])
            }
            Workload::Insert => {
                connection.exec(c"BEGIN IMMEDIATE");
                for i in 0..INSERT_BATCH {
                    self.insert.sum_rows(&[worker, i64::try_from(i).unwrap()]);
                }
                connection.exec(c"COMMIT");
                1
            }
            Workload::ColdScan => self.cold_scan.sum_rows(&[]),
            Workload::KeyedOpen => unreachable!("keyed opens run without a prepared connection"),
            Workload::Mix => {
                if rng.below(10) == 0 {
                    self.update.sum_rows(&[1 + rng.below(ROWS)]);
                    1
                } else {
                    self.point.sum_rows(&[1 + rng.below(ROWS)])
                }
            }
        }
    }
}

/// Runs `ops` operations after the start line, returning `[start, end, checksum]` in shared-clock milliseconds.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn run(
    point: u32,
    parties: u32,
    modality: u32,
    nomutex: bool,
    workload: u32,
    batched: bool,
    worker: u32,
    ops: u32,
) -> Vec<f64> {
    let (modality, workload) = (Modality::from_code(modality), Workload::from_code(workload));
    with_connection(modality, nomutex, |connection| {
        let prepared = Prepared::new(connection);
        let mut rng = Lcg(u64::from(point) << 32 | u64::from(worker));
        if matches!(workload, Workload::ColdScan) {
            connection.exec(c"PRAGMA cache_size = 16");
        }
        if matches!(workload, Workload::Insert) && (worker == 0 || !modality.shared()) {
            // Keeps databases from growing across points, outside the timed span.
            connection.exec(c"DELETE FROM w");
        }
        start_line(point, parties);
        let start = now();
        if batched {
            connection.exec(c"BEGIN");
        }
        let slot = usize::try_from(worker).unwrap() % MAX_WORKERS;
        PROGRESS[slot].store(0, Ordering::Relaxed);
        PROGRESS[MAX_WORKERS + slot].store(1, Ordering::Relaxed);
        let mut checksum = 0i64;
        for _ in 0..ops {
            if matches!(workload, Workload::KeyedOpen) {
                let opened = Connection::open(modality, worker, nomutex);
                opened.exec(c"SELECT count(*) FROM sqlite_schema");
                PROGRESS[slot].fetch_add(1, Ordering::Relaxed);
                continue;
            }
            checksum = checksum.wrapping_add(prepared.one(
                connection,
                workload,
                &mut rng,
                i64::from(worker),
            ));
            PROGRESS[slot].fetch_add(1, Ordering::Relaxed);
        }
        PROGRESS[MAX_WORKERS + slot].store(0, Ordering::Relaxed);
        if matches!(workload, Workload::ColdScan) {
            connection.exec(c"PRAGMA cache_size = -16384");
        }
        if batched {
            connection.exec(c"COMMIT");
        }
        let end = now();
        // The checksum only keeps the work observable, so its low 52 bits suffice.
        vec![start, end, (checksum & ((1 << 52) - 1)) as f64]
    })
}

/// One autocommit operation for the postMessage baseline, where a single worker serves every client.
#[wasm_bindgen]
pub fn serve(workload: u32, seed: u32) -> f64 {
    let workload = Workload::from_code(workload);
    with_connection(Modality::MemvfsShared, false, |connection| {
        SERVING.with(|serving| {
            let mut serving = serving.borrow_mut();
            let prepared = serving.get_or_insert_with(|| Prepared::new(connection));
            let mut rng = Lcg(u64::from(seed));
            // Checksums stay far below 2^52, so the conversion is exact.
            prepared.one(connection, workload, &mut rng, 0) as f64
        })
    })
}

#[cfg(feature = "threadsafe")]
static STRESS_COUNTER: AtomicU32 = AtomicU32::new(0);

const MAX_WORKERS: usize = 64;
// Per worker: operations finished in the current run, 1 while inside `run` and 0 after it, and the
// last reply stage the worker's JS reached (2 before posting, 3 after).
static PROGRESS: [AtomicU32; 3 * MAX_WORKERS] = [const { AtomicU32::new(0) }; 3 * MAX_WORKERS];

/// Address of the progress words, which the orchestrator reads when a worker stops answering.
#[wasm_bindgen]
pub fn progress_address() -> u32 {
    // Linear memory addresses fit in 32 bits on wasm32.
    PROGRESS.as_ptr() as u32
}

/// Enters and leaves one static SQLite mutex `iterations` times, counting with separate load and
/// store so any lapse in mutual exclusion loses increments.
#[cfg(feature = "threadsafe")]
#[wasm_bindgen]
pub fn mutex_stress(point: u32, parties: u32, iterations: u32) -> u32 {
    // SAFETY: static mutexes live for the whole process.
    let mutex = unsafe { sqlite3_mutex_alloc(SQLITE_MUTEX_STATIC_APP3) };
    start_line(point, parties);
    for _ in 0..iterations {
        // SAFETY: `mutex` is valid and entered and left on this thread.
        unsafe { sqlite3_mutex_enter(mutex) };
        let value = STRESS_COUNTER.load(Ordering::Relaxed);
        STRESS_COUNTER.store(value + 1, Ordering::Relaxed);
        // SAFETY: entered just above.
        unsafe { sqlite3_mutex_leave(mutex) };
    }
    STRESS_COUNTER.load(Ordering::Relaxed)
}

/// Grows the heap once and returns the space to the allocator, so later work never grows memory.
#[wasm_bindgen]
pub fn reserve_heap(megabytes: u32) {
    let bytes = usize::try_from(megabytes).unwrap() << 20;
    let mut block = Vec::<u8>::with_capacity(bytes);
    // Touching the block keeps the allocation from being optimized away.
    block.push(1);
    drop(std::hint::black_box(block));
}
