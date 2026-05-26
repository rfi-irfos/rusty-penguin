#![no_std]
#![no_main]

mod vga;

use ternary_core::Tryte;
use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn kernel_main(magic: u32, _mb2: u32) {
    vga::clear();

    if magic != 0x36d76289 {
        vga::write_str("ERROR: bad multiboot2 magic\n", vga::Color::Red);
        loop {}
    }

    vga::write_str("  Rusty Penguin v1.0.0 -- bare metal kernel\n", vga::Color::Green);
    vga::write_str("  Binary hardware. Ternary mind.\n\n", vga::Color::Amber);

    let a = Tryte::from_i32(42);
    let b = Tryte::from_i32(-7);
    let c = a + b;
    vga::write_str("  ternary: 42 + (-7) = ", vga::Color::Cyan);
    vga::write_i32(c.to_i32());
    vga::write_str("\n  [halted -- phase 0 complete]\n", vga::Color::White);

    loop {}
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    vga::write_str("\nKERNEL PANIC\n", vga::Color::Red);
    loop {}
}
