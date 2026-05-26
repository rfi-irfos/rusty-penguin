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
/// Boot.s only maps PD[0] (0–2 MiB huge page).  We add PD[1..N] as additional
/// 2 MiB huge pages so PMM frames above 2 MiB are reachable.
pub fn extend_identity_map(limit_mib: usize) {
    unsafe {
        let pml4 = cr3();
        let pdpt = read64(pml4, 0) & !0xFFF;
        let pd   = read64(pdpt,  0) & !0xFFF;

        let entries = (limit_mib / 2).min(511);  // PD[0] already set, fill [1..entries]
        for i in 1..=entries {
            if read64(pd, i) & PTE_PRESENT == 0 {
                let phys = i as u64 * 2 * 1024 * 1024;
                write64(pd, i, phys | PTE_HUGE | PTE_WRITABLE | PTE_PRESENT);
            }
        }
        flush_tlb();
    }
}
