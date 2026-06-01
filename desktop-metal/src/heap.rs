//! Free-list heap core — an address-sorted free list with block splitting and
//! coalescing on free. Unlike the old bump allocator, this RECLAIMS memory, so
//! opening and closing windows for hours no longer marches the heap toward
//! exhaustion. The code is `core`-only and holds no global state, so the exact
//! same implementation is fuzz-tested on the host (see tools/heap_fuzz.rs);
//! `allocator.rs` instantiates one `Heap` over the static .bss arena behind a
//! spinlock to satisfy `GlobalAlloc`.
//!
//! Invariants maintained:
//!   * the free list is sorted by ascending address and maximally coalesced
//!     (no two free blocks are adjacent),
//!   * every block address is 16-aligned and every block size is a multiple of
//!     16 and at least 16 (room for the {size,next} header while free),
//!   * `live` equals the number of payload bytes (rounded) currently handed out.

/// Header bytes that must fit in any free block: `size: usize` + `next: usize`.
const HDR: usize = 16;
/// All allocations are at least 16-aligned: LLVM emits movaps/vmovaps for `Vec`
/// internals which #GP on sub-16-byte-aligned pointers.
const ALIGN: usize = 16;

#[inline]
fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

#[inline]
fn round_block(size: usize) -> usize {
    let s = if size < HDR { HDR } else { size };
    align_up(s, ALIGN)
}

/// A free-list heap over a single contiguous arena. Free blocks store their
/// `{size, next}` header at the start of the free region; once a region is
/// handed out the header bytes become caller payload.
pub struct Heap {
    head: usize, // address of the first free block, or 0 if the list is empty
    base: usize,
    end: usize,
    live: usize, // payload bytes (rounded) currently handed out
}

impl Heap {
    pub const fn new() -> Self {
        Heap { head: 0, base: 0, end: 0, live: 0 }
    }

    /// One-time initialisation over `[base, base+size)`. `base` must be
    /// 16-aligned; `size` is truncated down to a multiple of 16.
    ///
    /// # Safety
    /// The arena must be valid, exclusively owned, and live for the lifetime of
    /// this `Heap`.
    pub unsafe fn init(&mut self, base: usize, size: usize) {
        let size = size & !(ALIGN - 1);
        self.base = base;
        self.end = base + size;
        self.live = 0;
        self.head = base;
        Self::write(base, size, 0); // one free block spanning the whole arena
    }

    #[inline]
    unsafe fn size_of(addr: usize) -> usize {
        *(addr as *const usize)
    }
    #[inline]
    unsafe fn next_of(addr: usize) -> usize {
        *((addr + 8) as *const usize)
    }
    #[inline]
    unsafe fn set_size(addr: usize, size: usize) {
        *(addr as *mut usize) = size;
    }
    #[inline]
    unsafe fn set_next(addr: usize, next: usize) {
        *((addr + 8) as *mut usize) = next;
    }
    #[inline]
    unsafe fn write(addr: usize, size: usize, next: usize) {
        Self::set_size(addr, size);
        Self::set_next(addr, next);
    }

    /// Bytes currently handed out to callers.
    pub fn live(&self) -> usize {
        self.live
    }

    /// Debug accessors — let an external validator walk the free list. Unused in
    /// the no_std build (dead-code-eliminated); exercised by the host fuzzer.
    pub fn dbg_head(&self) -> usize {
        self.head
    }
    pub fn dbg_base(&self) -> usize {
        self.base
    }
    pub fn dbg_end(&self) -> usize {
        self.end
    }

    /// Allocate `size` bytes aligned to `align`. Returns 0 on out-of-memory
    /// (first-fit; a request can fail under fragmentation even if `live` is low).
    ///
    /// # Safety
    /// Must be called only after `init`.
    pub unsafe fn alloc(&mut self, size: usize, align: usize) -> usize {
        let align = if align < ALIGN { ALIGN } else { align };
        let need = round_block(size);

        let mut prev = 0usize; // 0 == "before head"
        let mut cur = self.head;
        while cur != 0 {
            let csz = Self::size_of(cur);
            let cnext = Self::next_of(cur);
            let payload = align_up(cur, align);
            let cend = cur + csz;
            // Does `need` bytes at the aligned offset fit inside this block?
            if payload + need <= cend {
                let pre = payload - cur; // gap before payload (multiple of 16)
                let post = cend - (payload + need); // gap after payload

                // Replace `cur` in the list with its leftover gaps, preserving
                // ascending-address order:  [pre] -> [post] -> cnext.
                let mut chain = cnext;
                if post >= HDR {
                    let paddr = payload + need;
                    Self::write(paddr, post, chain);
                    chain = paddr;
                }
                if pre >= HDR {
                    Self::write(cur, pre, chain);
                    chain = cur;
                }
                if prev == 0 {
                    self.head = chain;
                } else {
                    Self::set_next(prev, chain);
                }

                self.live += need;
                return payload;
            }
            prev = cur;
            cur = cnext;
        }
        0
    }

    /// Free a region previously returned by `alloc`, coalescing with adjacent
    /// free blocks. `size` is the original `Layout` size (rounded identically).
    ///
    /// # Safety
    /// `ptr` must have come from `alloc` on this heap and not been freed since.
    pub unsafe fn dealloc(&mut self, ptr: usize, size: usize) {
        if ptr == 0 {
            return;
        }
        let need = round_block(size);
        self.live -= need;

        // Locate prev (largest free addr < ptr) and cur (smallest free addr > ptr).
        let mut prev = 0usize;
        let mut cur = self.head;
        while cur != 0 && cur < ptr {
            prev = cur;
            cur = Self::next_of(cur);
        }

        let prev_adj = prev != 0 && prev + Self::size_of(prev) == ptr;
        let cur_adj = cur != 0 && ptr + need == cur;

        match (prev_adj, cur_adj) {
            (true, true) => {
                // prev absorbs the freed region AND the following block.
                let newsz = Self::size_of(prev) + need + Self::size_of(cur);
                Self::write(prev, newsz, Self::next_of(cur));
            }
            (true, false) => {
                // Extend prev; prev.next already points at cur.
                Self::set_size(prev, Self::size_of(prev) + need);
            }
            (false, true) => {
                // New block at ptr swallows the following adjacent block.
                let newsz = need + Self::size_of(cur);
                Self::write(ptr, newsz, Self::next_of(cur));
                if prev == 0 {
                    self.head = ptr;
                } else {
                    Self::set_next(prev, ptr);
                }
            }
            (false, false) => {
                Self::write(ptr, need, cur);
                if prev == 0 {
                    self.head = ptr;
                } else {
                    Self::set_next(prev, ptr);
                }
            }
        }
    }
}
