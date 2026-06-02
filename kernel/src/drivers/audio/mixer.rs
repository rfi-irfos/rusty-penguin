//! Native Audio Mixer for Rusty Penguin.
//! Mixes multi-stream audio inputs into a single output buffer.

use crate::drivers::udi::{UniversalDriver, register_driver};
use crate::serial;

pub struct AudioMixer;

impl UniversalDriver for AudioMixer {
    fn init(&self) -> bool {
        serial::write_str("  [udi] Initializing Native Audio Mixer...\n");
        true
    }
    fn handle_interrupt(&self, _irq: u8) { }
    fn control(&self, code: u32, _input: &[u8], _output: &mut [u8]) -> u32 {
        // Handle channel volume/sample rate
        0
    }
}

pub fn udi_init() {
    register_driver(&AudioMixer);
}

/// Mix audio streams into the output buffer.
pub fn mix_stream(buffer: &mut [i16], stream_data: &[i16]) {
    for (out, inp) in buffer.iter_mut().zip(stream_data.iter()) {
        *out = out.saturating_add(*inp);
    }
}
