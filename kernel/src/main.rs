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

use ternary_core::{Trit, Tryte};
use mathematics::{mul_tryte, consensus, scale};
use hardware_abstraction::{TernaryALU, SoftwareALU};
use ai_runtime::{TernaryTensor, TernaryLinear};
use core::panic::PanicInfo;

extern "C" { static kernel_end: u8; }

#[no_mangle]
pub extern "C" fn kernel_main(magic: u32, mb2: u32) {
    vga::clear();

    if magic != 0x36d76289 {
        vga::write_str("ERROR: bad multiboot2 magic\n", vga::Color::Red);
        loop {}
    }

    vga::write_str("  Rusty Penguin v1.0.0 -- bare metal kernel\n", vga::Color::Green);
    vga::write_str("  Binary hardware. Ternary mind.\n\n", vga::Color::Amber);

    // Heap for Vec/Box in ai-runtime
    allocator::init();

    // Proper GDT (null | kcode | kdata | TSS) + load TSS
    gdt::init();
    vga::write_str("  [GDT+TSS: OK]\n", vga::Color::Green);

    // PIC + IDT
    unsafe { pic::init(); }
    idt::init();
    idt::enable();
    vga::write_str("  [interrupts: OK]\n", vga::Color::Green);

    // Physical memory map
    vga::write_str("  [memory map]\n", vga::Color::Cyan);
    memory::print_map(mb2);
    vga::write_byte(b'\n', vga::Color::White);

    // Bitmap page allocator
    let kend = core::ptr::addr_of!(kernel_end) as u64;
    pmm::init(mb2, kend);
    let (free, total) = pmm::stats();
    vga::write_str("  [PMM] ", vga::Color::Cyan);
    vga::write_i32((free / 256) as i32);
    vga::write_str(" MiB free / ", vga::Color::White);
    vga::write_i32((total / 256) as i32);
    vga::write_str(" MiB total  (", vga::Color::White);
    vga::write_i32(free as i32);
    vga::write_str(" frames)\n", vga::Color::White);

    // Test: allocate + free one frame
    if let Some(frame) = pmm::alloc_frame() {
        vga::write_str("  [PMM] alloc test: 0x", vga::Color::Green);
        vga::write_hex(frame, vga::Color::Green);
        pmm::free_frame(frame);
        vga::write_str(" [freed]\n\n", vga::Color::Green);
    }

    // Ternary mathematics
    vga::write_str("  [mathematics]\n", vga::Color::Cyan);
    let a = Tryte::from_i32(42);
    let b = Tryte::from_i32(-7);
    vga::write_str("    42 + (-7)  = ", vga::Color::White);
    vga::write_i32((a + b).to_i32());
    vga::write_byte(b'\n', vga::Color::White);

    let (lo, _hi) = mul_tryte(Tryte::from_i32(6), Tryte::from_i32(7));
    vga::write_str("    6 * 7      = ", vga::Color::White);
    vga::write_i32(lo.to_i32());
    vga::write_byte(b'\n', vga::Color::White);

    vga::write_str("    scale(100,-)= ", vga::Color::White);
    vga::write_i32(scale(Tryte::from_i32(100), Trit::Neg).to_i32());
    vga::write_byte(b'\n', vga::Color::White);

    let con = consensus(Trit::Pos, Trit::Pos);
    vga::write_str("    cons(+,+)  = ", vga::Color::White);
    vga::write_str(match con { Trit::Pos => "Pos", Trit::Neg => "Neg", Trit::Zero => "Zero" }, vga::Color::Green);
    vga::write_byte(b'\n', vga::Color::White);

    // Hardware ALU
    let alu = SoftwareALU;
    let r = alu.add(Tryte::from_i32(9), Tryte::from_i32(3));
    vga::write_str("    ALU 9+3    = ", vga::Color::White);
    vga::write_i32(r.to_i32());
    vga::write_byte(b'\n', vga::Color::White);
    vga::write_byte(b'\n', vga::Color::White);

    // Sparse AI inference
    vga::write_str("  [sparse AI inference]\n", vga::Color::Cyan);
    let mut layer = TernaryLinear::new(4, 2);
    layer.weights.data = alloc::vec![
        Trit::Pos,  Trit::Zero, Trit::Neg, Trit::Pos,
        Trit::Neg,  Trit::Pos,  Trit::Zero, Trit::Neg,
    ];
    let input = TernaryTensor::new(
        alloc::vec![Trit::Pos, Trit::Zero, Trit::Neg, Trit::Pos],
        alloc::vec![4],
    );
    let (output, total_ops, skipped) = layer.forward(&input);
    vga::write_str("    input:  [+, 0, -, +]\n", vga::Color::White);
    vga::write_str("    output: [", vga::Color::White);
    for (i, t) in output.data.iter().enumerate() {
        if i > 0 { vga::write_str(", ", vga::Color::White); }
        vga::write_str(match t { Trit::Pos => "+", Trit::Neg => "-", Trit::Zero => "0" }, vga::Color::Amber);
    }
    vga::write_str("]\n    ops: ", vga::Color::White);
    vga::write_i32(total_ops as i32);
    vga::write_str(" total, ", vga::Color::White);
    vga::write_i32(skipped as i32);
    let pct = (skipped * 100) / total_ops.max(1);
    vga::write_str(" skipped (", vga::Color::White);
    vga::write_i32(pct as i32);
    vga::write_str("% zero-dormancy)\n    heap used: ", vga::Color::White);
    vga::write_i32(allocator::used() as i32);
    vga::write_str(" bytes\n\n", vga::Color::White);

    vga::write_str("  keyboard active -- type below\n  > ", vga::Color::White);

    loop {
        unsafe { core::arch::asm!("hlt", options(nostack)); }
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
