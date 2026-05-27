// Input via sys_input_poll (nr=7) — non-blocking drain of the kernel event ring.
// Event encoding: bits[63:56]=type  0x01=Key  0x02=Mouse
//   Key:   bits[7:0]=ascii  [15:8]=scancode
//   Mouse: bits[15:0]=dx i16  [31:16]=dy i16  [39:32]=buttons

pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub buttons: u8,
}

// Up to 8 key bytes per poll (handles multi-byte ESC sequences like ESC [ 3 ~).
// 9 bytes total — stays within two 64-bit registers on x86_64, avoids hidden-pointer ABI.
pub struct Keys {
    buf: [u8; 8],
    len: u8,
}

impl Keys {
    fn new() -> Self { Keys { buf: [0; 8], len: 0 } }
    fn push(&mut self, b: u8) {
        if (self.len as usize) < 8 { self.buf[self.len as usize] = b; self.len += 1; }
    }
    pub fn iter(&self) -> &[u8] { &self.buf[..(self.len as usize).min(8)] }
}

fn sys_input_poll() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 7u64 => n,
            in("rdi") 0u64,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    n
}

/// Drain all pending input events. Updates mouse state. Returns all ASCII key bytes.
/// Multiple bytes per call is essential for ESC sequences (3–4 bytes each).
pub fn poll(mouse: &mut MouseState, width: i32, height: i32) -> Keys {
    let mut keys = Keys::new();
    loop {
        let ev = sys_input_poll();
        if ev == 0 { break; }
        match (ev >> 56) as u8 {
            0x01 => {
                let ascii = (ev & 0xFF) as u8;
                if ascii != 0 { keys.push(ascii); }
            }
            0x02 => {
                let dx = (ev & 0xFFFF) as u16 as i16 as i32;
                let dy = ((ev >> 16) & 0xFFFF) as u16 as i16 as i32;
                mouse.buttons = ((ev >> 32) & 0xFF) as u8;
                mouse.x = (mouse.x + dx).max(0).min(width  - 1);
                // Keep cursor below topbar and high enough that the sprite doesn't vanish.
                // Topbar is 28px; CURSOR_H=24, so max y keeps the sprite inside the screen.
                mouse.y = (mouse.y + dy).max(28).min(height - 1);
            }
            _ => {}
        }
    }
    keys
}
