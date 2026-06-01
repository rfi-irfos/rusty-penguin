// Global allocator for the bare-metal desktop. A real free-list allocator (see
// `heap.rs`) over a 32 MiB static arena — it RECLAIMS freed memory, so a
// long-running session that opens and closes windows for hours stays bounded
// instead of marching the old bump pointer toward exhaustion + a crash. The
// free-list core is fuzz-tested on the host (tools/heap_fuzz.rs); here we wrap
// one instance behind a spinlock to satisfy `GlobalAlloc`.
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::heap::Heap;

// 32 MiB. Holds the 1080p backbuffer (~8.3 MiB) + the static-background cache
// (~8.3 MiB, for smooth window dragging) + the apps/terminals working set. BSS
// then ends ~36 MiB, still below the relocated initrd at 40 MiB and the ring-3
// stack at ~63 MiB (see kernel vmm::USER_STACK_TOP). Two kernel-side changes
// make placing this arena in .bss safe: the ring-3 stack was moved high, and
// the kernel relocates GRUB's initrd module before loading this ELF so the
// in-place .bss zero-fill cannot clobber it.
const HEAP_BYTES: usize = 32 * 1024 * 1024;

#[repr(align(16))]
struct Arena([u8; HEAP_BYTES]);
static mut ARENA: Arena = Arena([0; HEAP_BYTES]);

/// `Heap` behind a tiny spinlock. The ring-3 desktop is single-threaded and
/// never allocates from an interrupt handler (those run in the kernel against a
/// different allocator), so the lock never actually contends — it exists only
/// to make the `GlobalAlloc` impl sound.
pub struct LockedHeap {
    inner: UnsafeCell<Heap>,
    inited: UnsafeCell<bool>,
    lock: AtomicBool,
}

unsafe impl Sync for LockedHeap {}

impl LockedHeap {
    const fn new() -> Self {
        LockedHeap {
            inner: UnsafeCell::new(Heap::new()),
            inited: UnsafeCell::new(false),
            lock: AtomicBool::new(false),
        }
    }

    #[inline]
    fn enter(&self) {
        while self.lock.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
    }

    #[inline]
    fn leave(&self) {
        self.lock.store(false, Ordering::Release);
    }

    /// Lazily lay down the single free block spanning the arena on first use.
    /// Called under the lock; `init` itself never allocates, so no re-entrancy.
    #[inline]
    unsafe fn ensure_init(&self) {
        let inited = &mut *self.inited.get();
        if !*inited {
            let base = core::ptr::addr_of_mut!(ARENA) as usize;
            (*self.inner.get()).init(base, HEAP_BYTES);
            *inited = true;
        }
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.enter();
        self.ensure_init();
        let p = (*self.inner.get()).alloc(layout.size(), layout.align());
        self.leave();
        p as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.enter();
        // `dealloc` can only be reached after at least one `alloc`, so the heap
        // is already initialised; the freed size is rounded identically to alloc.
        (*self.inner.get()).dealloc(ptr as usize, layout.size());
        self.leave();
    }
}

#[global_allocator]
pub static ALLOCATOR: LockedHeap = LockedHeap::new();

/// Bytes currently handed out (live). Unlike the old bump pointer this rises and
/// FALLS as windows open and close — a true heap-usage gauge, not just a
/// high-water mark. Read without the lock: single-threaded, so the snapshot is
/// benign.
pub fn used_bytes() -> usize {
    unsafe { (*ALLOCATOR.inner.get()).live() }
}

/// Total heap capacity in bytes.
pub fn total_bytes() -> usize {
    HEAP_BYTES
}
