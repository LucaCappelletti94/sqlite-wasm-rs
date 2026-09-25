//! A global allocator that keeps small freed blocks in per-thread lists, so most SQLite
//! allocations skip the global allocator's lock. It measures that lock and is not tuned.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::UnsafeCell;

/// Size classes 16, 32, ... 32768 bytes, all 16-byte aligned.
const CLASSES: usize = 12;
const MAX_CACHED: usize = 512;

struct Cache {
    heads: [*mut u8; CLASSES],
    counts: [usize; CLASSES],
}

#[thread_local]
static CACHE: UnsafeCell<Cache> = UnsafeCell::new(Cache {
    heads: [std::ptr::null_mut(); CLASSES],
    counts: [0; CLASSES],
});

fn class(layout: Layout) -> Option<usize> {
    if layout.align() > 16 {
        return None;
    }
    let size = layout.size().max(16).next_power_of_two();
    let index = usize::try_from(size.trailing_zeros()).ok()? - 4;
    (index < CLASSES).then_some(index)
}

fn class_layout(index: usize) -> Layout {
    // Every class is a power of two of at least 16 bytes with 16-byte alignment, which is always valid.
    Layout::from_size_align(16 << index, 16).unwrap()
}

struct ThreadCache;

// SAFETY: blocks are handed out once, each list belongs to one thread, and every block
// returned to `System` carries the class layout it was allocated with. `dealloc` receives the
// caller's layout, so a block freed on another thread still maps to the same class.
unsafe impl GlobalAlloc for ThreadCache {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some(index) = class(layout) else {
            return System.alloc(layout);
        };
        let cache = &mut *CACHE.get();
        let head = cache.heads[index];
        if head.is_null() {
            return System.alloc(class_layout(index));
        }
        cache.heads[index] = head.cast::<*mut u8>().read();
        cache.counts[index] -= 1;
        head
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let Some(index) = class(layout) else {
            return System.dealloc(ptr, layout);
        };
        let cache = &mut *CACHE.get();
        if cache.counts[index] >= MAX_CACHED {
            return System.dealloc(ptr, class_layout(index));
        }
        ptr.cast::<*mut u8>().write(cache.heads[index]);
        cache.heads[index] = ptr;
        cache.counts[index] += 1;
    }
}

#[global_allocator]
static ALLOCATOR: ThreadCache = ThreadCache;
