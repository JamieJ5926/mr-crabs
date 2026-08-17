//! Counting global allocator for S12 allocation metrics.
//!
//! Installed only in the bench crate's own binaries (feature `count-alloc`,
//! on by default), so no other workspace crate observes it. The counters are
//! process-local; each measured run executes in a fresh process, so
//! `before`/`after` deltas scope exactly to the workload.

use serde::{Deserialize, Serialize};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

/// Allocation counters for one measured window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AllocationStats {
    /// Number of `alloc`/`alloc_zeroed`/`realloc` events.
    pub count: u64,
    /// Bytes requested by those events (net growth for `realloc`).
    pub bytes: u64,
}

/// A `System`-backed allocator that counts allocation events and requested
/// bytes with relaxed atomics. `dealloc` is not counted (bytes were already
/// attributed at allocation time).
pub struct CountingAllocator;

// SAFETY: forwards to the thread-safe `System` allocator; the atomic
// counters are updated with relaxed ordering, which is sufficient for
// monotonic statistics. All methods uphold the `GlobalAlloc` contract by
// delegating to `System` for the same layout/alignment invariants.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: delegated verbatim to the system allocator with the same
        // layout contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: delegated verbatim to the system allocator with the same
        // layout contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        if new_size > layout.size() {
            ALLOCATED_BYTES.fetch_add((new_size - layout.size()) as u64, Ordering::Relaxed);
        }
        // SAFETY: delegated verbatim to the system allocator with the same
        // pointer/layout/new_size contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: delegated verbatim to the system allocator with the same
        // pointer/layout contract.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[cfg(feature = "count-alloc")]
#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// Snapshot of the process-wide allocation counters.
pub fn stats() -> AllocationStats {
    AllocationStats {
        count: ALLOCATIONS.load(Ordering::Relaxed),
        bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
    }
}
