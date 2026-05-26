// Input handling — PS/2 mouse via /dev/input/mice
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::sync::{Arc, Mutex};

pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub buttons: u8,
}

pub fn read_mouse(path: &str) -> Option<File> {
    OpenOptions::new().read(true).open(path).ok()
}

pub fn mouse_thread(state: Arc<Mutex<MouseState>>, width: i32, height: i32) {
    let mut file = match read_mouse("/dev/input/mice") {
        Some(f) => f,
        None => return,
    };

    let mut buf = [0u8; 3];
    loop {
        if file.read_exact(&mut buf).is_err() {
            break;
        }

        // PS/2 packet: byte0 = buttons+flags, byte1 = dx (signed), byte2 = dy (signed)
        let buttons = buf[0] & 0x07;
        let dx = buf[1] as i8 as i32;
        let dy = -(buf[2] as i8 as i32); // Y is inverted on screen

        let mut s = state.lock().unwrap();
        s.x = (s.x + dx).max(0).min(width - 1);
        s.y = (s.y + dy).max(0).min(height - 1);
        s.buttons = buttons;
    }
}
