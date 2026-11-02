//! Files stored as IndexedDB blocks. Writes collect in memory until a flush
//! stores them, any truncation and the new size in one IndexedDB transaction.

use super::{
    context,
    idb::{Durability, Meta, MAX_SAFE_INTEGER},
    js_vfs_error,
    lock::{self, HeldLock, LockError},
    Inner, BLOCK_SIZE,
};
use js_sys::Uint8Array;
use rsqlite_vfs::{
    DeviceCharacteristics, LockLevel, SectorSize, SyncOptions, VfsError, VfsErrorCode, VfsFile,
    VfsResult,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    rc::Rc,
};

const BLOCK: u64 = BLOCK_SIZE as u64; // 4096 fits in u64.

/// Widens a byte count. `usize` is at most 64 bits on every Rust target.
fn len64(len: usize) -> u64 {
    len as u64
}

/// Narrows an offset inside one block.
fn in_block(offset: u64) -> usize {
    debug_assert!(offset <= BLOCK, "{offset} is not inside a block");
    offset as usize
}

fn blocks_for(size: u64) -> u64 {
    size.div_ceil(BLOCK)
}

fn zero_block() -> Box<[u8]> {
    vec![0; BLOCK_SIZE].into_boxed_slice()
}

/// Byte range of block `index` covered by `[offset, end)`, relative to the block.
fn segment(index: u64, offset: u64, end: u64) -> (usize, usize) {
    let start = index * BLOCK;
    (
        in_block(offset.max(start) - start),
        in_block(end.min(start + BLOCK) - start),
    )
}

fn size_limit() -> VfsError {
    VfsError::new(VfsErrorCode::Full, "file size limit reached".into())
}

/// Blocks read from IndexedDB for one call, `None` for blocks never stored.
type Fetched = HashMap<u64, Option<Box<[u8]>>>;

/// One file as seen by this context, shared by all its open handles.
pub(crate) struct FileState {
    name: String,
    /// A temporary file, kept only in memory and never locked.
    ephemeral: bool,
    /// The file is in IndexedDB. Cleared when it is deleted while open.
    exists: bool,
    /// Logical size, buffered writes included.
    size: u64,
    /// What IndexedDB holds as of the last flush or revalidation.
    durable: Meta,
    dirty: BTreeMap<u64, Box<[u8]>>,
    /// Smallest size truncated to since the last flush.
    truncate_floor: Option<u64>,
    /// Size when the open atomic write batch began.
    batch: Option<u64>,
    pub(crate) handles: usize,
    /// A handle of this context holds the database lock.
    locked: bool,
}

impl FileState {
    pub(crate) fn new(name: &str, meta: Meta) -> Self {
        Self {
            name: name.into(),
            ephemeral: false,
            exists: true,
            size: meta.size,
            durable: meta,
            dirty: BTreeMap::new(),
            truncate_floor: None,
            batch: None,
            handles: 0,
            locked: false,
        }
    }

    /// A temporary file, which lives only in its dirty blocks.
    pub(crate) fn temporary() -> Self {
        Self {
            ephemeral: true,
            exists: false,
            ..Self::new("", Meta::default())
        }
    }

    pub(crate) fn exists(&self) -> bool {
        self.exists
    }

    pub(crate) fn is_temporary(&self) -> bool {
        self.ephemeral
    }

    /// Replaces the contents with `meta` from IndexedDB, discarding buffers.
    pub(crate) fn reset(&mut self, meta: Option<Meta>) {
        self.exists = meta.is_some();
        self.durable = meta.unwrap_or_default();
        self.size = self.durable.size;
        self.dirty.clear();
        self.truncate_floor = None;
        self.batch = None;
    }

    /// Blocks below this index may be stored, and the rest read as zeros.
    fn stored_blocks(&self) -> u64 {
        blocks_for(
            self.durable
                .size
                .min(self.truncate_floor.unwrap_or(u64::MAX)),
        )
    }

    fn needs_fetch(&self, index: u64) -> bool {
        index < self.stored_blocks() && !self.dirty.contains_key(&index)
    }

    fn has_pending(&self) -> bool {
        !self.dirty.is_empty() || self.truncate_floor.is_some() || self.size != self.durable.size
    }

    /// The dirty copy of block `index`, made from its current contents if needed.
    fn dirty_block(&mut self, fetched: &mut Fetched, index: u64) -> &mut [u8] {
        self.dirty
            .entry(index)
            .or_insert_with(|| fetched.remove(&index).flatten().unwrap_or_else(zero_block))
    }
}

/// A handle on a file stored in IndexedDB.
pub(crate) struct IdbFile {
    pub(crate) inner: Rc<Inner>,
    pub(crate) state: Rc<RefCell<FileState>>,
    pub(crate) read_only: bool,
    level: LockLevel,
    /// The database lock, held at every level above NONE.
    lock: Option<HeldLock>,
}

impl IdbFile {
    pub(crate) fn new(inner: Rc<Inner>, state: Rc<RefCell<FileState>>, read_only: bool) -> Self {
        Self {
            inner,
            state,
            read_only,
            level: LockLevel::None,
            lock: None,
        }
    }

    fn name(&self) -> String {
        self.state.borrow().name.clone()
    }

    fn writable(&self) -> VfsResult<()> {
        context::check_entry()?;
        if self.read_only {
            return Err(VfsError::new(
                VfsErrorCode::ReadOnly,
                "file is read-only".into(),
            ));
        }
        Ok(())
    }

    /// Reads the blocks of `[offset, end)` that only IndexedDB holds, leaving
    /// out blocks the range covers whole when they are about to be overwritten.
    fn prefetch(
        &self,
        offset: u64,
        end: u64,
        overwrite: bool,
        code: VfsErrorCode,
    ) -> VfsResult<Fetched> {
        let missing = {
            let state = self.state.borrow();
            (offset / BLOCK..blocks_for(end))
                .filter(|&index| {
                    let whole = offset <= index * BLOCK && (index + 1) * BLOCK <= end;
                    !(overwrite && whole) && state.needs_fetch(index)
                })
                .collect::<Vec<_>>()
        };
        if missing.is_empty() {
            return Ok(Fetched::new());
        }
        let blocks = self
            .inner
            .idb
            .blocks(&self.name(), &missing)
            .map_err(|err| js_vfs_error(code, "read blocks from IndexedDB", err))?;
        Ok(missing.into_iter().zip(blocks).collect())
    }

    /// Stores buffered writes and truncations in one IndexedDB transaction.
    pub(crate) fn flush(&self, durability: Durability, code: VfsErrorCode) -> VfsResult<()> {
        let (name, blocks, truncate_from, meta) = {
            let state = self.state.borrow();
            if state.ephemeral || !state.has_pending() {
                return Ok(());
            }
            let blocks = state
                .dirty
                .iter()
                .map(|(&index, block)| (index, Uint8Array::from(&block[..])))
                .collect::<Vec<_>>();
            let meta = Meta {
                size: state.size,
                generation: state.durable.generation + 1,
            };
            (
                state.name.clone(),
                blocks,
                state.truncate_floor.map(blocks_for),
                meta,
            )
        };
        self.inner
            .idb
            .flush(&name, blocks, truncate_from, meta, durability)
            .map_err(|err| js_vfs_error(code, "write to IndexedDB", err))?;

        let mut state = self.state.borrow_mut();
        state.dirty.clear();
        state.durable = meta;
        state.truncate_floor = None;
        state.exists = true;
        Ok(())
    }

    /// Reloads the size another context may have changed.
    fn revalidate(&self) -> VfsResult<()> {
        let meta = self
            .inner
            .idb
            .meta(&self.name())
            .map_err(|err| js_vfs_error(VfsErrorCode::IoLock, "read file metadata", err))?;
        let mut state = self.state.borrow_mut();
        if meta != Some(state.durable) || !state.exists {
            debug_assert!(!state.has_pending(), "buffered writes without a lock");
            state.reset(meta);
        }
        Ok(())
    }

    pub(crate) fn begin_batch(&mut self) -> VfsResult<()> {
        context::check_entry()?;
        self.flush(Durability::Default, VfsErrorCode::IoWrite)?;
        let mut state = self.state.borrow_mut();
        state.batch = Some(state.size);
        Ok(())
    }

    pub(crate) fn commit_batch(&mut self) -> VfsResult<()> {
        context::check_entry()?;
        self.flush(Durability::Strict, VfsErrorCode::IoWrite)?;
        self.state.borrow_mut().batch = None;
        Ok(())
    }

    pub(crate) fn rollback_batch(&mut self) {
        let mut state = self.state.borrow_mut();
        if let Some(size) = state.batch.take() {
            state.dirty.clear();
            state.truncate_floor = None;
            state.size = size;
        }
    }

    /// Takes the database lock, which keeps every other connection out.
    fn lock_database(&mut self) -> VfsResult<()> {
        if self.state.borrow().locked {
            return Err(VfsError::new(
                VfsErrorCode::Busy,
                "database is locked in this context".into(),
            ));
        }
        let name = format!("{}/{}", self.inner.lock_prefix, self.name());
        let held =
            lock::acquire(&self.inner.locks, &name, self.inner.lock_timeout).map_err(|err| {
                match err {
                    LockError::Busy => {
                        VfsError::new(VfsErrorCode::Busy, "database is locked".into())
                    }
                    LockError::Js(err) => {
                        js_vfs_error(VfsErrorCode::IoLock, "request a Web Lock", err)
                    }
                }
            })?;
        // Dropping `held` on failure releases the lock.
        self.revalidate()?;
        self.state.borrow_mut().locked = true;
        self.lock = Some(held);
        Ok(())
    }
}

impl VfsFile for IdbFile {
    fn sector_size(&self) -> SectorSize {
        SectorSize::DEFAULT
    }

    fn device_characteristics(&self) -> DeviceCharacteristics {
        // Writes between two flushes commit in one IndexedDB transaction.
        DeviceCharacteristics::POWERSAFE_OVERWRITE
            .union(DeviceCharacteristics::SAFE_APPEND)
            .union(DeviceCharacteristics::SEQUENTIAL)
            .union(DeviceCharacteristics::BATCH_ATOMIC)
    }

    fn read(&mut self, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
        context::check_entry()?;
        let size = self.state.borrow().size;
        if buf.is_empty() || offset >= size {
            return Ok(0);
        }
        let end = size.min(offset.saturating_add(len64(buf.len())));
        let fetched = self.prefetch(offset, end, false, VfsErrorCode::IoRead)?;

        let state = self.state.borrow();
        let zeros = [0; BLOCK_SIZE];
        let mut copied = 0;
        for index in offset / BLOCK..blocks_for(end) {
            let (from, to) = segment(index, offset, end);
            let block: &[u8] = match (state.dirty.get(&index), fetched.get(&index)) {
                (Some(block), _) | (None, Some(Some(block))) => block,
                _ => &zeros,
            };
            buf[copied..copied + to - from].copy_from_slice(&block[from..to]);
            copied += to - from;
        }
        Ok(copied)
    }

    fn write(&mut self, buf: &[u8], offset: u64) -> VfsResult<()> {
        self.writable()?;
        if buf.is_empty() {
            return Ok(());
        }
        let end = offset
            .checked_add(len64(buf.len()))
            .filter(|&end| end <= MAX_SAFE_INTEGER)
            .ok_or_else(size_limit)?;
        let mut fetched = self.prefetch(offset, end, true, VfsErrorCode::IoWrite)?;

        let mut state = self.state.borrow_mut();
        let mut written = 0;
        for index in offset / BLOCK..blocks_for(end) {
            let (from, to) = segment(index, offset, end);
            state.dirty_block(&mut fetched, index)[from..to]
                .copy_from_slice(&buf[written..written + to - from]);
            written += to - from;
        }
        state.size = state.size.max(end);
        Ok(())
    }

    fn truncate(&mut self, size: u64) -> VfsResult<()> {
        self.writable()?;
        if size > MAX_SAFE_INTEGER {
            return Err(size_limit());
        }
        if size >= self.state.borrow().size {
            self.state.borrow_mut().size = size;
            return Ok(());
        }
        let partial = size % BLOCK != 0;
        let mut fetched = if partial {
            self.prefetch(size, size + 1, false, VfsErrorCode::IoTruncate)?
        } else {
            Fetched::new()
        };

        let mut state = self.state.borrow_mut();
        if partial {
            // Bytes past the end must read as zeros if the file grows again.
            state.dirty_block(&mut fetched, size / BLOCK)[in_block(size % BLOCK)..].fill(0);
        }
        let kept = blocks_for(size);
        state.dirty.retain(|&index, _| index < kept);
        state.truncate_floor = Some(state.truncate_floor.map_or(size, |floor| floor.min(size)));
        state.size = size;
        Ok(())
    }

    fn sync(&mut self, _options: SyncOptions) -> VfsResult<()> {
        context::check_entry()?;
        // A batch becomes durable as a whole at COMMIT_ATOMIC_WRITE.
        if self.state.borrow().batch.is_some() {
            return Ok(());
        }
        self.flush(Durability::Strict, VfsErrorCode::IoSync)
    }

    fn size(&self) -> VfsResult<u64> {
        context::check_entry()?;
        Ok(self.state.borrow().size)
    }

    fn lock(&mut self, level: LockLevel) -> VfsResult<()> {
        context::check_entry()?;
        if level <= self.level {
            return Ok(());
        }
        if self.state.borrow().ephemeral {
            self.level = level;
            return Ok(());
        }
        if level == LockLevel::Pending {
            return Err(VfsError::new(
                VfsErrorCode::IoLock,
                "PENDING cannot be requested".into(),
            ));
        }
        if self.lock.is_none() {
            self.lock_database()?;
        }
        self.level = level;
        Ok(())
    }

    fn unlock(&mut self, level: LockLevel) -> VfsResult<()> {
        if level >= self.level {
            return Ok(());
        }
        let mut result = context::check_entry();
        if result.is_ok() && self.state.borrow().has_pending() {
            // Other contexts read IndexedDB as soon as this lock is gone.
            result = self.flush(Durability::Default, VfsErrorCode::IoUnlock);
        }
        if result.is_err() {
            let durable = self.state.borrow().durable;
            self.state.borrow_mut().reset(Some(durable));
        }

        if level == LockLevel::None || result.is_err() {
            if self.lock.take().is_some() {
                self.state.borrow_mut().locked = false;
            }
            self.level = LockLevel::None;
        } else {
            self.level = LockLevel::Shared;
        }
        result
    }

    /// No other connection can hold a lock while this one holds the database lock.
    fn check_reserved_lock(&self) -> VfsResult<bool> {
        context::check_entry()?;
        Ok(self.level >= LockLevel::Reserved)
    }
}
