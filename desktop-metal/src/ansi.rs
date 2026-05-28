// Small streaming parser for the ANSI escape sequences the kernel keyboard
// driver produces. Arrows arrive as ESC [ A/B/C/D, Home/End as ESC [ H / F,
// Delete as ESC [ 3 ~. Everything else is a literal byte.
//
// Usage:
//   let mut p = AnsiParser::new();
//   for &b in keys.iter() {
//       match p.feed(b) {
//           Key::None => {}                  // mid-sequence, swallow
//           Key::Char(c) => handle_char(c),
//           Key::Up | Key::Down | ... => handle_special(...),
//       }
//   }

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Key {
    None,
    Char(u8),
    Up, Down, Left, Right,
    Home, End, Delete,
}

pub struct AnsiParser {
    // 0 = normal, 1 = saw ESC, 2 = saw ESC [, 3 = saw ESC [ 3
    state: u8,
}

impl AnsiParser {
    pub fn new() -> Self { AnsiParser { state: 0 } }

    pub fn feed(&mut self, key: u8) -> Key {
        match self.state {
            1 => {
                self.state = if key == b'[' { 2 } else { 0 };
                Key::None
            }
            2 => {
                self.state = 0;
                match key {
                    b'A' => Key::Up,
                    b'B' => Key::Down,
                    b'C' => Key::Right,
                    b'D' => Key::Left,
                    b'H' => Key::Home,
                    b'F' => Key::End,
                    b'3' => { self.state = 3; Key::None }
                    _ => Key::None,
                }
            }
            3 => {
                self.state = 0;
                if key == b'~' { Key::Delete } else { Key::None }
            }
            _ => {
                if key == 0x1B { self.state = 1; Key::None }
                else { Key::Char(key) }
            }
        }
    }
}
