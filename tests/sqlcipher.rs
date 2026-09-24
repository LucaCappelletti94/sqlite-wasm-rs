use sqlite_wasm_rs::vfs::transfer::{DbTransfer, ExportSource};
use sqlite_wasm_rs::vfs::VfsFilesManager;
use sqlite_wasm_rs::*;
use std::{
    ffi::{CStr, CString},
    ptr,
};
use wasm_bindgen_test::wasm_bindgen_test;

struct Db(*mut sqlite3);

impl Db {
    fn open(name: &CStr, flags: i32) -> Self {
        let mut raw = ptr::null_mut();
        let code = unsafe { sqlite3_open_v2(name.as_ptr(), &mut raw, flags, ptr::null()) };
        let db = Self(raw);
        assert_eq!(code, SQLITE_OK);
        db
    }

    fn exec(&self, sql: &CStr) -> i32 {
        unsafe { sqlite3_exec(self.0, sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) }
    }

    fn pragma_text(&self, name: &str) -> String {
        let sql = CString::new(format!("PRAGMA {name}")).unwrap();
        let mut stmt = ptr::null_mut();
        let rc =
            unsafe { sqlite3_prepare_v2(self.0, sql.as_ptr(), -1, &mut stmt, ptr::null_mut()) };
        if rc != SQLITE_OK || stmt.is_null() {
            return String::new();
        }
        let mut result = String::new();
        if unsafe { sqlite3_step(stmt) } == SQLITE_ROW {
            // SAFETY: non-null and NUL-terminated, valid until the next step, and copied before advancing.
            let p = unsafe { sqlite3_column_text(stmt, 0) };
            if !p.is_null() {
                result = unsafe { CStr::from_ptr(p.cast()) }
                    .to_string_lossy()
                    .into_owned();
            }
        }
        unsafe { sqlite3_finalize(stmt) };
        result
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        assert_eq!(unsafe { sqlite3_close(self.0) }, SQLITE_OK);
    }
}

fn apply_key(db: &Db, passphrase: &[u8]) {
    assert_eq!(
        unsafe {
            sqlite3_key(
                db.0,
                passphrase.as_ptr().cast(),
                passphrase.len() as i32, // FFI: key length; encryption keys are always short
            )
        },
        SQLITE_OK,
    );
}

#[wasm_bindgen_test]
fn test_cipher_info() {
    let db = Db::open(
        c"sqlcipher-info.db",
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
    );
    apply_key(&db, b"k");
    let version = db.pragma_text("cipher_version");
    assert!(
        version.starts_with("4.19.0"),
        "expected cipher_version 4.19.0, got {version:?}",
    );
    assert_eq!(db.pragma_text("cipher_provider"), "libtomcrypt");
    drop(db);
    unsafe { vfs::memvfs::MemVfsUtil::get().unwrap() }
        .remove("sqlcipher-info.db")
        .unwrap();
}

#[wasm_bindgen_test]
fn test_encrypt_decrypt() {
    // Write encrypted data, then close so all pages are flushed to the memvfs store.
    {
        let db = Db::open(
            c"sqlcipher-enc.db",
            SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
        );
        apply_key(&db, b"correct horse battery staple");
        assert_eq!(db.exec(c"CREATE TABLE t (s TEXT NOT NULL);"), SQLITE_OK);
        assert_eq!(
            db.exec(c"INSERT INTO t VALUES ('hello sqlcipher');"),
            SQLITE_OK
        );
    }

    // Export raw bytes through the memvfs and verify no plaintext leaks.
    {
        let util = unsafe { vfs::memvfs::MemVfsUtil::get().unwrap() };
        let mut src = util.open_export("sqlcipher-enc.db").unwrap();
        let size = src.size();
        let mut bytes = vec![0u8; usize::try_from(size).expect("size fits usize")];
        src.read_at(0, &mut bytes).unwrap();
        assert!(
            !bytes.windows(16).any(|w| w == b"SQLite format 3\0"),
            "database header must be encrypted",
        );
        assert!(
            !bytes.windows(15).any(|w| w == b"hello sqlcipher"),
            "row data must be encrypted",
        );
    }

    // No key: must be rejected.
    {
        let db = Db::open(c"sqlcipher-enc.db", SQLITE_OPEN_READWRITE);
        assert_ne!(db.exec(c"SELECT s FROM t;"), SQLITE_OK);
    }
    // Wrong key: must be rejected.
    {
        let db = Db::open(c"sqlcipher-enc.db", SQLITE_OPEN_READWRITE);
        apply_key(&db, b"wrong passphrase");
        assert_ne!(db.exec(c"SELECT s FROM t;"), SQLITE_OK);
    }
    // Correct key: must succeed and return the stored row.
    {
        let db = Db::open(c"sqlcipher-enc.db", SQLITE_OPEN_READWRITE);
        apply_key(&db, b"correct horse battery staple");
        let mut stmt = ptr::null_mut();
        unsafe {
            assert_eq!(
                sqlite3_prepare_v2(
                    db.0,
                    c"SELECT s FROM t".as_ptr(),
                    -1,
                    &mut stmt,
                    ptr::null_mut(),
                ),
                SQLITE_OK,
            );
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            // SAFETY: non-null and NUL-terminated, valid until the next step, and copied at once.
            let p = sqlite3_column_text(stmt, 0);
            assert!(!p.is_null());
            let value = CStr::from_ptr(p.cast()).to_string_lossy().into_owned();
            assert_eq!(sqlite3_step(stmt), SQLITE_DONE);
            assert_eq!(sqlite3_finalize(stmt), SQLITE_OK);
            assert_eq!(value, "hello sqlcipher");
        }
    }

    unsafe { vfs::memvfs::MemVfsUtil::get().unwrap() }
        .remove("sqlcipher-enc.db")
        .unwrap();
}

#[wasm_bindgen_test]
fn test_rekey() {
    let db = Db::open(
        c"sqlcipher-rekey.db",
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
    );
    apply_key(&db, b"old passphrase");
    assert_eq!(db.exec(c"CREATE TABLE t (n INTEGER NOT NULL);"), SQLITE_OK);
    assert_eq!(db.exec(c"INSERT INTO t VALUES (99);"), SQLITE_OK);
    // Rekey while the database is open.
    let new_pass: &[u8] = b"new passphrase";
    assert_eq!(
        unsafe {
            sqlite3_rekey(
                db.0,
                new_pass.as_ptr().cast(),
                new_pass.len() as i32, // FFI: key length; encryption keys are always short
            )
        },
        SQLITE_OK,
    );
    drop(db);

    // Old key no longer works.
    {
        let db = Db::open(c"sqlcipher-rekey.db", SQLITE_OPEN_READWRITE);
        apply_key(&db, b"old passphrase");
        assert_ne!(db.exec(c"SELECT n FROM t;"), SQLITE_OK);
    }
    // New key works and data is intact.
    {
        let db = Db::open(c"sqlcipher-rekey.db", SQLITE_OPEN_READWRITE);
        apply_key(&db, b"new passphrase");
        let mut stmt = ptr::null_mut();
        unsafe {
            assert_eq!(
                sqlite3_prepare_v2(
                    db.0,
                    c"SELECT n FROM t".as_ptr(),
                    -1,
                    &mut stmt,
                    ptr::null_mut(),
                ),
                SQLITE_OK,
            );
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_int(stmt, 0), 99);
            assert_eq!(sqlite3_step(stmt), SQLITE_DONE);
            assert_eq!(sqlite3_finalize(stmt), SQLITE_OK);
        }
    }

    unsafe { vfs::memvfs::MemVfsUtil::get().unwrap() }
        .remove("sqlcipher-rekey.db")
        .unwrap();
}
