//! Concurrency scenarios on one memvfs database, shared by the native and Wasm suites.
//!
//! The including crate root provides `mod ffi` with SQLite's C API, and memvfs installed.

use super::ffi::{
    sqlite3, sqlite3_busy_timeout, sqlite3_close, sqlite3_column_int64, sqlite3_errmsg,
    sqlite3_exec, sqlite3_finalize, sqlite3_open_v2, sqlite3_prepare_v2, sqlite3_step, SQLITE_OK,
    SQLITE_OPEN_CREATE, SQLITE_OPEN_FULLMUTEX, SQLITE_OPEN_READWRITE, SQLITE_ROW,
};
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicUsize, Ordering};

/// One connection to a memvfs database, closed explicitly so a failed close is reported.
pub struct Connection(*mut sqlite3);

// SAFETY: every connection is opened with SQLITE_OPEN_FULLMUTEX, so SQLite serializes all use of it.
unsafe impl Send for Connection {}
// SAFETY: as for `Send`, the connection mutex serializes calls made through a shared reference.
unsafe impl Sync for Connection {}

impl Connection {
    pub fn open(name: &str) -> Self {
        let name = CString::new(name).unwrap();
        let mut db = std::ptr::null_mut();
        let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX;
        // SAFETY: both strings are NUL-terminated and outlive the call, and `db` is a valid out pointer.
        let rc = unsafe { sqlite3_open_v2(name.as_ptr(), &mut db, flags, c"memvfs".as_ptr()) };
        let connection = Self(db);
        assert_eq!(rc, SQLITE_OK, "open: {}", connection.error());
        // SAFETY: `db` is an open connection owned by `connection`.
        assert_eq!(unsafe { sqlite3_busy_timeout(db, 60_000) }, SQLITE_OK);
        connection
    }

    fn error(&self) -> String {
        // SAFETY: SQLite returns a NUL-terminated message owned by the connection, copied at once.
        unsafe { CStr::from_ptr(sqlite3_errmsg(self.0)) }
            .to_string_lossy()
            .into_owned()
    }

    pub fn exec(&self, sql: &str) {
        let sql = CString::new(sql).unwrap();
        // SAFETY: the connection is open and `sql` is NUL-terminated for the whole call.
        let rc = unsafe {
            sqlite3_exec(
                self.0,
                sql.as_ptr(),
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, SQLITE_OK, "{sql:?}: {}", self.error());
    }

    pub fn query_row(&self, sql: &str, columns: usize) -> Vec<i64> {
        let sql = CString::new(sql).unwrap();
        let mut stmt = std::ptr::null_mut();
        // SAFETY: the connection is open, `sql` is NUL-terminated and `stmt` is a valid out pointer.
        let rc = unsafe {
            sqlite3_prepare_v2(self.0, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut())
        };
        assert_eq!(rc, SQLITE_OK, "{sql:?}: {}", self.error());
        // SAFETY: `stmt` was prepared above and is finalized below.
        let rc = unsafe { sqlite3_step(stmt) };
        assert_eq!(rc, SQLITE_ROW, "{sql:?}: {}", self.error());
        let row = (0..columns)
            .map(|column| {
                let column = i32::try_from(column).unwrap();
                // SAFETY: `stmt` holds a row and `column` is within the selected columns.
                unsafe { sqlite3_column_int64(stmt, column) }
            })
            .collect();
        // SAFETY: `stmt` is finalized exactly once.
        unsafe { sqlite3_finalize(stmt) };
        row
    }

    pub fn assert_integrity(&self) {
        let ok = "SELECT count(*) FROM pragma_integrity_check WHERE integrity_check = 'ok'";
        assert_eq!(self.query_row(ok, 1), [1], "integrity_check failed");
    }

    pub fn close(self) {
        let db = self.0;
        std::mem::forget(self);
        // SAFETY: the connection is open, every statement was finalized, and `forget` skips `Drop`.
        assert_eq!(unsafe { sqlite3_close(db) }, SQLITE_OK);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: the connection is open and every statement was finalized. A failed close only
        // leaks on a path that already panicked.
        unsafe { sqlite3_close(self.0) };
    }
}

/// Creates a one-row counter at zero, after running `prepare` on the new connection.
pub fn create_counter(name: &str, prepare: &dyn Fn(&Connection)) -> Connection {
    let connection = Connection::open(name);
    prepare(&connection);
    connection.exec("CREATE TABLE counter(n INTEGER NOT NULL); INSERT INTO counter VALUES (0);");
    connection
}

/// Opens its own connection, runs `prepare` on it, waits for `start`, then adds one to the counter `times` times.
pub fn increment(name: &str, times: usize, prepare: &dyn Fn(&Connection), start: &dyn Fn()) {
    let connection = Connection::open(name);
    prepare(&connection);
    start();
    for _ in 0..times {
        connection.exec("BEGIN IMMEDIATE; UPDATE counter SET n = n + 1; COMMIT;");
    }
    connection.close();
}

pub fn assert_counter(connection: &Connection, expected: usize) {
    let expected = i64::try_from(expected).unwrap();
    assert_eq!(connection.query_row("SELECT n FROM counter", 1), [expected]);
    connection.assert_integrity();
}

pub fn create_pairs(name: &str, prepare: &dyn Fn(&Connection)) -> Connection {
    let connection = Connection::open(name);
    prepare(&connection);
    connection.exec("CREATE TABLE t(k INTEGER PRIMARY KEY, v INTEGER NOT NULL);");
    connection
}

/// Inserts `pairs` pairs of rows `(k, k)` and `(-k, -k)`, one pair per transaction.
pub fn write_pairs(
    name: &str,
    writer: usize,
    pairs: usize,
    prepare: &dyn Fn(&Connection),
    start: &dyn Fn(),
) {
    let connection = Connection::open(name);
    prepare(&connection);
    start();
    for pair in 0..pairs {
        let key = i64::try_from(1 + writer * pairs + pair).unwrap();
        // Each pair sums to zero, so a torn snapshot shows a nonzero sum or an odd count.
        connection.exec(&format!(
            "BEGIN IMMEDIATE; INSERT INTO t VALUES ({key}, {key}); INSERT INTO t VALUES ({}, {}); COMMIT;",
            -key, -key
        ));
    }
    connection.close();
}

/// Reads count and sum until `done` holds, checking every snapshot is committed and monotonic.
pub fn read_pairs(
    name: &str,
    done: &dyn Fn() -> bool,
    prepare: &dyn Fn(&Connection),
    start: &dyn Fn(),
) -> usize {
    let connection = Connection::open(name);
    prepare(&connection);
    start();
    let mut last_count = 0;
    let mut reads = 0;
    loop {
        let finished = done();
        let row = connection.query_row("SELECT count(*), coalesce(sum(v), 0) FROM t", 2);
        let (count, sum) = (row[0], row[1]);
        assert_eq!(count % 2, 0, "torn snapshot with {count} rows");
        assert_eq!(sum, 0, "torn snapshot with sum {sum}");
        assert!(
            count >= last_count,
            "count went back from {last_count} to {count}"
        );
        last_count = count;
        reads += 1;
        if finished {
            break;
        }
    }
    connection.close();
    reads
}

pub fn assert_pairs(connection: &Connection, writers: usize, pairs: usize) {
    let expected = i64::try_from(2 * writers * pairs).unwrap();
    assert_eq!(
        connection.query_row("SELECT count(*), sum(v) FROM t", 2),
        [expected, 0]
    );
    connection.assert_integrity();
}

/// A start line that releases every participant at once by spinning, so no Wasm worker needs its event loop.
pub struct StartLine {
    arrived: AtomicUsize,
    parties: usize,
}

impl StartLine {
    pub const fn new(parties: usize) -> Self {
        Self {
            arrived: AtomicUsize::new(0),
            parties,
        }
    }

    pub fn wait(&self) {
        self.arrived.fetch_add(1, Ordering::AcqRel);
        while self.arrived.load(Ordering::Acquire) < self.parties {
            std::hint::spin_loop();
        }
    }
}
