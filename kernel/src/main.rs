#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod font;
mod fb;
mod term;
mod vga;
mod port;
mod pic;
mod idt;
mod gdt;
mod pmm;
mod memory;
mod vmm;
mod syscall;
mod elf;
mod rpv;
mod serial;
mod sched;
mod input;
mod ps2mouse;
mod ramfs;
mod vfs;
mod linux;
mod pci;
mod hda;
mod rtl8139;
mod e1000;
mod r8169;
mod net;
mod usb;
mod crypto;
mod tls;
mod ahci;
mod rpfs;
mod diskfs;
mod virtio_gpu;
mod p256;
mod bignum;
mod x509;
mod test_certs;
mod ca_roots;
mod iwlwifi_fw;
mod iwlwifi;
mod wpa2;
mod acpi;

use ternary_core::{Trit, Tryte};
use mathematics::{mul_tryte, consensus, scale};
use hardware_abstraction::{TernaryALU, SoftwareALU};
use ai_runtime::{TernaryTensor, TernaryLinear};
use core::panic::PanicInfo;

extern "C" { static kernel_end: u8; }

// user-psh ELF — built by iso/build.sh, embedded at compile time
static USER_PSH_ELF: &[u8] = include_bytes!("../user-psh.elf");

// Linux-ABI brick 1: a freestanding, unmodified static Linux x86-64 ELF
// (raw write/exit_group). Booting with `linuxtest` on the kernel cmdline runs
// THIS through the Linux ABI layer instead of the desktop — proof the bare-metal
// kernel can execute real Linux binaries. Built by kernel/linux-abi-test/.
static LINUX_HELLO_ELF: &[u8] = include_bytes!("../linux-abi-test/linux-hello");

/// Walk the Multiboot2 info structure looking for the first module tag (type=3).
/// Returns (mod_start, mod_end) if found.
unsafe fn parse_mb2_module(mb2: u32) -> Option<(u32, u32)> {
    if mb2 == 0 { return None; }
    let total = *(mb2 as *const u32);
    let mut off: u32 = 8;
    while off < total {
        let tag_ptr = (mb2 + off) as *const u32;
        let ttype = *tag_ptr;
        let tsize = *tag_ptr.add(1);
        if ttype == 0 { break; }
        if ttype == 3 && tsize >= 16 {
            // struct multiboot_tag_module: u32 type, u32 size, u32 mod_start, u32 mod_end, char cmdline[]
            let mod_start = *((mb2 + off + 8)  as *const u32);
            let mod_end   = *((mb2 + off + 12) as *const u32);
            return Some((mod_start, mod_end));
        }
        off += (tsize + 7) & !7;
    }
    None
}

/// Walk the Multiboot2 info structure looking for the framebuffer tag (type=8).
/// Returns (phys_addr, width, height, pitch, bpp) if found.
unsafe fn parse_mb2_framebuffer(mb2: u32) -> Option<(u64, u32, u32, u32, u8)> {
    if mb2 == 0 { return None; }
    let total = *(mb2 as *const u32);
    let mut off: u32 = 8;
    while off < total {
        let tag_ptr = (mb2 + off) as *const u32;
        let ttype = *tag_ptr;
        let tsize = *tag_ptr.add(1);
        if ttype == 0 { break; }
        if ttype == 8 && tsize >= 32 {
            // struct multiboot_tag_framebuffer:
            //   u32 type, u32 size, u64 addr, u32 pitch, u32 width, u32 height, u8 bpp, u8 fb_type
            let addr  = *((mb2 + off + 8)  as *const u64);
            let pitch = *((mb2 + off + 16) as *const u32);
            let width = *((mb2 + off + 20) as *const u32);
            let height= *((mb2 + off + 24) as *const u32);
            let bpp   = *((mb2 + off + 28) as *const u8);
            let fbtype= *((mb2 + off + 29) as *const u8);
            // fbtype 1 = RGB linear (what we want), 2 = EGA text (skip)
            if fbtype == 1 && bpp >= 24 && addr != 0 {
                return Some((addr, width, height, pitch, bpp));
            }
        }
        off += (tsize + 7) & !7;
    }
    None
}

/// Walk the Multiboot2 info for the boot command line (tag type=1) and test
/// whether it contains `needle`.
unsafe fn mb2_cmdline_contains(mb2: u32, needle: &[u8]) -> bool {
    if mb2 == 0 || needle.is_empty() { return false; }
    let total = *(mb2 as *const u32);
    let mut off: u32 = 8;
    while off < total {
        let tag_ptr = (mb2 + off) as *const u32;
        let ttype = *tag_ptr;
        let tsize = *tag_ptr.add(1);
        if ttype == 0 { break; }
        if ttype == 1 && tsize > 8 {
            let s = (mb2 + off + 8) as *const u8;
            let slen = (tsize - 8) as usize;
            // naive substring search
            if slen >= needle.len() {
                for i in 0..=(slen - needle.len()) {
                    let mut hit = true;
                    for j in 0..needle.len() {
                        if *s.add(i + j) != needle[j] { hit = false; break; }
                    }
                    if hit { return true; }
                }
            }
        }
        off += (tsize + 7) & !7;
    }
    false
}

/// Set by boot from `autostart=N` on the kernel cmdline; the desktop reads it via
/// sys_autostart (#22) and opens app N on launch. -1 = none. Used to screendump a
/// specific app headlessly (deterministic GUI verification without driving mouse).
pub static mut AUTOSTART_APP: i64 = -1;

/// Set by boot from `wallpaper=N` on the kernel cmdline; the desktop reads it via
/// sys_wallpaper (#23) and boots with system background N. -1 = default.
pub static mut WALLPAPER_DEF: i64 = -1;

/// Set by boot from `brightness=N` (15..100) on the kernel cmdline; the desktop
/// reads it via sys_boot_brightness (#40) and starts at that software-brightness
/// level (a real default/kiosk/accessibility knob, and the headless way to verify
/// the dimming since the slider can't be driven by mouse). -1 = full (100%).
pub static mut BOOT_BRIGHTNESS: i64 = -1;

/// Set by boot from `gpudisplay` on the kernel cmdline. When true, sys_fb_query
/// hands the desktop the virtio-gpu backing instead of the VBE framebuffer, so
/// the whole UI is rendered into RAM and DMA-scanned by the GPU (sys_gpu_flush).
pub static mut GPU_DISPLAY: bool = false;

/// Set by boot from `showmenu` on the kernel cmdline; the desktop reads it via
/// sys_showmenu (#35) and opens the start menu on launch. A marketing/screenshot
/// aid (mouse clicks can't be driven headlessly), like `autostart=`.
pub static mut SHOW_MENU: bool = false;

/// Parse the decimal value following `key` (e.g. b"autostart=") in the MB2
/// command line. Returns None if absent.
unsafe fn mb2_cmdline_value(mb2: u32, key: &[u8]) -> Option<i64> {
    if mb2 == 0 || key.is_empty() { return None; }
    let total = *(mb2 as *const u32);
    let mut off: u32 = 8;
    while off < total {
        let tag_ptr = (mb2 + off) as *const u32;
        let ttype = *tag_ptr;
        let tsize = *tag_ptr.add(1);
        if ttype == 0 { break; }
        if ttype == 1 && tsize > 8 {
            let s = (mb2 + off + 8) as *const u8;
            let slen = (tsize - 8) as usize;
            if slen >= key.len() {
                'scan: for i in 0..=(slen - key.len()) {
                    for j in 0..key.len() {
                        if *s.add(i + j) != key[j] { continue 'scan; }
                    }
                    // matched key at i; parse digits after it
                    let mut p = i + key.len();
                    let mut val: i64 = 0;
                    let mut any = false;
                    while p < slen {
                        let c = *s.add(p);
                        if c < b'0' || c > b'9' { break; }
                        val = val * 10 + (c - b'0') as i64;
                        any = true; p += 1;
                    }
                    return if any { Some(val) } else { None };
                }
            }
        }
        off += (tsize + 7) & !7;
    }
    None
}

/// Enable SSE/SSE2 (CR0.EM=0, CR0.MP=1, CR4.OSFXSR=1, CR4.OSXMMEXCPT=1).
/// The kernel itself is built soft-float (no SSE), but every real Linux x86-64
/// binary uses SSE2 (it's the x86-64 baseline) — the Linux ABI layer needs it
/// or the first `movd %xmm` in glibc's startup faults.
fn enable_sse() {
    unsafe {
        core::arch::asm!(
            "mov rax, cr0",
            "and ax, 0xFFFB",  // clear CR0.EM (bit 2): no x87 emulation
            "or  ax, 0x0002",  // set   CR0.MP (bit 1)
            "mov cr0, rax",
            "mov rax, cr4",
            "or  ax, 0x0600",  // set CR4.OSFXSR (9) + CR4.OSXMMEXCPT (10)
            "mov cr4, rax",
            out("rax") _,
            options(nostack),
        );
    }
}

#[no_mangle]
pub extern "C" fn kernel_main(magic: u32, mb2: u32) {
    vga::clear();
    serial::init();
    enable_sse();

    // Higher-half migration (docs/VMM_HIGHER_HALF.md): the kernel is now linked
    // at -2 GiB and boot.s jumped RIP up here. Confirm we are executing from the
    // higher half (RIP ≥ 0xFFFFFFFF80000000) before anything else.
    {
        let rip: u64;
        let rsp: u64;
        unsafe {
            core::arch::asm!("lea {}, [rip + 0]", out(reg) rip, options(nostack));
            core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nostack));
        }
        serial::write_str("[hh] kernel_main RIP = ");
        serial::write_hex_u64(rip);
        serial::write_str(if rip >= vmm::KERNEL_VMA { "  (higher half OK)\n" } else { "  (STILL LOW!)\n" });
        serial::write_str("[hh] kernel_main RSP = ");
        serial::write_hex_u64(rsp);
        serial::write_str(if rsp >= vmm::KERNEL_VMA { "  (stack high OK)\n" } else { "  (stack STILL LOW!)\n" });
    }

    if magic != 0x36d76289 {
        vga::write_str("ERROR: bad multiboot2 magic\n", vga::Color::Red);
        loop {}
    }

    vga::write_str("  Rusty Penguin v1.0.0 -- bare metal kernel\n", vga::Color::Green);
    vga::write_str("  Binary hardware. Ternary mind.\n\n", vga::Color::Amber);

    allocator::init();

    // GDT: null | kcode | kdata | udata | ucode | TSS
    gdt::init();
    vga::write_str("  [GDT+TSS: OK]\n", vga::Color::Green);

    // PIC + IDT
    unsafe { pic::init(); pic::pit_init(); }
    idt::init();
    idt::enable();
    vga::write_str("  [interrupts: OK]\n", vga::Color::Green);

    // Memory map + physical memory manager
    vga::write_str("  [memory map]\n", vga::Color::Cyan);
    memory::print_map(mb2);
    vga::write_byte(b'\n', vga::Color::White);

    // kernel_end is a higher-half VMA now; pmm reserves the kernel's PHYSICAL
    // image range, so translate back to physical (LMA = VMA − KERNEL_VMA).
    let kend = core::ptr::addr_of!(kernel_end) as u64 - vmm::KERNEL_VMA;
    pmm::init(mb2, kend);
    let (free, total) = pmm::stats();
    vga::write_str("  [PMM] ", vga::Color::Cyan);
    vga::write_i32((free / 256) as i32);
    vga::write_str(" MiB free / ", vga::Color::White);
    vga::write_i32((total / 256) as i32);
    vga::write_str(" MiB total\n\n", vga::Color::White);

    // Extend identity map: boot.s only covered 0-2 MiB.
    // We need PMM-allocated frames (above 2 MiB) to be reachable.
    vmm::extend_identity_map(64);  // identity-map 0–64 MiB
    vga::write_str("  [VMM] identity map extended to 64 MiB\n", vga::Color::Green);

    // Higher-half physmap (docs/VMM_HIGHER_HALF.md sub-step 1b): direct-map all
    // physical RAM at PHYSMAP_BASE so the VMM reaches page-table frames through
    // the higher half instead of the low identity map. After this the kernel no
    // longer depends on PML4[0] for paging structures — the prerequisite for
    // dropping the low alias and giving each process a private low half.
    let phys_mib = (total / 256) as usize;            // frames → MiB (256 × 4 KiB)
    let map_mib  = ((phys_mib + 4) & !1).max(64);     // round up to 2 MiB, small margin
    unsafe { vmm::build_physmap(map_mib); }
    vga::write_str("  [VMM] physmap: RAM direct-mapped at higher half (", vga::Color::Green);
    vga::write_i32(map_mib as i32);
    vga::write_str(" MiB)\n", vga::Color::Green);

    // Parse initramfs CPIO from GRUB module (Multiboot2 tag type=3).
    //
    // Relocate it to a fixed high address first. The ring-3 desktop is loaded at
    // 0x400000 and the ELF loader zero-fills its .bss (the 24 MiB heap) IN PLACE,
    // covering roughly 0x400000..0x1C00000 (~28 MiB). GRUB may park the initrd
    // inside that range, so loading the desktop ELF would zero out the very
    // module we read it from (seen as `entry @ 0x0` + #PF). Copying the module
    // up to 40 MiB — above the heap, below the 63 MiB ring-3 stack, inside the
    // 64 MiB identity map — keeps the source intact across the .bss wipe.
    // 128 MiB (physical). Above the desktop image+heap (≤28 MiB), the ring-3
    // stack (~63 MiB), AND the HDA audio DMA buffer (~50 MiB) — a large initrd
    // (e.g. the 58 MiB meta-video) at the old 40 MiB would overlap the audio
    // buffer and get corrupted as PCM streams. Accessed via the physmap, so it
    // needs no low identity coverage; only enough RAM (initrd_top < phys RAM).
    const INITRD_RELOC: u64 = 0x0800_0000;
    if let Some((mod_start, mod_end)) = unsafe { parse_mb2_module(mb2) } {
        let size = (mod_end - mod_start) as usize;
        // Copy + read the initrd through the physmap (higher half) so ramfs does
        // not depend on the low identity map. Same physical bytes (40 MiB), high
        // virtual addresses. The reloc target is still low phys 40 MiB, which
        // survives the desktop's low .bss wipe (≤28 MiB). ptr::copy is memmove.
        let src = vmm::phys_to_virt(mod_start as u64) as *const u8;
        let dst = vmm::phys_to_virt(INITRD_RELOC) as *mut u8;
        unsafe { core::ptr::copy(src, dst, size); }
        ramfs::init(dst as *const u8, size);
        let count = ramfs::inode_count();
        vga::write_str("  [ramfs] ", vga::Color::Green);
        vga::write_i32(count as i32);
        vga::write_str(" files loaded (initrd relocated to 40 MiB)\n", vga::Color::Green);
    } else {
        vga::write_str("  [ramfs] no module — VFS unavailable\n", vga::Color::Amber);
    }

    // Switch to framebuffer if GRUB provided one (Multiboot2 tag type 8).
    // fb::init maps the VRAM into the higher half (map_mmio_high); term::init()
    // paints the dark background.
    if let Some((addr, w, h, pitch, bpp)) = unsafe { parse_mb2_framebuffer(mb2) } {
        fb::init(addr, w, h, pitch, bpp);
        // Prove the higher-half FB mapping is live VRAM: write a sentinel and
        // read it back through the high virtual base before painting over it.
        fb::pixel(0, 0, 0x00ABCDEF);
        let got = fb::read_pixel(0, 0);
        serial::write_str("[hh] FB via higher half: wrote 0xABCDEF read ");
        serial::write_hex_u64(got as u64);
        serial::write_str(if got == 0x00ABCDEF { "  (FB high OK)\n" } else { "  (FB MISMATCH!)\n" });
        term::init();
        vga::write_str("  [FB] ", vga::Color::Green);
        vga::write_i32(w as i32);
        vga::write_str("x", vga::Color::White);
        vga::write_i32(h as i32);
        vga::write_str("x", vga::Color::White);
        vga::write_i32(bpp as i32);
        vga::write_str("bpp @ 0x", vga::Color::White);
        vga::write_hex(addr, vga::Color::Cyan);
        vga::write_byte(b'\n', vga::Color::White);
    } else {
        // GRUB didn't give us a framebuffer — probe common UEFI GOP addresses.
        // Most Intel iGPUs map their linear framebuffer at 0x80000000 or
        // 0xC0000000 or another PCI BAR. Without GOP we can't know the exact
        // address, so we must stay in VGA text mode. The GRUB gfxpayload=... in
        // grub.cfg should have handled this on any UEFI system — if we reach
        // here it means GRUB is in pure-EFI mode without gfxpayload, which is
        // rare. The user should add `set gfxpayload=keep` or `set gfxpayload=1920x1080x32`
        // to their grub.cfg. We log the advisory so the serial log guides them.
        vga::write_str("  [FB] no framebuffer from GRUB — VGA text mode\n", vga::Color::Amber);
        vga::write_str("  [FB] hint: add gfxpayload=1920x1080x32 to grub.cfg entry\n", vga::Color::Amber);
    }

    // SYSCALL / SYSRET setup
    syscall::init();
    vga::write_str("  [SYSCALL: OK]\n", vga::Color::Green);

    // Process table: PID 0 = idle, PID 1 = psh (registered before IRETQ)
    sched::init();
    vga::write_str("  [sched: OK]\n", vga::Color::Green);

    // PS/2 mouse: init, then unmask IRQ12 on the PIC (fallback for older HW)
    ps2mouse::init();
    unsafe { pic::unmask_mouse(); }
    vga::write_str("  [PS/2 mouse: OK]\n", vga::Color::Green);

    // USB HID — xHCI for modern laptops (no PS/2), EHCI/OHCI fallback.
    // Feeds into the same input ring as PS/2 so the desktop sees one unified stream.
    match usb::init() {
        Trit::Pos  => vga::write_str("  [USB HID: keyboard+mouse OK]\n", vga::Color::Green),
        Trit::Zero => vga::write_str("  [USB HID: controller found, no HID]\n", vga::Color::Amber),
        Trit::Neg  => vga::write_str("  [USB HID: no USB controller]\n", vga::Color::Amber),
    }

    // Intel HDA audio — attempt to init; ternary result logged.
    let audio_state = hda::init();
    use ternary_core::Trit;
    match audio_state {
        Trit::Pos  => vga::write_str("  [HDA audio: playing]\n", vga::Color::Green),
        Trit::Zero => vga::write_str("  [HDA audio: no device]\n", vga::Color::Amber),
        Trit::Neg  => vga::write_str("  [HDA audio: init failed]\n", vga::Color::Red),
    }

    // Networking — RTL8139 NIC, ARP, then ICMP ping the gateway.
    match net::init() {
        Trit::Pos  => vga::write_str("  [net: DHCP lease OK]\n", vga::Color::Green),
        Trit::Zero => vga::write_str("  [net: NIC up, no lease]\n", vga::Color::Amber),
        Trit::Neg  => vga::write_str("  [net: no NIC]\n", vga::Color::Red),
    }

    // TLS crypto self-test (SHA-256, X25519, ChaCha20-Poly1305, HKDF) vs RFC vectors.
    if crypto::selftest() {
        vga::write_str("  [crypto: TLS primitives OK]\n", vga::Color::Green);
    } else {
        vga::write_str("  [crypto: SELF-TEST FAILED]\n", vga::Color::Red);
    }

    // X.509 certificate-chain validation — the trust half of TLS (RSA-PKCS#1 +
    // ECDSA-P256, chain walk to an embedded root, expiry + hostname). Validates an
    // embedded leaf->int->root chain and confirms a tampered copy is rejected.
    if x509::selftest() {
        vga::write_str("  [x509: cert-chain trust OK]\n", vga::Color::Green);
    } else {
        vga::write_str("  [x509: SELF-TEST FAILED]\n", vga::Color::Red);
    }

    // Intel WiFi probe (brick 1): recognise an iwlwifi card + the firmware it
    // needs. QEMU has no iwlwifi to emulate, so this only lights up on real metal.
    if iwlwifi::init() {
        vga::write_str("  [wifi: Intel WiFi card detected]\n", vga::Color::Green);
    } else {
        vga::write_str("  [wifi: no Intel WiFi (QEMU has none)]\n", vga::Color::Amber);
    }
    // WPA2 auth core (the hardware-independent half of WiFi): verify the PSK/PTK
    // key-derivation crypto against its canonical IEEE/RFC vectors at boot. Unlike
    // the radio, this needs no hardware — so it is proven even under QEMU.
    if wpa2::selftest() {
        vga::write_str("  [wifi: WPA2 auth core OK (PMK/PTK vectors verified)]\n", vga::Color::Green);
    } else {
        vga::write_str("  [wifi: WPA2 auth core SELFTEST FAILED]\n", vga::Color::Red);
    }

    // ACPI power management: parse the firmware tables so we can cleanly power
    // off (S5) and reboot — a daily-driver basic the kernel lacked entirely.
    if acpi::init() {
        vga::write_str("  [power: ACPI shutdown + reboot ready]\n", vga::Color::Green);
    } else {
        vga::write_str("  [power: no ACPI]\n", vga::Color::Amber);
    }

    // AHCI/SATA disk + RPFS filesystem — persistent bare-metal storage.
    if ahci::init() {
        if ahci::selftest() {
            vga::write_str("  [disk: AHCI SATA read/write OK]\n", vga::Color::Green);
        } else {
            vga::write_str("  [disk: AHCI present, self-test failed]\n", vga::Color::Amber);
        }
        diskfs::init();
    } else {
        vga::write_str("  [disk: no AHCI disk]\n", vga::Color::Amber);
    }

    // virtio-gpu — from-scratch GPU driver (brick 1: transport + display query).
    // Additive: the VBE framebuffer stays the live display for now.
    vga::write_str("  [virtio-gpu]\n", vga::Color::Cyan);
    // `virgltest` → negotiate VIRTIO_GPU_F_VIRGL and probe the 3D control path
    // (capset query + context create). Off by default (2D path unchanged).
    if unsafe { mb2_cmdline_contains(mb2, b"virgltest") } {
        virtio_gpu::enable_virgl_test();
    }
    if virtio_gpu::init() {
        let (gw, gh) = virtio_gpu::display_dims();
        if gw > 0 {
            vga::write_str("  [gpu: virtio-gpu up]\n", vga::Color::Green);
        }
        let _ = gh;
    }

    // Ternary math demo
    vga::write_str("  [mathematics]\n", vga::Color::Cyan);
    let a = Tryte::from_i32(42);
    let b = Tryte::from_i32(-7);
    vga::write_str("    42+(-7)=", vga::Color::White);
    vga::write_i32((a + b).to_i32());
    vga::write_str("  6*7=", vga::Color::White);
    let (lo, _) = mul_tryte(Tryte::from_i32(6), Tryte::from_i32(7));
    vga::write_i32(lo.to_i32());
    vga::write_str("  scale(100,-)=", vga::Color::White);
    vga::write_i32(scale(Tryte::from_i32(100), Trit::Neg).to_i32());
    let con = consensus(Trit::Pos, Trit::Pos);
    vga::write_str("  cons(+,+)=", vga::Color::White);
    vga::write_str(match con { Trit::Pos=>"Pos", Trit::Neg=>"Neg", Trit::Zero=>"Zero" }, vga::Color::Green);
    vga::write_byte(b'\n', vga::Color::White);

    let alu = SoftwareALU;
    vga::write_str("    ALU 9+3=", vga::Color::White);
    vga::write_i32(alu.add(Tryte::from_i32(9), Tryte::from_i32(3)).to_i32());
    vga::write_byte(b'\n', vga::Color::White);

    // Sparse AI inference
    vga::write_str("  [sparse AI]\n", vga::Color::Cyan);
    let mut layer = TernaryLinear::new(4, 2);
    layer.weights.data = alloc::vec![
        Trit::Pos, Trit::Zero, Trit::Neg, Trit::Pos,
        Trit::Neg, Trit::Pos, Trit::Zero, Trit::Neg,
    ];
    let input = TernaryTensor::new(
        alloc::vec![Trit::Pos, Trit::Zero, Trit::Neg, Trit::Pos],
        alloc::vec![4],
    );
    let (output, total_ops, skipped) = layer.forward(&input);
    vga::write_str("    [+,0,-,+] → [", vga::Color::White);
    for (i, t) in output.data.iter().enumerate() {
        if i > 0 { vga::write_str(",", vga::Color::White); }
        vga::write_str(match t { Trit::Pos=>"+", Trit::Neg=>"-", Trit::Zero=>"0" }, vga::Color::Amber);
    }
    let pct = (skipped * 100) / total_ops.max(1);
    vga::write_str("]  ", vga::Color::White);
    vga::write_i32(pct as i32);
    vga::write_str("% dormancy\n\n", vga::Color::White);

    // ── Scheduler brick (Increment 1) ────────────────────────────────────────
    // Boot with `schedtest` on the cmdline to run the cooperative context-switch
    // self-test (kernel tasks, serial output) instead of the desktop. Gated so it
    // can never disturb the working desktop boot. See docs/SCHEDULER.md.
    if unsafe { mb2_cmdline_contains(mb2, b"physmaptest") } {
        vmm::selftest_physmap(); // higher-half direct map (VMM migration step 1a)
    }
    if unsafe { mb2_cmdline_contains(mb2, b"metavideo") } {
        rpv::play_from_initrd(); // play the founding clip on the kernel that it triggered
    }
    if unsafe { mb2_cmdline_contains(mb2, b"videowin") } {
        rpv::selftest_window(); // windowed service path (desktop Media app drives this)
    }
    if unsafe { mb2_cmdline_contains(mb2, b"fstest") } {
        diskfs::selftest(); // RPFS v2 on real AHCI: persistence + reclamation + dirs
    }
    if unsafe { mb2_cmdline_contains(mb2, b"acpipoweroff") } {
        acpi::poweroff(); // verify ACPI S5 soft-off (QEMU exits cleanly)
    }
    if unsafe { mb2_cmdline_contains(mb2, b"acpireboot") } {
        acpi::reboot(); // verify ACPI/8042 reset
    }
    if unsafe { mb2_cmdline_contains(mb2, b"multiproc") } {
        // Hung-app isolation: a healthy + a wedged ring-3 process under preemption.
        sched::selftest_multiproc(); // never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"watchdog") } {
        // Detect + force-quit a hung ring-3 process, then keep running.
        sched::selftest_watchdog(); // never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"realelf") } {
        // Two REAL ELF programs as preemptively-scheduled, isolated processes.
        sched::selftest_realelf(); // never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"offscreen") } {
        // A scheduled process renders into an isolated buffer the compositor reads.
        sched::selftest_offscreen(); // never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"composite") } {
        // Process renders offscreen; kernel compositor blits its surface to screen.
        sched::selftest_composite(); // never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"multiwin") } {
        // Two isolated app processes composited into two windows at once.
        sched::selftest_multiwin(); // never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"recoverwin") } {
        // A hung windowed app is force-quit; the healthy one keeps running.
        sched::selftest_recover_win(); // never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"linuxmmap") } {
        // Private-AS mmap for a scheduled Linux process (windowed-DOOM brick 2b foundation).
        sched::selftest_linuxmmap(); // never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"linuxsched") } {
        // A real static Linux ELF as a scheduled process (windowed-DOOM brick 2a).
        sched::selftest_linuxsched(); // never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"linuxroute") } {
        // Per-task ABI mode: a native + a Linux process scheduled together, each
        // routing syscalls to the right table (the foundation for windowed DOOM).
        sched::selftest_linuxroute(); // never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"schedesktop2") } {
        // The REAL desktop + a second real app, both preemptively scheduled and
        // isolated. Checked BEFORE `schedesktop` (which is a substring).
        sched::selftest_schedesktop2(); // never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"schedesktop") } {
        // The REAL desktop loaded + run as a scheduled process (not the default path).
        sched::selftest_schedesktop(); // never returns
    }
    // autostart=N → desktop opens app N on launch (deterministic GUI screendumps).
    if let Some(n) = unsafe { mb2_cmdline_value(mb2, b"autostart=") } {
        unsafe { AUTOSTART_APP = n; }
    }
    // wallpaper=N → desktop boots with system background N (test + feature).
    if let Some(n) = unsafe { mb2_cmdline_value(mb2, b"wallpaper=") } {
        unsafe { WALLPAPER_DEF = n; }
    }
    // brightness=N → desktop boots at software-brightness N% (default/kiosk knob,
    // and the headless way to verify the present-time dimming LUT).
    if let Some(n) = unsafe { mb2_cmdline_value(mb2, b"brightness=") } {
        unsafe { BOOT_BRIGHTNESS = n; }
    }
    // gpudisplay → route the desktop's framebuffer through the virtio-gpu device.
    if unsafe { mb2_cmdline_contains(mb2, b"gpudisplay") } && virtio_gpu::is_ready() {
        unsafe { GPU_DISPLAY = true; }
        vga::write_str("  [gpu: desktop routed through virtio-gpu]\n", vga::Color::Green);
    }
    // showmenu → desktop opens the start menu on launch (screenshot aid).
    if unsafe { mb2_cmdline_contains(mb2, b"showmenu") } {
        unsafe { SHOW_MENU = true; }
    }
    if unsafe { mb2_cmdline_contains(mb2, b"schedtest6") } {
        sched::selftest_ring3_lowhalf(); // private low half per process (Increment 3d)
    }
    if unsafe { mb2_cmdline_contains(mb2, b"schedtest5") } {
        sched::selftest_ring3(); // ring-3 task in a private address space
    }
    if unsafe { mb2_cmdline_contains(mb2, b"schedtest4") } {
        sched::selftest_cr3_sched(); // per-task CR3 switching under preemption
    }
    if unsafe { mb2_cmdline_contains(mb2, b"schedtest3") } {
        sched::selftest_vmm(); // per-process address spaces; never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"schedtest2") } {
        sched::selftest_preempt(); // timer-driven preemption; never returns
    }
    if unsafe { mb2_cmdline_contains(mb2, b"schedtest") } {
        sched::selftest();
        serial::write_str("[sched] self-test returned to boot thread; halting.\n");
        loop { unsafe { core::arch::asm!("hlt"); } }
    }

    // ── Linux-ABI brick 1 ─────────────────────────────────────────────────────
    // Boot with `linuxtest` on the cmdline to run an unmodified static Linux
    // x86-64 ELF through the Linux ABI layer instead of the desktop.
    if unsafe { mb2_cmdline_contains(mb2, b"linuxtest") } {
        // Prefer bin/linuxtest from the initrd (fast iteration — swap the test
        // binary without rebuilding the kernel); fall back to the embedded ELF.
        let elf: &[u8] = ramfs::find(b"bin/linuxtest").unwrap_or(LINUX_HELLO_ELF);
        vga::write_str("\n  [LINUX-ABI] executing an unmodified Linux ELF\n", vga::Color::Amber);
        serial::write_str("\n  [LINUX-ABI] executing an unmodified Linux ELF\n");
        vga::clear();
        if fb::is_live() { fb::fill(0, 0, fb::width(), fb::height(), 0x000000); }
        linux::enter(elf);
    }

    // ── Ring-3 launch ────────────────────────────────────────────────────────
    vga::write_str("  [ring-3 launch]\n", vga::Color::Cyan);

    // Prefer bin/desktop from VFS; fall back to embedded psh
    let (elf_bytes, proc_name): (&[u8], &[u8]) =
        if let Some(bytes) = ramfs::find(b"bin/desktop") {
            vga::write_str("    launching desktop-metal from VFS\n", vga::Color::Green);
            (bytes, b"desktop")
        } else {
            vga::write_str("    launching embedded psh\n", vga::Color::Amber);
            (USER_PSH_ELF, b"psh")
        };

    sched::register(proc_name);

    // Load ring-3 ELF into identity-mapped address space (virt == phys)
    let entry = elf::load(elf_bytes).expect("ELF parse failed");
    vga::write_str("    entry @ 0x", vga::Color::DimGray);
    vga::write_hex(entry, vga::Color::DimGray);
    vga::write_str("  stack @ 0x", vga::Color::DimGray);
    vga::write_hex(vmm::USER_STACK_TOP, vga::Color::DimGray);
    vga::write_byte(b'\n', vga::Color::White);

    // Clear VGA text buffer and framebuffer before ring-3 launch.
    // The framebuffer fill removes all kernel boot text written via term.rs
    // so ring-3 starts with a completely black canvas (it repaints immediately).
    vga::clear();
    if fb::is_live() {
        fb::fill(0, 0, fb::width(), fb::height(), 0x000000);
    }
    serial::write_byte(b'\n');

    // IRETQ into ring-3 — stack + code both live in PTE_USER huge pages
    // Pin entry→r8 and user_rsp→r9 so the compiler never aliases them to
    // rax, which we clobber during the RFLAGS pushfq/pop/or sequence.
    unsafe {
        let user_rsp: u64 = vmm::USER_STACK_TOP - 8;
        core::arch::asm!(
            "push 0x1B",    // SS  = UDATA_SEL | 3
            "push r9",      // user RSP
            "pushfq",
            "pop rax",
            "or  rax, 0x202",   // RFLAGS: IF=1, reserved=1
            "push rax",
            "push 0x23",    // CS  = UCODE_SEL | 3
            "push r8",      // user RIP
            "iretq",
            in("r8") entry,
            in("r9") user_rsp,
            options(noreturn),
        );
    }
}

/// Re-enter the desktop-metal ring-3 process after a Linux binary exits.
/// Called by linux::syscall exit_group when RESTART_DESKTOP is set.
pub fn restart_desktop() -> ! {
    let elf = crate::ramfs::find(b"bin/desktop")
        .expect("bin/desktop not in initrd");
    let entry = crate::elf::load(elf).expect("desktop ELF reload failed");
    if crate::fb::is_live() {
        crate::fb::fill(0, 0, crate::fb::width(), crate::fb::height(), 0x000000);
    }
    unsafe {
        let user_rsp: u64 = crate::vmm::USER_STACK_TOP - 8;
        core::arch::asm!(
            "push 0x1B",
            "push r9",
            "pushfq",
            "pop rax",
            "or  rax, 0x202",
            "push rax",
            "push 0x23",
            "push r8",
            "iretq",
            in("r8") entry,
            in("r9") user_rsp,
            options(noreturn),
        );
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // VGA — on-screen banner for direct console viewing.
    vga::write_str("\nKERNEL PANIC", vga::Color::Red);
    // Serial — captured by QEMU into /tmp/rusty-penguin.log so host-side
    // log inspection can distinguish a kernel panic from a userspace
    // panic ('!') and read the location/line.
    serial::write_str("\nKERNEL PANIC");
    if let Some(loc) = info.location() {
        vga::write_str(" at ", vga::Color::Red);
        vga::write_str(loc.file(), vga::Color::Red);
        vga::write_byte(b':', vga::Color::Red);
        vga::write_i32(loc.line() as i32);

        serial::write_str(" at ");
        serial::write_str(loc.file());
        serial::write_byte(b':');
        let mut line = loc.line();
        if line == 0 {
            serial::write_byte(b'0');
        } else {
            let mut buf = [0u8; 10];
            let mut i = 0;
            while line > 0 { buf[i] = b'0' + (line % 10) as u8; line /= 10; i += 1; }
            while i > 0 { i -= 1; serial::write_byte(buf[i]); }
        }
    }
    serial::write_byte(b'\n');
    loop {}
}
