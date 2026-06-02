//! Portable Executable (PE) parser and loader for the native Wine subsystem.
//! This allows Rusty Penguin to load unmodified Windows x86_64 binaries.

use core::mem::size_of;

#[repr(C, packed)]
pub struct ImageDosHeader {
    pub e_magic: u16,      // Magic number 'MZ'
    pub e_cblp: u16,       // Bytes on last page of file
    pub e_cp: u16,        // Pages in file
    pub e_crlc: u16,      // Relocations
    pub e_cparhdr: u16,   // Size of header in paragraphs
    pub e_minalloc: u16,  // Minimum extra paragraphs needed
    pub e_maxalloc: u16,  // Maximum extra paragraphs needed
    pub e_ss: u16,        // Initial (relative) SS value
    pub e_sp: u16,        // Initial SP value
    pub e_csum: u16,      // Checksum
    pub e_ip: u16,        // Initial IP value
    pub e_cs: u16,        // Initial (relative) CS value
    pub e_lfarlc: u16,    // File address of relocation table
    pub e_ovno: u16,      // Overlay number
    pub e_res: [u16; 4],  // Reserved words
    pub e_oemid: u16,     // OEM identifier (for e_oeminfo)
    pub e_oeminfo: u16,   // OEM information; e_oemid specific
    pub e_res2: [u16; 10],// Reserved words
    pub e_lfanew: i32,    // File address of new exe header
}

#[repr(C, packed)]
pub struct ImageFileHeader {
    pub machine: u16,
    pub number_of_sections: u16,
    pub time_date_stamp: u32,
    pub pointer_to_symbol_table: u32,
    pub number_of_symbols: u32,
    pub size_of_optional_header: u16,
    pub characteristics: u16,
}

#[repr(C, packed)]
pub struct ImageDataDirectory {
    pub virtual_address: u32,
    pub size: u32,
}

#[repr(C, packed)]
pub struct ImageOptionalHeader64 {
    pub magic: u16,
    pub major_linker_version: u8,
    pub minor_linker_version: u8,
    pub size_of_code: u32,
    pub size_of_initialized_data: u32,
    pub size_of_uninitialized_data: u32,
    pub address_of_entry_point: u32,
    pub base_of_code: u32,
    pub image_base: u64,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub major_operating_system_version: u16,
    pub minor_operating_system_version: u16,
    pub major_image_version: u16,
    pub minor_image_version: u16,
    pub major_subsystem_version: u16,
    pub minor_subsystem_version: u16,
    pub win32_version_value: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub check_sum: u32,
    pub subsystem: u16,
    pub dll_characteristics: u16,
    pub size_of_stack_reserve: u64,
    pub size_of_stack_commit: u64,
    pub size_of_heap_reserve: u64,
    pub size_of_heap_commit: u64,
    pub loader_flags: u32,
    pub number_of_rva_and_sizes: u32,
    pub data_directory: [ImageDataDirectory; 16],
}

#[repr(C, packed)]
pub struct ImageNtHeaders64 {
    pub signature: u32, // 'PE\0\0'
    pub file_header: ImageFileHeader,
    pub optional_header: ImageOptionalHeader64,
}

#[repr(C, packed)]
pub struct ImageSectionHeader {
    pub name: [u8; 8],
    pub virtual_size: u32,
    pub virtual_address: u32,
    pub size_of_raw_data: u32,
    pub pointer_to_raw_data: u32,
    pub pointer_to_relocations: u32,
    pub pointer_to_linenumbers: u32,
    pub number_of_relocations: u16,
    pub number_of_linenumbers: u16,
    pub characteristics: u32,
}

#[repr(C, packed)]
pub struct ImageImportDescriptor {
    pub characteristics: u32,
    pub time_date_stamp: u32,
    pub forwarder_chain: u32,
    pub name: u32,
    pub first_thunk: u32,
}

#[repr(C, packed)]
pub struct ImageBaseRelocation {
    pub virtual_address: u32,
    pub size_of_block: u32,
}

#[repr(C, packed)]
pub struct ImageExportDirectory {
    pub characteristics: u32,
    pub time_date_stamp: u32,
    pub major_version: u16,
    pub minor_version: u16,
    pub name: u32,
    pub base: u32,
    pub number_of_functions: u32,
    pub number_of_names: u32,
    pub address_of_functions: u32,
    pub address_of_names: u32,
    pub address_of_name_ordinals: u32,
}

pub fn is_pe(data: &[u8]) -> bool {
    if data.len() < size_of::<ImageDosHeader>() { return false; }
    let dos = unsafe { &*(data.as_ptr() as *const ImageDosHeader) };
    if dos.e_magic != 0x5A4D { return false; } // 'MZ'
    
    let nt_off = dos.e_lfanew as usize;
    if data.len() < nt_off + size_of::<ImageNtHeaders64>() { return false; }
    let nt = unsafe { &*(data.as_ptr().add(nt_off) as *const ImageNtHeaders64) };
    nt.signature == 0x00004550 // 'PE\0\0'
}

pub fn get_entry_point(data: &[u8]) -> Option<u64> {
    if !is_pe(data) { return None; }
    let dos = unsafe { &*(data.as_ptr() as *const ImageDosHeader) };
    let nt_off = dos.e_lfanew as usize;
    let nt = unsafe { &*(data.as_ptr().add(nt_off) as *const ImageNtHeaders64) };
    Some(nt.optional_header.image_base + nt.optional_header.address_of_entry_point as u64)
}
