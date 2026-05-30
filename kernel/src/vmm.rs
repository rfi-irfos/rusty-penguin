// Page flags
pub const PTE_PRESENT:  u64 = 1 << 0;
pub const PTE_WRITABLE: u64 = 1 << 1;
pub const PTE_USER:     u64 = 1 << 2;
pub const PTE_HUGE:     u64 = 1 << 7;  // in PD: 2MB page

// User-space virtual addresses for the ring-3 demo
pub const USER_CODE_VIRT: u64 = 0x0040_1000;
// Stack grows down from here. Placed high in the identity-mapped 0–64 MiB
// region, ABOVE the ring-3 program's .bss/heap (which starts at 0x400000 and,
// with a multi-MiB heap + high-res backbuffer, can extend well past 8 MiB).
// The old value (0x802000) sat *inside* the heap region and corrupted once a
// large framebuffer backbuffer was allocated — that was the high-res crash.
pub const USER_STACK_TOP: u64 = 0x03F0_0000;  // ~63 MiB, above heap, below 64 MiB map
pub const USER_STACK_PAGES: usize = 16;        // 16 × 4KiB = 64 KiB

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

/// Map an arbitrary physical MMIO range as identity-mapped (virt == phys), 4 KiB pages.
/// Used for the framebuffer and other device memory that lives above the identity-mapped range.
pub fn map_mmio_range(phys_start: u64, size: u64) {
    const PAGE: u64 = 4096;
    let start = phys_start & !(PAGE - 1);
    let end   = (phys_start + size + PAGE - 1) & !(PAGE - 1);
    let mut addr = start;
    while addr < end {
        unsafe { map_page(addr, addr, PTE_PRESENT | PTE_WRITABLE | PTE_USER); }
        addr += PAGE;
    }
}

/// Read the current CR3 (physical base of the active PML4). Public for the
/// scheduler, which records each task's address space.
pub fn current_cr3() -> u64 { cr3() }

/// Switch the active address space by loading `pml4_phys` into CR3. The new PML4
/// MUST map the kernel (code/stack/data) and any MMIO the kernel touches, or the
/// next instruction faults. `new_address_space` guarantees this by sharing the
/// kernel's low 512 GiB (PML4[0]).
pub unsafe fn switch_address_space(pml4_phys: u64) {
    core::arch::asm!("mov cr3, {}", in(reg) pml4_phys & !0xFFF, options(nostack));
}

/// Create a fresh address space (PML4) for a new process. It SHARES the kernel's
/// entire low 512 GiB (PML4[0] — identity map, framebuffer, kernel code/stack),
/// so the kernel keeps running after a CR3 switch. Private per-process mappings
/// go in PML4[1..] (virtual addresses ≥ 512 GiB) so they don't disturb the
/// shared kernel region. Returns the new PML4's physical address.
pub unsafe fn new_address_space() -> Option<u64> {
    let pml4 = crate::pmm::alloc_frame()?;
    core::ptr::write_bytes(pml4 as *mut u8, 0, 4096);
    let kpml4 = cr3();
    write64(pml4, 0, read64(kpml4, 0)); // share kernel low half (PML4[0])
    Some(pml4)
}

/// Map one 4 KiB page into a SPECIFIC address space (`pml4_phys`), rather than
/// the active one. Used to populate a process's private region before switching
/// to it. All intermediate tables get USER|WRITABLE|PRESENT.
pub unsafe fn map_page_in(pml4_phys: u64, virt: u64, phys: u64, flags: u64) -> bool {
    let fi = PTE_PRESENT | PTE_WRITABLE | PTE_USER;
    let pdpt = match descend(pml4_phys, pml4_idx(virt), fi) { Some(a) => a, None => return false };
    let pd   = match descend(pdpt,      pdpt_idx(virt), fi) { Some(a) => a, None => return false };
    let pt   = match descend(pd,        pd_idx(virt),   fi) { Some(a) => a, None => return false };
    write64(pt, pt_idx(virt), phys | flags | PTE_PRESENT);
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

        // Fix PD[0..N]: boot.s maps these as huge pages without PTE_USER.
        // Add PTE_USER to existing entries; create new entries for any gaps.
        let entries = (limit_mib / 2).min(511);
        for i in 0..=entries {
            let entry = read64(pd, i);
            if entry & PTE_PRESENT != 0 {
                write64(pd, i, entry | PTE_USER);
            } else {
                let phys = i as u64 * 2 * 1024 * 1024;
                write64(pd, i, phys | PTE_HUGE | PTE_WRITABLE | PTE_USER | PTE_PRESENT);
            }
        }
        flush_tlb();
    }
}
