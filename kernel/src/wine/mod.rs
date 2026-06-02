//! The native Wine subsystem for Rusty Penguin.
//! Rebuilt from scratch to provide a first-class Windows environment.

pub mod pe;

use crate::serial;
use crate::vmm;
use crate::pmm;
use crate::elf;
use core::mem::size_of;

// ── Per-process ABI mode ─────────────────────────────────────────────────────
static mut WINE_ABI: bool = false;

#[inline]
pub fn is_wine() -> bool { unsafe { WINE_ABI } }

#[inline]
pub fn set_wine(v: bool) { unsafe { WINE_ABI = v; } }

/// Loads a Windows PE binary and prepares it for execution.
/// Returns (entry_point, image_base)
pub fn load_pe(data: &[u8]) -> Option<(u64, u64)> {
    if !pe::is_pe(data) {
        serial::write_str("  [wine] Invalid PE signature\n");
        return None;
    }

    let dos = unsafe { &*(data.as_ptr() as *const pe::ImageDosHeader) };
    let nt_off = dos.e_lfanew as usize;
    let nt = unsafe { &*(data.as_ptr().add(nt_off) as *const pe::ImageNtHeaders64) };

    let image_base = nt.optional_header.image_base;
    let entry_point = image_base + nt.optional_header.address_of_entry_point as u64;

    serial::write_str("  [wine] Loading PE image at 0x");
    serial::write_hex_u32(image_base as u32);
    serial::write_str(" entry 0x");
    serial::write_hex_u32(entry_point as u32);
    serial::write_str("\n");

    // Map sections
    let section_headers_off = nt_off + size_of::<pe::ImageNtHeaders64>();
    let num_sections = nt.file_header.number_of_sections as usize;
    
    for i in 0..num_sections {
        let sh_off = section_headers_off + i * size_of::<pe::ImageSectionHeader>();
        let sh = unsafe { &*(data.as_ptr().add(sh_off) as *const pe::ImageSectionHeader) };
        
        let name = core::str::from_utf8(&sh.name).unwrap_or("unknown");
        serial::write_str("  [wine] Mapping section: ");
        serial::write_str(name);
        serial::write_str("\n");

        let dest_va = image_base + sh.virtual_address as u64;
        let size = sh.virtual_size as usize;
        let raw_data_ptr = unsafe { data.as_ptr().add(sh.pointer_to_raw_data as usize) };
        let raw_size = sh.size_of_raw_data as usize;

        // Allocate and map pages
        let num_pages = (size + 4095) / 4096;
        for p in 0..num_pages {
            let va = dest_va + (p * 4096) as u64;
            if let Some(phys) = pmm::alloc_frame() {
                let pfw = vmm::PTE_PRESENT | vmm::PTE_WRITABLE | vmm::PTE_USER;
                unsafe { vmm::map_page_in(vmm::current_cr3(), va, phys, pfw); }
                
                // Clear page
                unsafe { core::ptr::write_bytes(vmm::phys_to_virt(phys) as *mut u8, 0, 4096); }
                
                // Copy raw data if available
                let offset = p * 4096;
                if offset < raw_size {
                    let to_copy = (raw_size - offset).min(4096);
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            raw_data_ptr.add(offset),
                            vmm::phys_to_virt(phys) as *mut u8,
                            to_copy
                        );
                    }
                }
            } else {
                serial::write_str("  [wine] Out of memory mapping section\n");
                return None;
            }
        }
    }

    Some((entry_point, image_base))
}

extern "C" {
    static _lx_a4: u64;
    static _lx_a5: u64;
    static _lx_a6: u64;
    static _user_rip_save: u64;
}

#[inline]
fn extra_args() -> (u64, u64, u64) {
    let (a4, a5, a6): (u64, u64, u64);
    unsafe {
        core::arch::asm!(
            "mov {0}, qword ptr [rip + _lx_a4]",
            "mov {1}, qword ptr [rip + _lx_a5]",
            "mov {2}, qword ptr [rip + _lx_a6]",
            out(reg) a4, out(reg) a5, out(reg) a6,
            options(nostack, readonly, preserves_flags),
        );
    }
    (a4, a5, a6)
}

/// The Windows-native syscall entry point.
pub fn syscall_handler(nr: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64) -> u64 {
    let (w_a1, w_a3, w_a4) = extra_args();
    let w_a2 = _a3; 

    serial::write_str("  [wine] Syscall nr=0x");
    serial::write_hex_u32(nr as u32);
    serial::write_str(" a1=0x");
    serial::write_hex_u32(w_a1 as u32);
    serial::write_str(" a2=0x");
    serial::write_hex_u32(w_a2 as u32);
    serial::write_str("\n");
    
    match nr {
        // NtTerminateProcess
        0x2c => {
            serial::write_str("  [wine] NtTerminateProcess\n");
            loop { unsafe { core::arch::asm!("hlt"); } }
        }
        _ => 0xC0000002 // STATUS_NOT_IMPLEMENTED
    }
}

/// Windows TEB (Thread Environment Block) and PEB (Process Environment Block) stubs.
/// On x86_64, GS:base points to the TEB.
#[repr(C, align(4096))]
struct WinTeb {
    _reserved1: [u8; 48],
    peb_ptr: u64,
    _reserved2: [u8; 4040],
}

#[repr(C, align(4096))]
struct WinPeb {
    _reserved1: [u8; 16],
    image_base: u64,
    _reserved2: [u8; 4072],
}

pub fn enter_wine(entry: u64, image_base: u64) -> ! {
    // 1. Allocate TEB and PEB
    let teb_phys = pmm::alloc_frame().expect("teb alloc failed");
    let peb_phys = pmm::alloc_frame().expect("peb alloc failed");
    
    let teb_va = 0x0000_7FF0_0000_0000;
    let peb_va = 0x0000_7FF0_0000_1000;
    
    let pfw = vmm::PTE_PRESENT | vmm::PTE_WRITABLE | vmm::PTE_USER;
    unsafe {
        vmm::map_page_in(vmm::current_cr3(), teb_va, teb_phys, pfw);
        vmm::map_page_in(vmm::current_cr3(), peb_va, peb_phys, pfw);
        
        let teb = vmm::phys_to_virt(teb_phys) as *mut WinTeb;
        let peb = vmm::phys_to_virt(peb_phys) as *mut WinPeb;
        
        core::ptr::write_bytes(teb as *mut u8, 0, 4096);
        core::ptr::write_bytes(peb as *mut u8, 0, 4096);
        
        (*teb).peb_ptr = peb_va;
        (*peb).image_base = image_base;
    }

    // 2. Set GS_BASE to TEB
    const IA32_GS_BASE: u32 = 0xC000_0101;
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_GS_BASE,
            in("eax") teb_va as u32,
            in("edx") (teb_va >> 32) as u32,
            options(nostack)
        );
    }

    // 3. Set ABI mode
    set_wine(true);

    serial::write_str("  [wine] Entering ring-3 PE @ 0x");
    serial::write_hex_u32(entry as u32);
    serial::write_str("\n");

    // 4. IRETQ into ring-3
    let user_rsp = vmm::USER_STACK_TOP;
    unsafe {
        core::arch::asm!(
            "push 0x1B",      // SS
            "push {0}",       // RSP
            "pushfq",
            "pop rax",
            "or rax, 0x202",  // IF=1
            "push rax",
            "push 0x23",      // CS
            "push {1}",       // RIP
            "xor rax, rax", "xor rbx, rbx", "xor rcx, rcx", "xor rdx, rdx",
            "xor rsi, rsi", "xor rdi, rdi", "xor rbp, rbp",
            "xor r8, r8", "xor r9, r9", "xor r10, r10", "xor r11, r11",
            "xor r12, r12", "xor r13, r13", "xor r14, r14", "xor r15, r15",
            "iretq",
            in(reg) user_rsp,
            in(reg) entry,
            options(noreturn)
        );
    }
}
