use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

// 512 KiB static heap in .bss — enough for Vec-based AI runtime demos
const HEAP_SIZE: usize = 512 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

static HEAP_START: AtomicUsize = AtomicUsize::new(0);
static HEAP_NEXT:  AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    let start = unsafe { core::ptr::addr_of!(HEAP) as usize };
    HEAP_START.store(start, Ordering::Relaxed);
    HEAP_NEXT.store(start, Ordering::Relaxed);
}

pub fn used() -> usize {
    HEAP_NEXT.load(Ordering::Relaxed).saturating_sub(HEAP_START.load(Ordering::Relaxed))
}

struct BumpAllocator;

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let start = HEAP_START.load(Ordering::Relaxed);
        if start == 0 { return core::ptr::null_mut(); }
        loop {
            let cur   = HEAP_NEXT.load(Ordering::Relaxed);
            let align = (cur + layout.align() - 1) & !(layout.align() - 1);
            let next  = align + layout.size();
            if next > start + HEAP_SIZE { return core::ptr::null_mut(); }
            if HEAP_NEXT.compare_exchange(cur, next, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                return align as *mut u8;
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator — dealloc is a no-op; reset on kernel restart
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;
