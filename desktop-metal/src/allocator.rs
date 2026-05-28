// Bump allocator — no dealloc, suited for a long-running desktop with bounded peak usage.
// Minimum 16-byte alignment on every allocation: LLVM emits movaps/vmovaps for Vec
// internals which fault (#GP) on sub-16-byte-aligned heap pointers.
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

// 24 MiB — large enough for a 1920×1080×32 backbuffer (~8.3 MiB) plus the
// terminals/windows working set. Two kernel-side changes make this safe:
//   1. the ring-3 stack was moved high (vmm::USER_STACK_TOP ≈ 63 MiB), out of
//      this heap's .bss region;
//   2. the kernel relocates GRUB's initrd module to a high address before
//      loading this ELF, so the in-place .bss zero-fill can't clobber it.
// 32 MiB. Holds the 1080p backbuffer (~8.3 MiB) + the static-background cache
// (another ~8.3 MiB, for smooth window dragging) + the apps/terminals working
// set. BSS then ends ~36 MiB, still below the relocated initrd at 40 MiB and
// the ring-3 stack at ~63 MiB (see kernel vmm::USER_STACK_TOP).
const HEAP_BYTES: usize = 32 * 1024 * 1024;

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
