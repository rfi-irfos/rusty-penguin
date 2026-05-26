use core::sync::atomic::{AtomicUsize, AtomicBool, Ordering};

const PAGE_SIZE: usize  = 4096;
const MAX_PHYS:  usize  = 4 * 1024 * 1024 * 1024; // 4 GiB ceiling
const MAX_PAGES: usize  = MAX_PHYS / PAGE_SIZE;    // 1 048 576 pages
const BITMAP_LEN: usize = MAX_PAGES / 64;          // 16 384 u64s = 128 KiB in .bss

// Bit = 1 means the page is FREE, 0 means USED.
// Starts zeroed (all used); init() marks available regions free.
static mut BITMAP: [u64; BITMAP_LEN] = [0u64; BITMAP_LEN];

static FREE_FRAMES:  AtomicUsize = AtomicUsize::new(0);
static TOTAL_FRAMES: AtomicUsize = AtomicUsize::new(0);
static READY:        AtomicBool  = AtomicBool::new(false);

#[inline]
fn mark_free(page: usize) {
    if page < MAX_PAGES {
        unsafe { BITMAP[page / 64] |= 1 << (page % 64); }
    }
}

#[inline]
fn mark_used(page: usize) {
    if page < MAX_PAGES {
        unsafe { BITMAP[page / 64] &= !(1 << (page % 64)); }
    }
}

#[inline]
fn is_free(page: usize) -> bool {
    page < MAX_PAGES && unsafe { BITMAP[page / 64] & (1 << (page % 64)) != 0 }
}

pub fn init(mb2_addr: u32, kernel_end: u64) {
    if mb2_addr == 0 { return; }

    unsafe {
        let total_size = *(mb2_addr as *const u32);
        let mut offset: u32 = 8;

        while offset < total_size {
            let tag_ptr  = (mb2_addr + offset) as *const u32;
            let ttype    = *tag_ptr;
            let tsize    = *tag_ptr.add(1);
            if ttype == 0 { break; }

            if ttype == 6 {
                // Memory map tag: 16-byte header, entry_size at byte 8
                let entry_size = *((mb2_addr + offset + 8) as *const u32);
                let mut eoff = mb2_addr + offset + 16;
                let eend     = mb2_addr + offset + tsize;

                while eoff < eend {
                    let base:  u64 = *((eoff + 0)  as *const u64);
                    let len:   u64 = *((eoff + 8)  as *const u64);
                    let etype: u32 = *((eoff + 16) as *const u32);

                    if etype == 1 && base < MAX_PHYS as u64 {
                        let pg_start = ((base as usize) + PAGE_SIZE - 1) / PAGE_SIZE;
                        let pg_end   = ((base + len) as usize).min(MAX_PHYS) / PAGE_SIZE;
                        let count    = pg_end.saturating_sub(pg_start);
                        for pg in pg_start..pg_end {
                            mark_free(pg);
                        }
                        FREE_FRAMES.fetch_add(count, Ordering::Relaxed);
                        TOTAL_FRAMES.fetch_add(count, Ordering::Relaxed);
                    }
                    eoff += entry_size;
                }
                break;
            }
            offset += (tsize + 7) & !7;
        }
    }

    // Reserve low 1 MiB (BIOS, ROM, VGA MMIO)
    for pg in 0..(0x10_0000 / PAGE_SIZE) {
        if is_free(pg) { mark_used(pg); FREE_FRAMES.fetch_sub(1, Ordering::Relaxed); }
    }

    // Reserve kernel image (0x100000 → kernel_end, rounded up)
    let kend_page = (kernel_end as usize + PAGE_SIZE - 1) / PAGE_SIZE;
    for pg in (0x10_0000 / PAGE_SIZE)..kend_page {
        if is_free(pg) { mark_used(pg); FREE_FRAMES.fetch_sub(1, Ordering::Relaxed); }
    }

    READY.store(true, Ordering::Release);
}

/// Allocate one 4 KiB physical frame. Returns the physical address.
pub fn alloc_frame() -> Option<u64> {
    if !READY.load(Ordering::Acquire) { return None; }
    unsafe {
        for i in 0..BITMAP_LEN {
            if BITMAP[i] != 0 {
                let bit  = BITMAP[i].trailing_zeros() as usize;
                let page = i * 64 + bit;
                mark_used(page);
                FREE_FRAMES.fetch_sub(1, Ordering::Relaxed);
                return Some((page * PAGE_SIZE) as u64);
            }
        }
    }
    None
}

/// Return a frame to the free pool.
pub fn free_frame(addr: u64) {
    let page = (addr as usize) / PAGE_SIZE;
    if !is_free(page) {
        mark_free(page);
        FREE_FRAMES.fetch_add(1, Ordering::Relaxed);
    }
}

/// (free_frames, total_frames)
pub fn stats() -> (usize, usize) {
    (FREE_FRAMES.load(Ordering::Relaxed), TOTAL_FRAMES.load(Ordering::Relaxed))
}
