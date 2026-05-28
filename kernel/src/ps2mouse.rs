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

fn ser(b: u8) { crate::serial::write_byte(b); }

pub fn init() {
    unsafe {
        ser(b'['); ser(b'I'); ser(b'N'); ser(b'I'); ser(b'T'); ser(b']'); ser(b'\n');

        // Mask IRQ12 at the PIC slave (bit 4) to prevent the mouse's reset-response
        // bytes from being consumed by irq_mouse during polled init reads.
        // The BIOS may leave IRQ12 unmasked; pic::init() restores the BIOS mask.
        let pic2_mask = port::inb(0xA1);
        port::outb(0xA1, pic2_mask | 0x10);  // mask IRQ12 (slave PIC bit 4)

        // Enable auxiliary (mouse) port
        ps2_wait_write();
        port::outb(0x64, 0xA8);
        ser(b'1'); ser(b'\n');

        // Enable mouse IRQs and both port clocks in the PS/2 command byte.
        // IRQ12 is masked in the PIC, so enabling it here doesn't let the
        // handler fire — it just allows the PS/2 output buffer to fill on
        // mouse data (some controllers require bit 1 set to forward aux bytes).
        ps2_wait_write();
        port::outb(0x64, 0x20);
        let mut cfg = ps2_read();
        cfg |= 0x03;    // bits 0+1: keyboard IRQ1 + mouse IRQ12 in PS/2 config
        cfg &= !0x30;   // bits 4+5: enable both port clocks
        ps2_wait_write();
        port::outb(0x64, 0x60);
        ps2_wait_write();
        port::outb(0x60, cfg);
        ser(b'2'); ser(b'\n');

        // Reset mouse — IRQ12 is PIC-masked so polled reads get the bytes
        mouse_write(0xFF);
        let ack1 = ps2_read();  // 0xFA ACK
        let aa   = ps2_read();  // 0xAA self-test OK
        let did  = ps2_read();  // 0x00 device ID
        let hx = |v: u8, n: u8| -> u8 { let nib = (v >> (n*4)) & 0xF; if nib < 10 { b'0'+nib } else { b'a'+nib-10 } };
        ser(b'3');
        ser(hx(ack1,1)); ser(hx(ack1,0));
        ser(hx(aa,1));   ser(hx(aa,0));
        ser(hx(did,1));  ser(hx(did,0));
        ser(b'\n');

        // Set default settings
        mouse_write(0xF6);
        let ack2 = ps2_read();
        ser(b'4'); ser(hx(ack2,1)); ser(hx(ack2,0)); ser(b'\n');

        // Enable streaming mode
        mouse_write(0xF4);
        let ack3 = ps2_read();
        ser(b'5'); ser(hx(ack3,1)); ser(hx(ack3,0)); ser(b'\n');

        // Centre cursor at screen midpoint
        let w = fb::width() as i32;
        let h2 = fb::height() as i32;
        MOUSE_X = if w > 0 { w / 2 } else { 320 };
        MOUSE_Y = if h2 > 0 { h2 / 2 } else { 200 };

        // Leave IRQ12 masked in PIC — kernel main calls pic::unmask_mouse() next,
        // which unmasks it now that the mouse is in streaming mode and the init
        // polled reads are done.

        ser(b'['); ser(b'M'); ser(b'S'); ser(b'E'); ser(b' ');
        ser(b'O'); ser(b'K'); ser(b']'); ser(b'\n');
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

            // Ignore if overflow bits set
            if flags & 0xC0 != 0 {
                crate::serial::write_byte(b'!');  // overflow
                return;
            }

            let buttons = flags & 0x07;

            // PS/2 9-bit signed delta: the 9th (sign) bit lives in flags[4] / flags[5],
            // not in the data byte.  Keep data bytes as unsigned (0..255) and subtract
            // 256 when the sign bit is set — do NOT cast to i8 first, that would
            // incorrectly sign-extend values ≥ 128 and flip the cursor direction.
            let x_neg = (flags & 0x10) != 0;
            let y_neg = (flags & 0x20) != 0;
            let dx_u = PACKET[1] as i32;  // 0..255
            let dy_u = PACKET[2] as i32;  // 0..255
            let dx = if x_neg { dx_u - 256 } else { dx_u };
            let dy = -(if y_neg { dy_u - 256 } else { dy_u }); // PS/2 Y inverted vs screen

            let w = fb::width() as i32;
            let h = fb::height() as i32;
            MOUSE_X = (MOUSE_X + dx).clamp(0, if w > 0 { w - 1 } else { 639 });
            MOUSE_Y = (MOUSE_Y + dy).clamp(0, if h > 0 { h - 1 } else { 479 });

            // Send absolute position so desktop doesn't re-accumulate potentially-wrong deltas.
            crate::input::push_mouse(MOUSE_X as u16, MOUSE_Y as u16, buttons);
        }
    }
}
