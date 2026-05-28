// PS/2 mouse driver — IRQ12, 3-byte relative packets.
// After init(), each mouse interrupt accumulates a 3-byte packet
// [flags, dx, dy] and appends a decoded event to the input queue.
// Cursor rendering is handled entirely by the ring-3 desktop process.

use crate::port;
use crate::fb;

static mut PACKET: [u8; 3] = [0; 3];
static mut PACKET_IDX: u8 = 0;

static mut MOUSE_X: i32 = 0;
static mut MOUSE_Y: i32 = 0;

// Wait until PS/2 controller input buffer is empty (bit 1 clear)
unsafe fn ps2_wait_write() {
    let mut n = 0usize;
    while port::inb(0x64) & 0x02 != 0 { n += 1; if n > 100_000 { break; } }
}

// Wait until PS/2 controller output buffer has data (bit 0 set)
unsafe fn ps2_wait_read() {
    let mut n = 0usize;
    while port::inb(0x64) & 0x01 == 0 { n += 1; if n > 100_000 { break; } }
}

// Send command to PS/2 mouse (via controller port 0x64 + 0x60)
unsafe fn mouse_write(byte: u8) {
    ps2_wait_write();
    port::outb(0x64, 0xD4);  // "send next byte to mouse"
    ps2_wait_write();
    port::outb(0x60, byte);
}

// Read one byte from PS/2 controller output buffer
unsafe fn ps2_read() -> u8 {
    ps2_wait_read();
    port::inb(0x60)
}

pub fn init() {
    unsafe {
        // Enable auxiliary (mouse) port
        ps2_wait_write();
        port::outb(0x64, 0xA8);

        // Enable mouse interrupts: read, set bit 1, write back
        ps2_wait_write();
        port::outb(0x64, 0x20);         // read current command byte
        let mut cfg = ps2_read();
        cfg |= 0x03;                    // bits 0+1: enable keyboard IRQ1 + mouse IRQ12
        cfg &= !0x30;                   // bits 4+5: clear port-disable (enable both clocks)
        ps2_wait_write();
        port::outb(0x64, 0x60);
        ps2_wait_write();
        port::outb(0x60, cfg);

        // Reset mouse
        mouse_write(0xFF);
        ps2_read();  // ACK (0xFA)
        ps2_read();  // AA (reset OK)
        ps2_read();  // 00 (device ID)

        // Set default settings
        mouse_write(0xF6);
        ps2_read();

        // Enable streaming mode
        mouse_write(0xF4);
        ps2_read();

        // Centre cursor at screen midpoint
        let w = fb::width() as i32;
        let h = fb::height() as i32;
        MOUSE_X = if w > 0 { w / 2 } else { 320 };
        MOUSE_Y = if h > 0 { h / 2 } else { 200 };
    }
}

/// Called from IRQ12 handler. Accumulates 3-byte packets and fires an
/// input event when a complete packet is ready.
pub fn handle_irq() {
    let byte = unsafe { port::inb(0x60) };
    unsafe {
        let idx = PACKET_IDX as usize;
        // First byte of every PS/2 mouse packet always has bit 3 set.
        // Resync the accumulator if we're out of phase.
        if idx == 0 && (byte & 0x08) == 0 { return; }
        PACKET[idx] = byte;
        PACKET_IDX += 1;

        if PACKET_IDX == 3 {
            PACKET_IDX = 0;
            let flags = PACKET[0];
            let dx_raw = PACKET[1] as i8;
            let dy_raw = PACKET[2] as i8;

            // Ignore if overflow bits set
            if flags & 0xC0 != 0 { return; }

            let buttons = flags & 0x07;

            // Correct 9-bit PS/2 signed delta: sign bit lives in the flags byte,
            // not in the data byte. Casting to i8 gives wrong sign for values 128-255.
            let x_neg = (flags & 0x10) != 0;
            let y_neg = (flags & 0x20) != 0;
            let dx = if x_neg { (dx_raw as i32) - 256 } else { dx_raw as i32 };
            let dy_raw9 = if y_neg { (dy_raw as i32) - 256 } else { dy_raw as i32 };
            let dy = -dy_raw9;  // PS/2 Y is inverted vs screen coords

            let w = fb::width() as i32;
            let h = fb::height() as i32;
            MOUSE_X = (MOUSE_X + dx).clamp(0, if w > 0 { w - 1 } else { 639 });
            MOUSE_Y = (MOUSE_Y + dy).clamp(0, if h > 0 { h - 1 } else { 479 });

            // Send absolute position so desktop doesn't re-accumulate potentially-wrong deltas.
            crate::input::push_mouse(MOUSE_X as u16, MOUSE_Y as u16, buttons);
        }
    }
}
