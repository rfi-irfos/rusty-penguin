#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod vga;
mod port;
mod pic;
mod idt;
mod memory;

use ternary_core::Tryte;
use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn kernel_main(magic: u32, mb2: u32) {
    vga::clear();

    if magic != 0x36d76289 {
        vga::write_str("ERROR: bad multiboot2 magic\n", vga::Color::Red);
        loop {}
    }

    vga::write_str("  Rusty Penguin v1.0.0 -- bare metal kernel\n", vga::Color::Green);
    vga::write_str("  Binary hardware. Ternary mind.\n\n", vga::Color::Amber);

    // Phase 1: interrupts
    unsafe { pic::init(); }
    idt::init();
    idt::enable();
    vga::write_str("  [interrupts: OK]\n", vga::Color::Green);

    // Memory map from multiboot2
    vga::write_str("  [memory map]\n", vga::Color::Cyan);
    memory::print_map(mb2);
    vga::write_byte(b'\n', vga::Color::White);

    // Ternary demo
    let a = Tryte::from_i32(42);
    let b = Tryte::from_i32(-7);
    let c = a + b;
    vga::write_str("  ternary: 42 + (-7) = ", vga::Color::Cyan);
    vga::write_i32(c.to_i32());
    vga::write_str("\n\n  keyboard active -- type below\n  > ", vga::Color::White);

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
