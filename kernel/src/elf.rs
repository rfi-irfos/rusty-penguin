use core::mem::size_of;

const ELF_MAGIC: &[u8; 4] = b"\x7FELF";
const PT_LOAD:   u32 = 1;
const PF_W:      u32 = 2;

#[repr(C)]
struct Elf64Ehdr {
    e_ident:     [u8; 16],
    e_type:      u16,
    e_machine:   u16,
    e_version:   u32,
    e_entry:     u64,
    e_phoff:     u64,
    e_shoff:     u64,
    e_flags:     u32,
    e_ehsize:    u16,
    e_phentsize: u16,
    e_phnum:     u16,
    e_shentsize: u16,
    e_shnum:     u16,
    e_shstrndx:  u16,
}

#[repr(C)]
struct Elf64Phdr {
    p_type:   u32,
    p_flags:  u32,
    p_offset: u64,
    p_vaddr:  u64,
    p_paddr:  u64,
    p_filesz: u64,
    p_memsz:  u64,
    p_align:  u64,
}

const PT_INTERP: u32 = 3;

/// Load an ELF64 binary into the identity-mapped address space (virt == phys).
/// Returns the entry point (e_entry), or None if malformed.
pub fn load(elf: &[u8]) -> Option<u64> { load_bias(elf, 0) }

/// Like `load`, but add `bias` to every `p_vaddr` (and the entry). Used to
/// place a position-independent ELF — e.g. the dynamic linker (ET_DYN) — at a
/// chosen base. Returns `e_entry + bias`.
pub fn load_bias(elf: &[u8], bias: u64) -> Option<u64> {
    if elf.len() < size_of::<Elf64Ehdr>() { return None; }

    let ehdr = unsafe { &*(elf.as_ptr() as *const Elf64Ehdr) };

    if &ehdr.e_ident[0..4] != ELF_MAGIC { return None; }
    if ehdr.e_machine != 0x3E           { return None; }  // EM_X86_64
    if ehdr.e_phnum   == 0              { return None; }

    let phoff    = ehdr.e_phoff     as usize;
    let phentsz  = ehdr.e_phentsize as usize;
    let phnum    = ehdr.e_phnum     as usize;

    for i in 0..phnum {
        let off = phoff + i * phentsz;
        if off + size_of::<Elf64Phdr>() > elf.len() { return None; }

        let ph = unsafe { &*(elf.as_ptr().add(off) as *const Elf64Phdr) };

        if ph.p_type != PT_LOAD || ph.p_memsz == 0 { continue; }

        let vaddr   = (ph.p_vaddr + bias) as usize;
        let memsz   = ph.p_memsz  as usize;
        let filesz  = ph.p_filesz as usize;
        let foff    = ph.p_offset as usize;

        // Zero the segment up to the PAGE boundary, not just memsz. Real ELF
        // loaders map full pages, so the bytes between memsz and the end of the
        // last page are guaranteed zero — and glibc's ld.so relies on this: it
        // places link_map / _rtld_global BSS structures in that page tail.
        // Zeroing only [vaddr, vaddr+memsz) leaves the tail as stale RAM, which
        // showed up as a non-canonical l_info pointer → #GP early in the linker.
        let zero_len = ((vaddr + memsz + 0xFFF) & !0xFFF) - vaddr;
        unsafe { core::ptr::write_bytes(vaddr as *mut u8, 0, zero_len); }

        // Copy file data
        if filesz > 0 {
            let src_end = foff.checked_add(filesz).filter(|&e| e <= elf.len())?;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    elf.as_ptr().add(foff),
                    vaddr as *mut u8,
                    src_end - foff,
                );
            }
        }

        let _ = ph.p_flags & PF_W;
    }

    Some(ehdr.e_entry + bias)
}

/// True if the ELF is position-independent (ET_DYN) — a PIE executable or a
/// shared object. Such images must be loaded at a non-zero bias (loading a PIE
/// at vaddr 0 maps the null page and breaks glibc's dynamic-section parsing).
pub fn is_dyn(elf: &[u8]) -> bool {
    if elf.len() < size_of::<Elf64Ehdr>() { return false; }
    let ehdr = unsafe { &*(elf.as_ptr() as *const Elf64Ehdr) };
    &ehdr.e_ident[0..4] == ELF_MAGIC && ehdr.e_type == 3 // ET_DYN
}

/// Return the `PT_INTERP` path (dynamic linker), if the ELF is dynamically
/// linked. The path is NUL-terminated in the file; the NUL is stripped.
pub fn interp_path(elf: &[u8]) -> Option<&[u8]> {
    if elf.len() < size_of::<Elf64Ehdr>() { return None; }
    let ehdr = unsafe { &*(elf.as_ptr() as *const Elf64Ehdr) };
    if &ehdr.e_ident[0..4] != ELF_MAGIC { return None; }
    let phoff = ehdr.e_phoff as usize;
    let phent = ehdr.e_phentsize as usize;
    for i in 0..ehdr.e_phnum as usize {
        let off = phoff + i * phent;
        if off + size_of::<Elf64Phdr>() > elf.len() { break; }
        let ph = unsafe { &*(elf.as_ptr().add(off) as *const Elf64Phdr) };
        if ph.p_type == PT_INTERP {
            let s = ph.p_offset as usize;
            let e = (ph.p_offset + ph.p_filesz) as usize;
            if e <= elf.len() {
                let mut bytes = &elf[s..e];
                if let Some(z) = bytes.iter().position(|&b| b == 0) { bytes = &bytes[..z]; }
                return Some(bytes);
            }
        }
    }
    None
}

/// Locate the program headers *in the loaded image* and return
/// (phdr_vaddr, e_phentsize, e_phnum) for the SysV auxv (AT_PHDR/PHENT/PHNUM).
/// glibc needs these to find PT_TLS (TCB sizing) and PT_GNU_RELRO; without them
/// it mis-sets up TLS (arch_prctl gets a bogus FS base) and crashes at exit.
pub fn phdr_info(elf: &[u8]) -> Option<(u64, u64, u64)> {
    if elf.len() < size_of::<Elf64Ehdr>() { return None; }
    let ehdr = unsafe { &*(elf.as_ptr() as *const Elf64Ehdr) };
    if &ehdr.e_ident[0..4] != ELF_MAGIC { return None; }
    let phoff = ehdr.e_phoff;
    let phent = ehdr.e_phentsize as u64;
    let phnum = ehdr.e_phnum as u64;
    // Find the PT_LOAD segment whose file range covers e_phoff; the phdrs are
    // then at p_vaddr + (e_phoff - p_offset) in the loaded image.
    for i in 0..phnum as usize {
        let off = phoff as usize + i * phent as usize;
        if off + size_of::<Elf64Phdr>() > elf.len() { break; }
        let ph = unsafe { &*(elf.as_ptr().add(off) as *const Elf64Phdr) };
        if ph.p_type == PT_LOAD
            && phoff >= ph.p_offset
            && phoff < ph.p_offset + ph.p_filesz
        {
            return Some((ph.p_vaddr + (phoff - ph.p_offset), phent, phnum));
        }
    }
    None
}
