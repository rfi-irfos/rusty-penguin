// Bump allocator — no dealloc, suited for a long-running desktop with bounded peak usage.
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

const HEAP_BYTES: usize = 8 * 1024 * 1024; // 8 MiB

pub struct BumpAllocator {
    next: AtomicUsize,
}

static mut HEAP: [u8; HEAP_BYTES] = [0; HEAP_BYTES];

unsafe impl Sync for BumpAllocator {}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size  = layout.size();
        let align = layout.align();
        let mut cur = self.next.load(Ordering::Relaxed);
        loop {
            let aligned  = (cur + align - 1) & !(align - 1);
            let new_next = aligned + size;
            if new_next > HEAP_BYTES { return core::ptr::null_mut(); }
            match self.next.compare_exchange_weak(cur, new_next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_)  => return HEAP.as_mut_ptr().add(aligned),
                Err(n) => cur = n,
            }
        }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
pub static ALLOCATOR: BumpAllocator = BumpAllocator { next: AtomicUsize::new(0) };
