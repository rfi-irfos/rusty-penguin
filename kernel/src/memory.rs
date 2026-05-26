use crate::vga;

// Multiboot2 tag header
#[repr(C)]
struct Mb2Tag { tag_type: u32, size: u32 }

// Memory map tag (type 6) header
#[repr(C)]
struct Mb2MmapTag { tag_type: u32, size: u32, entry_size: u32, _version: u32 }

// Memory map entry
#[repr(C)]
struct Mb2MmapEntry { base: u64, len: u64, entry_type: u32, _reserved: u32 }

const MMAP_AVAILABLE: u32 = 1;

pub fn print_map(mb2_addr: u32) {
    if mb2_addr == 0 { return; }

    unsafe {
        let total_size = *(mb2_addr as *const u32);
        let mut offset: u32 = 8; // skip total_size + reserved

        while offset < total_size {
            let tag = (mb2_addr + offset) as *const Mb2Tag;
            let ttype = (*tag).tag_type;
            let tsize = (*tag).size;

            if ttype == 0 { break; } // end tag

            if ttype == 6 {
                let mmap = (mb2_addr + offset) as *const Mb2MmapTag;
                let entry_size = (*mmap).entry_size;
                let mut eoff = mb2_addr + offset + 16; // 16-byte mmap tag header
                let eend = mb2_addr + offset + tsize;

                let mut total_kb: u64 = 0;
                while eoff < eend {
                    let e = eoff as *const Mb2MmapEntry;
                    if (*e).entry_type == MMAP_AVAILABLE {
                        let base = (*e).base;
                        let len  = (*e).len;
                        total_kb += len / 1024;
                        vga::write_str("    0x", vga::Color::DimGray);
                        vga::write_hex(base, vga::Color::DimGray);
                        vga::write_str(" + ", vga::Color::DimGray);
                        vga::write_hex(len / 1024, vga::Color::DimGray);
                        vga::write_str(" KiB\n", vga::Color::DimGray);
                    }
                    eoff += entry_size;
                }
                vga::write_str("    total available: ", vga::Color::Cyan);
                vga::write_hex(total_kb / 1024, vga::Color::Cyan);
                vga::write_str(" MiB\n", vga::Color::Cyan);
                return;
            }

            // Tags are 8-byte aligned
            offset += (tsize + 7) & !7;
        }
    }
}
