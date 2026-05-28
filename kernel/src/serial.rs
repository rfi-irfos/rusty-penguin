// COM1 UART driver — 115200 baud 8N1, polled TX
// Mirrors all VGA output to serial so QEMU -serial stdio captures everything.

use crate::port;

const COM1: u16 = 0x3F8;

static mut READY: bool = false;

pub fn init() {
    unsafe {
        port::outb(COM1 + 1, 0x00);  // disable all UART interrupts
        port::outb(COM1 + 3, 0x80);  // DLAB on — access divisor latch
        port::outb(COM1 + 0, 0x01);  // 115200 baud (divisor low = 1)
        port::outb(COM1 + 1, 0x00);  // divisor high = 0
        port::outb(COM1 + 3, 0x03);  // 8 data bits, no parity, 1 stop bit
        port::outb(COM1 + 2, 0xC7);  // FIFO on, clear RX+TX, 14-byte threshold
        port::outb(COM1 + 4, 0x0B);  // RTS + DTR + OUT2
        READY = true;
    }
}

fn tx_ready() -> bool {
    unsafe { port::inb(COM1 + 5) & 0x20 != 0 }
}

fn send(b: u8) {
    unsafe {
        while !tx_ready() {}
        port::outb(COM1, b);
    }
}

/// Write one byte to COM1. Expands \n → \r\n for terminal compatibility.
/// No-op before init() is called.
pub fn write_byte(b: u8) {
    unsafe { if !READY { return; } }
    if b == b'\n' { send(b'\r'); }
    send(b);
}

/// Write a string to COM1, byte-by-byte. Same \n expansion as write_byte.
pub fn write_str(s: &str) {
    for &b in s.as_bytes() { write_byte(b); }
}
