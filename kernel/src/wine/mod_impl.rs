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
#[derive(Clone, Copy)]
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

fn register_dll(name: &'static str, base: u64) {
    unsafe {
        for dll in LOADED_DLLS.iter_mut() {
            if dll.is_none() {
                *dll = Some(LoadedDll { name, base });
                return;
            }
        }
    }
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

        let num_pages = (size + 4095) / 4096;
        for p in 0..num_pages {
            let va = dest_va + (p * 4096) as u64;
            if let Some(phys) = pmm::alloc_frame() {
                let pfw = vmm::PTE_PRESENT | vmm::PTE_WRITABLE | vmm::PTE_USER;
                unsafe { vmm::map_page_in(vmm::current_cr3(), va, phys, pfw); }
                unsafe { core::ptr::write_bytes(vmm::phys_to_virt(phys) as *mut u8, 0, 4096); }
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

    // Process Imports
    let import_dir = nt.optional_header.data_directory[1].clone();
    if import_dir.virtual_address != 0 {
        resolve_imports(image_base, import_dir.virtual_address as u64);
    }

    Some((entry_point, image_base))
}

fn resolve_imports(image_base: u64, import_rva: u64) {
    serial::write_str("  [wine] Resolving imports...\n");
    let mut desc_ptr = (image_base + import_rva) as *const pe::ImageImportDescriptor;
    
    unsafe {
        while (*desc_ptr).name != 0 {
            let name_ptr = (image_base + (*desc_ptr).name as u64) as *const u8;
            let mut len = 0;
            while *name_ptr.add(len) != 0 { len += 1; }
            let dll_name = core::str::from_utf8(core::slice::from_raw_parts(name_ptr, len)).unwrap_or("unknown");
            
            serial::write_str("  [wine] Import from: ");
            serial::write_str(dll_name);
            serial::write_str("\n");
            
            let dll_base = if let Some(base) = find_loaded_dll(dll_name) {
                base
            } else {
                // For Brick 15, we only support ntdll.dll which is built-in or pre-loaded.
                // In a real system, we'd call load_library(dll_name) here.
                0
            };
            
            if dll_base != 0 {
                let mut thunk = (image_base + (*desc_ptr).first_thunk as u64) as *mut u64;
                let mut orig_thunk = if (*desc_ptr).characteristics != 0 {
                    (image_base + (*desc_ptr).characteristics as u64) as *const u64
                } else {
                    thunk as *const u64
                };
                
                while *orig_thunk != 0 {
                    if (*orig_thunk & (1 << 63)) == 0 { // Import by name
                        let name_data = (image_base + (*orig_thunk & 0x7FFFFFFF) as u64 + 2) as *const u8;
                        let mut nlen = 0;
                        while *name_data.add(nlen) != 0 { nlen += 1; }
                        let func_name = core::str::from_utf8(core::slice::from_raw_parts(name_data, nlen)).unwrap_or("");
                        
                        if let Some(func_ptr) = resolve_export(dll_base, func_name) {
                            *thunk = func_ptr;
                        }
                    }
                    thunk = thunk.add(1);
                    orig_thunk = orig_thunk.add(1);
                }
            }
            
            desc_ptr = desc_ptr.add(1);
        }
    }
}

fn resolve_export(image_base: u64, func_name: &str) -> Option<u64> {
    let dos = unsafe { &*(image_base as *const pe::ImageDosHeader) };
    let nt = unsafe { &*((image_base + dos.e_lfanew as u64) as *const pe::ImageNtHeaders64) };
    let export_dir_rva = nt.optional_header.data_directory[0].virtual_address;
    if export_dir_rva == 0 { return None; }
    
    let export_dir = unsafe { &*((image_base + export_dir_rva as u64) as *const pe::ImageExportDirectory) };
    let names = unsafe { core::slice::from_raw_parts((image_base + export_dir.address_of_names as u64) as *const u32, export_dir.number_of_names as usize) };
    let ordinals = unsafe { core::slice::from_raw_parts((image_base + export_dir.address_of_name_ordinals as u64) as *const u16, export_dir.number_of_names as usize) };
    let functions = unsafe { core::slice::from_raw_parts((image_base + export_dir.address_of_functions as u64) as *const u32, export_dir.number_of_functions as usize) };

    for i in 0..names.len() {
        let name_ptr = (image_base + names[i] as u64) as *const u8;
        let mut len = 0;
        while unsafe { *name_ptr.add(len) } != 0 { len += 1; }
        let name = unsafe { core::str::from_utf8(core::slice::from_raw_parts(name_ptr, len)).unwrap_or("") };
        
        if name == func_name {
            let ordinal = ordinals[i] as usize;
            return Some(image_base + functions[ordinal] as u64);
        }
    }
    None
}

extern "C" {
    static _lx_a4: u64;
    static _lx_a5: u64;
    static _lx_a6: u64;
    static _user_rsp: u64;
}

#[inline]
fn extra_args() -> (u64, u64, u64, u64) {
    let (a4, a5, a6, ursp): (u64, u64, u64, u64);
    unsafe {
        core::arch::asm!(
            "mov {0}, qword ptr [rip + _lx_a4]",
            "mov {1}, qword ptr [rip + _lx_a5]",
            "mov {2}, qword ptr [rip + _lx_a6]",
            "mov {3}, qword ptr [rip + _user_rsp]",
            out(reg) a4, out(reg) a5, out(reg) a6, out(reg) ursp,
            options(nostack, readonly, preserves_flags),
        );
    }
    (a4, a5, a6, ursp)
}

pub fn syscall_handler(nr: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64) -> u64 {
    let (a4, a5, a6, ursp) = extra_args();
    let w_a1 = a4;
    let w_a2 = _a3; 
    let w_a3 = a5; 
    let w_a4 = a6; 
    let w_a5 = unsafe { *(ursp as *const u64).add(4) }; 
    let w_a6 = unsafe { *(ursp as *const u64).add(5) }; 

    if is_dormant_syscall(nr) { return 0; }

    serial::write_str("  [wine] Syscall nr=0x");
    serial::write_hex_u32(nr as u32);
    serial::write_str("\n");
    
    match nr {
        0x01 => { crate::sched::yield_(); 0 }
        // NtQuerySystemTime (Brick 19)
        0x57 => unsafe {
            let time_ptr = w_a1 as *mut u64;
            if !time_ptr.is_null() {
                // Return rough Windows-epoch time
                *time_ptr = 132645600000000000 + (crate::idt::ticks() * 100000); 
            }
            0
        }
        // NtDelayExecution (Brick 19)
        0x34 => {
            crate::sched::yield_();
            0
        }
        // NtDeviceIoControlFile (Brick 28)
        0x03 => {
            serial::write_str("  [wine] NtDeviceIoControlFile handle=0x");
            serial::write_hex_u32(w_a1 as u32);
            serial::write_str(" ioctl=0x");
            serial::write_hex_u32(w_a3 as u32);
            serial::write_str("\n");
            0 // STATUS_SUCCESS
        }
        // NtCreateMutant (Brick 18)
        0x1b => {
            serial::write_str("  [wine] NtCreateMutant stub\n");
            alloc_handle(WinObject::Mutant)
        }
        // NtReleaseMutant (Brick 18)
        0x21 => {
            serial::write_str("  [wine] NtReleaseMutant stub\n");
            0
        }
        // NtCreateSemaphore (Brick 18)
        0x1c => {
            serial::write_str("  [wine] NtCreateSemaphore stub\n");
            alloc_handle(WinObject::Semaphore)
        }
        // NtCreateSection (Brick 20)
        0x4a => {
            serial::write_str("  [wine] NtCreateSection stub\n");
            alloc_handle(WinObject::Section)
        }
        // NtMapViewOfSection (Brick 20)
        0x28 => {
            serial::write_str("  [wine] NtMapViewOfSection stub\n");
            0
        }
        0x18 => unsafe {
            let base_address_ptr = w_a2 as *mut u64;
            let region_size_ptr = w_a4 as *mut u64;
            if base_address_ptr.is_null() || region_size_ptr.is_null() { return 0xC000000D; }
            let mut base = *base_address_ptr;
            let size = *region_size_ptr;
            if base == 0 {
                base = crate::linux::mmap_cur();
                crate::linux::set_mmap_cur(base + ((size + 4095) & !4095));
            }
            let num_pages = (size + 4095) / 4096;
            let cr3 = vmm::current_cr3();
            let pfw = vmm::PTE_PRESENT | vmm::PTE_WRITABLE | vmm::PTE_USER;
            for p in 0..num_pages {
                let va = (base + p * 4096) & !4095;
                if let Some(phys) = pmm::alloc_frame() {
                    vmm::map_page_in(cr3, va, phys, pfw);
                    core::ptr::write_bytes(vmm::phys_to_virt(phys) as *mut u8, 0, 4096);
                }
            }
            *base_address_ptr = base;
            *region_size_ptr = num_pages * 4096;
            0 
        }
        0x1e => 0, 
        0x04 => {
            serial::write_str("  [wine] NtWaitForSingleObject handle=0x");
            serial::write_hex_u32(w_a1 as u32);
            serial::write_str("\n");
            0 
        }
        0x0f => {
            let handle = w_a1;
            if free_handle(handle) { 0 } else { 0xC0000008 } 
        }
        // NtReadFile
        0x06 => {
            serial::write_str("  [wine] NtReadFile handle=0x");
            serial::write_hex_u32(w_a1 as u32);
            serial::write_str("\n");
            0 
        }
        // NtWriteFile
        0x08 => {
            serial::write_str("  [wine] NtWriteFile handle=0x");
            serial::write_hex_u32(w_a1 as u32);
            serial::write_str("\n");
            0 
        }
        // NtOpenFile
        0x33 => {
            serial::write_str("  [wine] NtOpenFile stub\n");
            alloc_handle(WinObject::File)
        }
        // NtCreateFile
        0x55 => {
            serial::write_str("  [wine] NtCreateFile stub\n");
            alloc_handle(WinObject::File)
        }
        // NtQueryInformationFile (Brick 21)
        0x22 => {
            serial::write_str("  [wine] NtQueryInformationFile stub\n");
            0 
        }
        // NtSetInformationFile (Brick 21)
        0x23 => {
            serial::write_str("  [wine] NtSetInformationFile stub\n");
            0 
        }
        // NtOpenThread (Brick 22)
        0x4e => {
            serial::write_str("  [wine] NtOpenThread stub\n");
            alloc_handle(WinObject::Thread)
        }
        // NtTerminateThread (Brick 22)
        0x50 => {
            serial::write_str("  [wine] NtTerminateThread stub\n");
            0
        }
        // NtOpenProcess (Brick 23)
        0x26 => {
            serial::write_str("  [wine] NtOpenProcess stub\n");
            alloc_handle(WinObject::Process)
        }
        // NtQueryInformationProcess (Brick 23)
        0x19 => {
            serial::write_str("  [wine] NtQueryInformationProcess stub\n");
            0
        }
        // NtQuerySystemInformation (Brick 24)
        0x36 => {
            serial::write_str("  [wine] NtQuerySystemInformation stub\n");
            0
        }
        // NtOpenProcessToken (Brick 25)
        0x3e => {
            serial::write_str("  [wine] NtOpenProcessToken stub\n");
            alloc_handle(WinObject::Token)
        }
        // NtOutputDebugString (Brick 26)
        0x3c => {
            serial::write_str("  [wine-dbg] Debug message\n");
            0
        }
        // NtUser/Gdi syscall stubs (Brick 27)
        0x1000..=0x10FF => {
            serial::write_str("  [wine] NtUser/Gdi syscall stub\n");
            0
        }
        // NtCreateNamedPipeFile
        0x5c => {
            serial::write_str("  [wine] NtCreateNamedPipeFile stub\n");
            alloc_handle(WinObject::Pipe)
        }
        // NtOpenKey
        0x12 => {
            serial::write_str("  [wine] NtOpenKey stub\n");
            alloc_handle(WinObject::Key)
        }
        // NtCreateKey
        0x1d => {
            serial::write_str("  [wine] NtCreateKey stub\n");
            alloc_handle(WinObject::Key)
        }
        // NtQueryValueKey
        0x15 => {
            serial::write_str("  [wine] NtQueryValueKey stub\n");
            0 
        }
        0x2c => {
            serial::write_str("  [wine] NtTerminateProcess\n");
            loop { unsafe { core::arch::asm!("hlt"); } }
        }
        _ => 0xC0000002 
    }
}

fn is_dormant_syscall(nr: u64) -> bool {
    if nr >= 0x100 && nr <= 0x1FF { return true; }
    if nr == 0x2A || nr == 0x2B { return true; }
    false
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
}

pub fn deliver_exception(code: u32, addr: u64) {
    serial::write_str("  [wine] Delivering exception 0x");
    serial::write_hex_u32(code);
    serial::write_str(" at 0x0");
    serial::write_hex_u32(addr as u32);
    serial::write_str("\n");
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

#[derive(Clone, Copy)]
pub enum WinObject {
    None,
    Process,
    Thread,
    Event,
    File,
    Pipe,
    Key,
    Mutant,
    Semaphore,
    Section,
    Token,
    Socket,
}

const MAX_HANDLES: usize = 256;
static mut HANDLE_TABLE: [WinObject; MAX_HANDLES] = [WinObject::None; MAX_HANDLES];

pub fn alloc_handle(obj: WinObject) -> u64 {
    unsafe {
        for i in 1..MAX_HANDLES {
            if let WinObject::None = HANDLE_TABLE[i] {
                HANDLE_TABLE[i] = obj;
                return (i * 4) as u64; 
            }
        }
    }
    0
}

pub fn free_handle(h: u64) -> bool {
    let idx = (h / 4) as usize;
    if idx < MAX_HANDLES {
        unsafe {
            if let WinObject::None = HANDLE_TABLE[idx] { return false; }
            HANDLE_TABLE[idx] = WinObject::None;
            return true;
        }
    }
    false
}
