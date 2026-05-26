// Page flags
pub const PTE_PRESENT:  u64 = 1 << 0;
pub const PTE_WRITABLE: u64 = 1 << 1;
pub const PTE_USER:     u64 = 1 << 2;
pub const PTE_HUGE:     u64 = 1 << 7;  // in PD: 2MB page

// User-space virtual addresses for the ring-3 demo
pub const USER_CODE_VIRT: u64 = 0x0040_1000;
pub const USER_STACK_TOP: u64 = 0x0080_2000;  // stack grows down from here
pub const USER_STACK_PAGES: usize = 4;         // 4 × 4KiB = 16 KiB

fn cr3() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) v, options(nostack, readonly)); }
    v & !0xFFF
}

fn flush_tlb() {
    let v = cr3();
    unsafe { core::arch::asm!("mov cr3, {}", in(reg) v, options(nostack)); }
}

unsafe fn read64(phys: u64, idx: usize) -> u64 {
    *((phys as usize + idx * 8) as *const u64)
}
unsafe fn write64(phys: u64, idx: usize, val: u64) {
    *((phys as usize + idx * 8) as *mut u64) = val;
}

fn pml4_idx(v: u64) -> usize { ((v >> 39) & 0x1FF) as usize }
fn pdpt_idx(v: u64) -> usize { ((v >> 30) & 0x1FF) as usize }
fn pd_idx(v: u64)   -> usize { ((v >> 21) & 0x1FF) as usize }
fn pt_idx(v: u64)   -> usize { ((v >> 12) & 0x1FF) as usize }

/// Walk a page table level, allocating a new frame if the entry is absent.
/// Returns the physical base address of the next level, or None on error.
unsafe fn descend(table: u64, idx: usize, flags: u64) -> Option<u64> {
    let entry = read64(table, idx);
    if entry & PTE_PRESENT != 0 {
        if entry & PTE_HUGE != 0 { return None; }  // huge page — don't walk into it
        return Some(entry & !0xFFF);
    }
    let frame = crate::pmm::alloc_frame()?;
    // Zero the new page table frame (it's in the identity-mapped region)
    core::ptr::write_bytes(frame as *mut u8, 0, 4096);
    write64(table, idx, frame | flags);
    Some(frame)
}

/// Map one 4KiB virtual page to a physical frame.
pub unsafe fn map_page(virt: u64, phys: u64, flags: u64) -> bool {
    let flags_int = PTE_PRESENT | PTE_WRITABLE | PTE_USER;

    let pml4 = cr3();
    let pdpt = match descend(pml4, pml4_idx(virt), flags_int) { Some(a) => a, None => return false };
    let pd   = match descend(pdpt, pdpt_idx(virt), flags_int) { Some(a) => a, None => return false };
    let pt   = match descend(pd,   pd_idx(virt),   flags_int) { Some(a) => a, None => return false };

    write64(pt, pt_idx(virt), phys | flags | PTE_PRESENT);
    core::arch::asm!("invlpg [{0}]", in(reg) virt as usize, options(nostack));
    true
}

/// Extend the boot identity map from 2 MiB up to `limit_mib` MiB (max 1022).
/// Boot.s sets PML4[0]/PDPT[0]/PD[0] with only PRESENT|WRITABLE — no USER bit.
/// The x86 page table walk checks U/S at EVERY level, so all three levels must
/// have PTE_USER before ring-3 can touch anything in this range.
pub fn extend_identity_map(limit_mib: usize) {
    unsafe {
        let pml4 = cr3();

        // Fix PML4[0]: add PTE_USER (boot.s used 0x3 — present+writable only)
        let pml4_e = read64(pml4, 0);
        write64(pml4, 0, pml4_e | PTE_USER);
        let pdpt = pml4_e & !0xFFF;

        // Fix PDPT[0]: same issue
        let pdpt_e = read64(pdpt, 0);
        write64(pdpt, 0, pdpt_e | PTE_USER);
        let pd = pdpt_e & !0xFFF;

        // Fix PD[0]: boot.s set this huge page with 0x83 (no USER)
        let pd0 = read64(pd, 0);
        if pd0 & PTE_PRESENT != 0 {
            write64(pd, 0, pd0 | PTE_USER);
        }

        // Fill PD[1..N] as new 2 MiB USER huge pages
        let entries = (limit_mib / 2).min(511);
        for i in 1..=entries {
            if read64(pd, i) & PTE_PRESENT == 0 {
                let phys = i as u64 * 2 * 1024 * 1024;
                write64(pd, i, phys | PTE_HUGE | PTE_WRITABLE | PTE_USER | PTE_PRESENT);
            }
        }
        flush_tlb();
    }
}
