// Input via sys_input_poll (nr=7) — non-blocking drain of the kernel event ring.
// Event encoding: bits[63:56]=type  0x01=Key  0x02=Mouse
//   Key:   bits[7:0]=ascii  [15:8]=scancode
//   Mouse: bits[15:0]=dx i16  [31:16]=dy i16  [39:32]=buttons

pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub buttons: u8,
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

/// Drain all pending input events. Updates mouse state. Returns first ASCII key, if any.
pub fn poll(mouse: &mut MouseState, width: i32, height: i32) -> Option<u8> {
    let mut key: Option<u8> = None;
    loop {
        let ev = sys_input_poll();
        if ev == 0 { break; }
        match (ev >> 56) as u8 {
            0x01 => {
                let ascii = (ev & 0xFF) as u8;
                if key.is_none() && ascii != 0 { key = Some(ascii); }
            }
            0x02 => {
                let dx = (ev & 0xFFFF) as u16 as i16 as i32;
                let dy = ((ev >> 16) & 0xFFFF) as u16 as i16 as i32;
                mouse.buttons = ((ev >> 32) & 0xFF) as u8;
                mouse.x = (mouse.x + dx).max(0).min(width  - 1);
                mouse.y = (mouse.y + dy).max(0).min(height - 1);
            }
            _ => {}
        }
    }
    key
}
