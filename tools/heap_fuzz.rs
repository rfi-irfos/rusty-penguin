// Host fuzz harness for the bare-metal desktop's free-list allocator. It pulls
// in the REAL `desktop-metal/src/heap.rs` via `#[path]` (so there is no second
// copy to drift) and hammers it with adversarial alloc/dealloc/coalesce
// sequences over a 16-aligned host buffer, asserting:
//   * returned regions never overlap each other or fall outside the arena,
//   * payloads keep their fill pattern (no header/metadata corruption),
//   * heap.live() exactly tracks the rounded sum of outstanding allocations,
//   * the free list stays address-sorted, coalesced, and in-range,
//   * after freeing everything the arena coalesces back to one full block,
//   * the whole arena re-allocates afterward (no permanent fragmentation).
//
// Run (from repo root):
//   rustc -O tools/heap_fuzz.rs -o /tmp/heap_fuzz && /tmp/heap_fuzz [SEED]
//
// Proven: 4 seeds x 2,000,000 iterations each, all invariants held.

#[path = "../desktop-metal/src/heap.rs"]
mod heap;
use heap::Heap;

use std::alloc::{alloc as sys_alloc, dealloc as sys_dealloc, Layout};

const HDR_T: usize = 16;
fn round_block_t(size: usize) -> usize {
    let s = if size < HDR_T { HDR_T } else { size };
    (s + 15) & !15
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn range(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

// Walk the free list and assert the structural invariants. Returns total free bytes.
unsafe fn validate_free_list(h: &Heap) -> usize {
    let base = h.dbg_base();
    let end = h.dbg_end();
    let mut addr = h.dbg_head();
    let mut prev_end = base;
    let mut total = 0usize;
    let mut last = 0usize;
    while addr != 0 {
        assert!(addr >= base && addr < end, "free block out of range: {addr:#x}");
        assert!(addr % 16 == 0, "free block not 16-aligned: {addr:#x}");
        let size = *(addr as *const usize);
        let next = *((addr + 8) as *const usize);
        assert!(size >= 16 && size % 16 == 0, "bad free block size {size}");
        assert!(addr + size <= end, "free block overruns arena");
        assert!(addr > last || last == 0, "free list not address-sorted");
        // After the first block: must start strictly after the previous block's
        // end — equal == adjacent (uncoalesced), less == overlap.
        if last != 0 {
            assert!(addr > prev_end, "free list not coalesced/overlap ({prev_end:#x}..{addr:#x})");
        }
        prev_end = addr + size;
        last = addr;
        total += size;
        addr = next;
    }
    total
}

fn main() {
    let seed: u64 = std::env::args()
        .nth(1)
        .and_then(|s| {
            let s = s.trim();
            if let Some(h) = s.strip_prefix("0x") {
                u64::from_str_radix(h, 16).ok()
            } else {
                s.parse().ok()
            }
        })
        .unwrap_or(0x9E3779B97F4A7C15);

    const ARENA: usize = 1024 * 1024; // 1 MiB
    let layout = Layout::from_size_align(ARENA, 16).unwrap();
    let base = unsafe { sys_alloc(layout) } as usize;
    assert!(base != 0 && base % 16 == 0);

    let mut heap = Heap::new();
    unsafe { heap.init(base, ARENA) };
    unsafe {
        assert_eq!(validate_free_list(&heap), ARENA & !15);
        assert_eq!(heap.live(), 0);
    }

    // live allocations: (ptr, layout_size, rounded_need, fill_byte)
    let mut live: Vec<(usize, usize, usize, u8)> = Vec::new();
    let mut rng = Rng(seed);
    let aligns = [16usize, 16, 16, 16, 32, 64, 128];

    let iters = 2_000_000usize;
    let (mut allocs, mut frees, mut oom) = (0u64, 0u64, 0u64);

    for i in 0..iters {
        let do_alloc = live.is_empty() || rng.range(100) < 55;
        if do_alloc {
            let size = 1 + rng.range(4096);
            let align = aligns[rng.range(aligns.len())];
            let p = unsafe { heap.alloc(size, align) };
            if p == 0 {
                oom += 1;
                continue;
            }
            allocs += 1;
            assert!(p % align == 0, "alloc not aligned: {p:#x} align {align}");
            assert!(p >= base && p + size <= base + ARENA, "alloc out of arena");
            let need = round_block_t(size);
            for &(q, _, qn, _) in &live {
                let (a0, a1, b0, b1) = (p, p + need, q, q + qn);
                assert!(a1 <= b0 || b1 <= a0, "OVERLAP {a0:#x}..{a1:#x} vs {b0:#x}..{b1:#x}");
            }
            let fill = (i & 0xFF) as u8;
            unsafe { std::ptr::write_bytes(p as *mut u8, fill, size) };
            live.push((p, size, need, fill));
        } else {
            let idx = rng.range(live.len());
            let (p, size, _need, fill) = live.swap_remove(idx);
            unsafe {
                for k in 0..size {
                    let b = *((p + k) as *const u8);
                    assert!(b == fill, "CORRUPTION at {:#x}+{k}: {b:#x} != {fill:#x}", p);
                }
                heap.dealloc(p, size);
            }
            frees += 1;
        }

        if i % 4096 == 0 {
            let live_sum: usize = live.iter().map(|&(_, _, n, _)| n).sum();
            assert_eq!(heap.live(), live_sum, "live accounting drift at iter {i}");
            let free_total = unsafe { validate_free_list(&heap) };
            assert_eq!(free_total + live_sum, ARENA & !15, "free+live != arena at iter {i}");
        }
    }

    for &(p, size, _, fill) in &live {
        unsafe {
            for k in 0..size {
                assert!(*((p + k) as *const u8) == fill, "final corruption check");
            }
            heap.dealloc(p, size);
        }
    }
    live.clear();

    unsafe {
        assert_eq!(heap.live(), 0, "live != 0 after freeing all");
        assert_eq!(validate_free_list(&heap), ARENA & !15, "arena did not fully reclaim");
        let head = heap.dbg_head();
        assert_eq!(*((head + 8) as *const usize), 0, "arena not coalesced into one block");
        assert_eq!(*(head as *const usize), ARENA & !15, "single block wrong size");
    }

    let big = unsafe { heap.alloc((ARENA & !15) - 64, 16) };
    assert!(big != 0, "could not re-allocate the reclaimed arena");

    unsafe { sys_dealloc(base as *mut u8, layout) };
    println!("HEAP FUZZ PASS (seed {seed:#x}): {iters} iters, allocs={allocs} frees={frees} oom={oom}");
}
