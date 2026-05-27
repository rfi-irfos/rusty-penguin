// Unified input event ring — keyboard + mouse in a single 64-bit packed format.
//
// Event encoding (64 bits):
//   bits [63:56] = type  0x01=Key  0x02=Mouse
//   Key:   bits [7:0]=ascii  [15:8]=scancode
//   Mouse: bits [15:0]=dx (i16)  [31:16]=dy (i16)  [39:32]=buttons

const RING_SIZE: usize = 256;
static mut RING:  [u64; RING_SIZE] = [0; RING_SIZE];
static mut HEAD:  usize = 0;  // next write
static mut TAIL:  usize = 0;  // next read

fn push(ev: u64) {
    unsafe {
        let next = (HEAD + 1) % RING_SIZE;
        if next != TAIL {
            RING[HEAD] = ev;
            HEAD = next;
        }
    }
}

/// Returns true if at least one event is available.
pub fn has_event() -> bool {
    unsafe { HEAD != TAIL }
}

/// Non-blocking poll — returns Some(event) or None.
pub fn poll() -> Option<u64> {
    unsafe {
        if HEAD == TAIL { return None; }
        let ev = RING[TAIL];
        TAIL = (TAIL + 1) % RING_SIZE;
        Some(ev)
    }
}

/// Blocking wait — enables interrupts, halts until an event arrives, returns it.
pub fn wait() -> u64 {
    loop {
        unsafe {
            core::arch::asm!("sti", options(nostack));
            core::arch::asm!("hlt", options(nostack));
        }
        if let Some(ev) = poll() { return ev; }
    }
}

/// Called from keyboard ISR for each decoded ASCII character.
pub fn push_key(ascii: u8, scancode: u8) {
    let ev: u64 = (0x01u64 << 56)
               | ((scancode as u64) << 8)
               | (ascii as u64);
    push(ev);
}

/// Called from PS/2 mouse ISR after a complete 3-byte packet.
/// Sends absolute cursor position so the desktop doesn't re-accumulate deltas.
pub fn push_mouse(abs_x: u16, abs_y: u16, buttons: u8) {
    let ev: u64 = (0x02u64 << 56)
               | ((buttons as u64) << 32)
               | ((abs_y as u64) << 16)
               | (abs_x as u64);
    push(ev);
}
