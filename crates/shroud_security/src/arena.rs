//! Secure memory arena — a single mlock'd region with size-class allocation.
//!
//! All SecureSignal values live here. One `mlock` syscall covers the entire
//! arena, respecting OS per-process limits (Linux default: 64 KB).

use std::cell::{Cell, RefCell};
use std::fmt;
use std::ptr::NonNull;

/// Size classes: 64 B, 256 B, 1 KB, 4 KB.
/// Allocations are rounded up to the nearest class.
const SIZE_CLASSES: [usize; 4] = [64, 256, 1024, 4096];

/// Default arena capacity: 64 KB (fits within Linux's default mlock limit).
pub const DEFAULT_ARENA_CAPACITY: usize = 64 * 1024;

// ── Error ──────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ArenaError {
    /// `memsec::malloc_sized` returned `None` (OS refused the allocation or mlock).
    AllocationFailed,
    /// The arena's bump region is exhausted and no free slot is available.
    OutOfMemory,
    /// Requested size exceeds the largest size class (4 KB).
    AllocationTooLarge,
}

impl fmt::Display for ArenaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AllocationFailed => write!(f, "secure arena allocation failed"),
            Self::OutOfMemory => write!(f, "secure arena out of memory"),
            Self::AllocationTooLarge => write!(f, "allocation exceeds max size class (4096 bytes)"),
        }
    }
}

impl std::error::Error for ArenaError {}

// ── ArenaSlot ──────────────────────────────────────────────────────

/// A handle to an allocation within a [`SecureArena`].
///
/// Copy/lightweight — the arena manages the actual memory.
#[derive(Clone, Copy, Debug)]
pub struct ArenaSlot {
    pub(crate) offset: usize,
    pub(crate) class_idx: u8,
}

impl ArenaSlot {
    /// Size of this slot's class in bytes.
    pub fn class_size(&self) -> usize {
        SIZE_CLASSES[self.class_idx as usize]
    }

    /// Byte offset of this slot within the arena.
    pub fn offset(&self) -> usize {
        self.offset
    }
}

// ── SecureArena ────────────────────────────────────────────────────

/// A contiguous mlock'd memory region with guard pages (via `memsec`).
///
/// Provides bump allocation with per-class free lists. All memory is
/// zeroized on drop (bulk zeroize), even if individual `dealloc` calls
/// were missed.
pub struct SecureArena {
    /// memsec-allocated region: mlock'd, with guard pages on both sides.
    region: NonNull<[u8]>,
    capacity: usize,
    bump: Cell<usize>,
    /// Per-size-class free lists storing offsets of returned slots.
    free_lists: RefCell<[Vec<usize>; 4]>,
}

impl SecureArena {
    /// Create a new arena with the given capacity.
    ///
    /// The memory is allocated via `memsec::malloc_sized`, which provides:
    /// - Guard pages (buffer overflow detection)
    /// - `mlock` (prevents swapping to disk)
    /// - Canary values (corruption detection)
    pub fn new(capacity: usize) -> Result<Self, ArenaError> {
        let region =
            unsafe { memsec::malloc_sized(capacity) }.ok_or(ArenaError::AllocationFailed)?;

        // Zero out memsec's initial garbage fill
        unsafe {
            memsec::memzero(region.as_ptr() as *mut u8, capacity);
        }

        Ok(Self {
            region,
            capacity,
            bump: Cell::new(0),
            free_lists: RefCell::new(Default::default()),
        })
    }

    /// Allocate a slot for a value of the given `size` and `align`.
    ///
    /// The allocation is rounded up to the nearest size class.
    pub fn alloc(&self, size: usize, align: usize) -> Result<ArenaSlot, ArenaError> {
        let effective = size.max(align);
        let class_idx = SIZE_CLASSES
            .iter()
            .position(|&c| c >= effective)
            .ok_or(ArenaError::AllocationTooLarge)?;
        let class_size = SIZE_CLASSES[class_idx];

        // 1. Check free list for this size class
        {
            let mut lists = self.free_lists.borrow_mut();
            if let Some(offset) = lists[class_idx].pop() {
                return Ok(ArenaSlot {
                    offset,
                    class_idx: class_idx as u8,
                });
            }
        }

        // 2. Bump allocate (class_size is a power-of-2 → self-aligning)
        let offset = align_up(self.bump.get(), class_size);
        let end = offset + class_size;
        if end > self.capacity {
            return Err(ArenaError::OutOfMemory);
        }
        self.bump.set(end);

        Ok(ArenaSlot {
            offset,
            class_idx: class_idx as u8,
        })
    }

    /// Return a slot to the arena and zeroize its memory.
    pub fn dealloc(&self, slot: ArenaSlot) {
        let class_size = SIZE_CLASSES[slot.class_idx as usize];
        let ptr = self.slot_ptr_mut(slot);
        unsafe {
            memsec::memzero(ptr, class_size);
        }
        self.free_lists.borrow_mut()[slot.class_idx as usize].push(slot.offset);
    }

    /// Raw const pointer to a slot's data.
    pub fn slot_ptr(&self, slot: ArenaSlot) -> *const u8 {
        unsafe { self.base_ptr().add(slot.offset) }
    }

    /// Raw mutable pointer to a slot's data.
    pub fn slot_ptr_mut(&self, slot: ArenaSlot) -> *mut u8 {
        unsafe { self.base_ptr().add(slot.offset) }
    }

    /// Zeroize the entire arena (nuclear option).
    ///
    /// Called automatically on `Drop`, but can be invoked manually for
    /// explicit shutdown sequences.
    pub fn zeroize_all(&self) {
        unsafe {
            memsec::memzero(self.base_ptr(), self.capacity);
        }
        self.bump.set(0);
        self.free_lists.borrow_mut().iter_mut().for_each(Vec::clear);
    }

    /// Total capacity in bytes.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes consumed by the bump allocator (not counting freed slots).
    pub fn bump_used(&self) -> usize {
        self.bump.get()
    }

    fn base_ptr(&self) -> *mut u8 {
        self.region.as_ptr() as *mut u8
    }
}

impl Drop for SecureArena {
    fn drop(&mut self) {
        // Bulk zeroize everything — catches any slots that weren't individually freed
        unsafe {
            memsec::memzero(self.base_ptr(), self.capacity);
            memsec::free(self.region);
        }
    }
}

// SecureArena is not Send/Sync — it's designed for thread-local use.

fn align_up(offset: usize, align: usize) -> usize {
    (offset + align - 1) & !(align - 1)
}
