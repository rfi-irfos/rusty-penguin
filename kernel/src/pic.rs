use crate::port::{inb, outb, io_wait};

const PIC1_CMD:  u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD:  u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

const ICW1_INIT: u8 = 0x11;  // init sequence + ICW4 required
const ICW4_8086: u8 = 0x01;  // 8086/88 mode
pub const EOI:   u8 = 0x20;

pub unsafe fn init() {
    // Save existing masks
    let m1 = inb(PIC1_DATA);
    let m2 = inb(PIC2_DATA);

    // ICW1: start initialization
    outb(PIC1_CMD,  ICW1_INIT); io_wait();
    outb(PIC2_CMD,  ICW1_INIT); io_wait();
    // ICW2: vector offsets — remap IRQs away from CPU exception vectors
    outb(PIC1_DATA, 32);        io_wait();  // master: IRQ0-7  → vectors 32-39
    outb(PIC2_DATA, 40);        io_wait();  // slave:  IRQ8-15 → vectors 40-47
    // ICW3: cascade wiring
    outb(PIC1_DATA, 0b0000_0100); io_wait(); // master: slave on IRQ2 (bit 2)
    outb(PIC2_DATA, 2);           io_wait(); // slave: identity = 2
    // ICW4: 8086 mode
    outb(PIC1_DATA, ICW4_8086); io_wait();
    outb(PIC2_DATA, ICW4_8086); io_wait();

    // Restore masks — but always unmask IRQ0 (timer) and IRQ1 (keyboard)
    outb(PIC1_DATA, m1 & 0xFC); // clear bits 0 and 1
    outb(PIC2_DATA, m2);
}

// Call at the end of every IRQ handler. irq = 0-15.
pub unsafe fn eoi(irq: u8) {
    if irq >= 8 {
        outb(PIC2_CMD, EOI);
    }
    outb(PIC1_CMD, EOI);
}
