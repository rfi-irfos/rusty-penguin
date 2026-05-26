#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
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
mod serial;

use ternary_core::{Trit, Tryte};
use mathematics::{mul_tryte, consensus, scale};
use hardware_abstraction::{TernaryALU, SoftwareALU};
use ai_runtime::{TernaryTensor, TernaryLinear};
use core::panic::PanicInfo;

extern "C" { static kernel_end: u8; }

// user-psh ELF — built by iso/build.sh, embedded at compile time
static USER_PSH_ELF: &[u8] = include_bytes!("../user-psh.elf");

#[no_mangle]
pub extern "C" fn kernel_main(magic: u32, mb2: u32) {
    vga::clear();
    serial::init();

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
    unsafe { pic::init(); }
    idt::init();
    idt::enable();
    vga::write_str("  [interrupts: OK]\n", vga::Color::Green);

    // Memory map + physical memory manager
    vga::write_str("  [memory map]\n", vga::Color::Cyan);
    memory::print_map(mb2);
    vga::write_byte(b'\n', vga::Color::White);

    let kend = core::ptr::addr_of!(kernel_end) as u64;
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

    // SYSCALL / SYSRET setup
    syscall::init();
    vga::write_str("  [SYSCALL: OK]\n", vga::Color::Green);

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

    // ── Ring-3 launch ────────────────────────────────────────────────────────
    vga::write_str("  [ring-3 launch]\n", vga::Color::Cyan);

    // Load user-psh ELF into identity-mapped address space (virt == phys)
    let entry = elf::load(USER_PSH_ELF).expect("ELF parse failed");
    vga::write_str("    psh @ 0x", vga::Color::DimGray);
    vga::write_hex(entry, vga::Color::DimGray);
    vga::write_str("  stack @ 0x", vga::Color::DimGray);
    vga::write_hex(vmm::USER_STACK_TOP, vga::Color::DimGray);
    vga::write_byte(b'\n', vga::Color::White);

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

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    vga::write_str("\nKERNEL PANIC", vga::Color::Red);
    if let Some(loc) = info.location() {
        vga::write_str(" at ", vga::Color::Red);
        vga::write_str(loc.file(), vga::Color::Red);
        vga::write_byte(b':', vga::Color::Red);
        vga::write_i32(loc.line() as i32);
    }
    loop {}
}
