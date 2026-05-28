// Input via sys_input_poll (nr=7) — non-blocking drain of the kernel event ring.
// Event encoding: bits[63:56]=type  0x01=Key  0x02=Mouse
//   Key:   bits[7:0]=ascii  [15:8]=scancode
//   Mouse: bits[15:0]=abs_x u16  [31:16]=abs_y u16  [39:32]=buttons
//   Mouse coords are absolute, clamped by kernel to [0, fb_w-1] × [0, fb_h-1].

pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub buttons: u8,
    pub btn_pressed: u8,  // bits that transitioned 0→1 this poll cycle
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
/// width/height kept for API compatibility but mouse coords are already clamped by kernel.
pub fn poll(mouse: &mut MouseState, _width: i32, _height: i32) -> Keys {
    let mut keys = Keys::new();
    mouse.btn_pressed = 0;
    let mut cur = mouse.buttons;
    loop {
        let ev = sys_input_poll();
        if ev == 0 { break; }
        match (ev >> 56) as u8 {
            0x01 => {
                let ascii = (ev & 0xFF) as u8;
                if ascii != 0 { keys.push(ascii); }
            }
            0x02 => {
                // Absolute position from kernel-tracked MOUSE_X / MOUSE_Y
                mouse.x = (ev & 0xFFFF) as u16 as i32;
                mouse.y = ((ev >> 16) & 0xFFFF) as u16 as i32;
                let new_b = ((ev >> 32) & 0xFF) as u8;
                mouse.btn_pressed |= new_b & !cur;  // capture 0→1 edges within this drain
                cur = new_b;
            }
            _ => {}
        }
    }
    mouse.buttons = cur;
    keys
}
