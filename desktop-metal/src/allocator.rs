// Bump allocator — no dealloc, suited for a long-running desktop with bounded peak usage.
// Minimum 16-byte alignment on every allocation: LLVM emits movaps/vmovaps for Vec
// internals which fault (#GP) on sub-16-byte-aligned heap pointers.
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

const HEAP_BYTES: usize = 8 * 1024 * 1024; // 8 MiB

pub struct BumpAllocator {
    next: AtomicUsize,
}

#[repr(align(16))]
struct AlignedHeap([u8; HEAP_BYTES]);
static mut HEAP: AlignedHeap = AlignedHeap([0; HEAP_BYTES]);

unsafe impl Sync for BumpAllocator {}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size  = layout.size();
        let align = layout.align().max(16); // never below 16 — SSE movaps requirement
        let mut cur = self.next.load(Ordering::Relaxed);
        loop {
            let aligned  = (cur + align - 1) & !(align - 1);
            let new_next = aligned + size;
            if new_next > HEAP_BYTES { return core::ptr::null_mut(); }
            match self.next.compare_exchange_weak(cur, new_next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_)  => return HEAP.0.as_mut_ptr().add(aligned),
                Err(n) => cur = n,
            }
        }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
pub static ALLOCATOR: BumpAllocator = BumpAllocator { next: AtomicUsize::new(0) };

/// Bytes currently committed by the bump allocator. Since the allocator
/// never frees, this only grows — useful as a "leak monitor" in the topbar.
pub fn used_bytes() -> usize {
    ALLOCATOR.next.load(Ordering::Relaxed)
}

/// Total heap capacity in bytes.
pub fn total_bytes() -> usize { HEAP_BYTES }
