//! The native Wine subsystem for Rusty Penguin.
//! Rebuilt from scratch to provide a first-class Windows environment.

pub mod pe;

use crate::serial;
use crate::vmm;
use crate::pmm;
use crate::ramfs;
use core::mem::size_of;

// ── Per-process ABI mode ─────────────────────────────────────────────────────
static mut WINE_ABI: bool = false;

#[inline]
pub fn is_wine() -> bool { unsafe { WINE_ABI } }

#[inline]
pub fn set_wine(v: bool) { unsafe { WINE_ABI = v; } }

/// Track loaded DLLs to avoid double-loading.
struct LoadedDll {
    name: &'static str,
    base: u64,
}
static mut LOADED_DLLS: [Option<LoadedDll>; 16] = [None; 16];

fn find_loaded_dll(name: &str) -> Option<u64> {
    unsafe {
        for dll in LOADED_DLLS.iter().flatten() {
            if dll.name.eq_ignore_ascii_case(name) { return Some(dll.base); }
        }
    }
    None
}

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
            }
        }
    }

    // Process Imports (simplified for ntdll only in Brick 7)
    let import_dir = nt.optional_header.data_directory[1];
    if import_dir.virtual_address != 0 {
        // IAT patching logic would go here
        serial::write_str("  [wine] Resolving imports...\n");
    }

    Some((entry_point, image_base))
}

extern "C" {
    static _lx_a4: u64;
    static _lx_a5: u64;
    static _lx_a6: u64;
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

pub fn syscall_handler(nr: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64) -> u64 {
    let (w_a1, w_a3, w_a4) = extra_args();
    let w_a2 = _a3; 

    serial::write_str("  [wine] Syscall nr=0x");
    serial::write_hex_u32(nr as u32);
    serial::write_str("\n");
    
    match nr {
        // NtYieldExecution
        0x01 => {
            crate::sched::yield_();
            0
        }
        // NtWaitForSingleObject stub
        0x04 => {
            serial::write_str("  [wine] NtWaitForSingleObject stub\n");
            0 // STATUS_SUCCESS
        }
        0x2c => {
            serial::write_str("  [wine] NtTerminateProcess\n");
            loop { unsafe { core::arch::asm!("hlt"); } }
        }
        _ => 0xC0000002 // STATUS_NOT_IMPLEMENTED
    }
}

/// Windows Context structure for x64 (simplified for SEH).
#[repr(C, align(16))]
pub struct WinContext {
    pub p1_home: u64, pub p2_home: u64, pub p3_home: u64, pub p4_home: u64,
    pub p5_home: u64, pub p6_home: u64,
    pub context_flags: u32,
    pub mx_csr: u32,
    pub seg_cs: u16, pub seg_ds: u16, pub seg_es: u16, pub seg_fs: u16,
    pub seg_gs: u16, pub seg_ss: u16,
    pub eflags: u32,
    pub dr0: u64, pub dr1: u64, pub dr2: u64, pub dr3: u64,
    pub dr6: u64, pub dr7: u64,
    pub rax: u64, pub rcx: u64, pub rdx: u64, pub rbx: u64,
    pub rsp: u64, pub rbp: u64, pub rsi: u64, pub rdi: u64,
    pub r8:  u64, pub r9:  u64, pub r10: u64, pub r11: u64,
    pub r12: u64, pub r13: u64, pub r14: u64, pub r15: u64,
    pub rip: u64,
    // Header + XMM omitted for brevity in this brick
}

/// Deliver a Windows exception (e.g. #GP, #PF) to the user-mode SEH handler.
pub fn deliver_exception(code: u32, addr: u64) {
    serial::write_str("  [wine] Delivering exception 0x");
    serial::write_hex_u32(code);
    serial::write_str(" at 0x0");
    serial::write_hex_u32(addr as u32);
    serial::write_str("\n");
    // SEH unwinding and KiUserExceptionDispatcher call logic will follow
}

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
    const IA32_GS_BASE: u32 = 0xC000_0101;
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") IA32_GS_BASE, in("eax") teb_va as u32, in("edx") (teb_va >> 32) as u32, options(nostack));
    }
    set_wine(true);
    let user_rsp = vmm::USER_STACK_TOP;
    unsafe {
        core::arch::asm!(
            "push 0x1B", "push {0}", "pushfq", "pop rax", "or rax, 0x202", "push rax", "push 0x23", "push {1}",
            "xor rax, rax", "xor rbx, rbx", "xor rcx, rcx", "xor rdx, rdx", "xor rsi, rsi", "xor rdi, rdi", "xor rbp, rbp",
            "xor r8, r8", "xor r9, r9", "xor r10, r10", "xor r11, r11", "xor r12, r12", "xor r13, r13", "xor r14, r14", "xor r15, r15",
            "iretq",
            in(reg) user_rsp, in(reg) entry, options(noreturn)
        );
    }
}
