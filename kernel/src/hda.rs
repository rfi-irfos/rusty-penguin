// Intel High Definition Audio (HDA) driver for the bare-metal Rusty Penguin kernel.
//
// QEMU emulates the ICH6 HDA controller (PCI 0x8086:0x2668, BAR0 = MMIO 64KB).
// We set up the CORB/RIRB rings to talk to the codec, find the first audio output
// widget, configure a PCM stream, fill a DMA buffer with a sine wave, and play it.
//
// Ternary lens: each codec node state is a Trit (+1 active / 0 dormant / -1 off).
// The stream state: +1 running, 0 idle, -1 error.

use ternary_core::Trit;

// ── Register offsets ──────────────────────────────────────────────────────────
const GCAP:    usize = 0x00;  // Global Capabilities
const GCTL:    usize = 0x08;  // Global Control
const STATESTS:usize = 0x0E;  // State Change Status
const INTCTL:  usize = 0x20;  // Interrupt Control
const CORBLBASE:usize= 0x40;  // CORB Lower Base Address
const CORBUBASE:usize= 0x44;  // CORB Upper Base Address
const CORBWP:  usize = 0x48;  // CORB Write Pointer
const CORBRP:  usize = 0x4A;  // CORB Read Pointer
const CORBCTL: usize = 0x4C;  // CORB Control
const CORBSIZE:usize = 0x4E;  // CORB Size
const RIRBLBASE:usize= 0x50;  // RIRB Lower Base Address
const RIRBUBASE:usize= 0x54;  // RIRB Upper Base Address
const RIRBWP:  usize = 0x58;  // RIRB Write Pointer
const RIRBCTL: usize = 0x5C;  // RIRB Control
const RIRBSIZE:usize = 0x5E;  // RIRB Size
const DPLBASE: usize = 0x70;  // DMA Position Lower Base
const DPUBASE: usize = 0x74;  // DMA Position Upper Base
// Stream Descriptors: input streams first, then output streams.
// GCAP[15:12]=OSS (output streams), GCAP[11:8]=ISS (input streams).
// For QEMU q35 with GCAP=0x4401: ISS=4 inputs, OSS=4 outputs.
// Output SD 0 is at base + 0x80 + ISS*0x20 = base + 0x80 + 4*0x20 = base + 0x100.
// We compute this at runtime from GCAP to handle different hardware.
// Placeholder; the real offset is calculated in hda_init from GCAP.
const SD0_BASE:usize = 0x80;  // updated at runtime; see sd_out_base()
const SD_CTL:  usize = 0x00;  // stream control
const SD_STS:  usize = 0x03;  // stream status
const SD_LPIB: usize = 0x04;  // link position in buffer
const SD_CBL:  usize = 0x08;  // cyclic buffer length
const SD_LVI:  usize = 0x0C;  // last valid BDL index
const SD_FIFOW:usize = 0x0E;  // FIFO size
const SD_FMT:  usize = 0x12;  // stream format
const SD_BDLPL:usize = 0x18;  // BDL physical addr low
const SD_BDLPU:usize = 0x1C;  // BDL physical addr high

// ── MMIO helpers ─────────────────────────────────────────────────────────────
unsafe fn r32(base: u64, off: usize) -> u32 { ((base + off as u64) as *const u32).read_volatile() }
unsafe fn r16(base: u64, off: usize) -> u16 { ((base + off as u64) as *const u16).read_volatile() }
unsafe fn r8 (base: u64, off: usize) -> u8  { ((base + off as u64) as *const u8 ).read_volatile() }
unsafe fn w32(base: u64, off: usize, v: u32) { ((base + off as u64) as *mut u32).write_volatile(v) }
unsafe fn w16(base: u64, off: usize, v: u16) { ((base + off as u64) as *mut u16).write_volatile(v) }
unsafe fn w8 (base: u64, off: usize, v: u8 ) { ((base + off as u64) as *mut u8 ).write_volatile(v) }

fn spin(n: u64) { for _ in 0..n { unsafe { core::arch::asm!("pause", options(nomem,nostack)); } } }

// ── Verb helpers ──────────────────────────────────────────────────────────────
// Codec address 0, node ID, verb 12-bit, parameter 8-bit.
fn verb(cad: u8, nid: u8, verb_id: u16, payload: u16) -> u32 {
    ((cad as u32) << 28) | ((nid as u32) << 20) | ((verb_id as u32) << 8) | (payload as u32)
}
fn verb20(cad: u8, nid: u8, verb_id: u8, payload: u32) -> u32 {
    ((cad as u32) << 28) | ((nid as u32) << 20) | ((verb_id as u32 | 0x700) << 8) | (payload & 0xF_FFFF)
}

// ── DMA buffers — placed in the identity-mapped arena at fixed addresses ─────
// DMA buffers at 50 MiB (0x03200000) — safely above the desktop-metal heap
// (which extends from 4 MiB to ~36 MiB) and the initrd relocation (40 MiB),
// but below the ring-3 stack (63 MiB), all inside the 64 MiB identity map.
const CORB_PHYS:  u64 = 0x0320_0000;  // 50 MiB: CORB (1 KB)
const RIRB_PHYS:  u64 = 0x0320_0400;  // RIRB  (2 KB)
const BDL_PHYS:   u64 = 0x0320_1000;  // BDL   (256 B)
const AUDIO_PHYS: u64 = 0x0320_2000;  // PCM audio buffer (128 KB)
const AUDIO_BYTES:u32 = 0x0002_0000;  // 128 KB cycle buffer

// ── Sine wave generator ───────────────────────────────────────────────────────
fn gen_sine(buf: *mut i16, n_samples: usize) {
    // 440 Hz tone at 44100 Hz, stereo 16-bit. Period = 44100/440 = 100.23 samples.
    // Integer: period = 100, which gives 441 Hz — close enough.
    const PERIOD: usize = 100;
    // Quarter-sine table (25 values, 0..π/2)
    const Q: [i16; 26] = [0,2057,4106,6140,8149,10126,12062,13951,
                           15785,17557,19260,20886,22430,23886,25247,26509,
                           27666,28714,29648,30465,31160,31730,32168,32473,
                           32642,32767];
    let sine = |phase: usize| -> i16 {
        let p = phase % PERIOD;
        let idx_f = p * 100 / PERIOD;  // 0..99 maps to 0..99 of period
        // Map 0..99 → sin(0..2π): 4 quadrants over PERIOD
        let q = (idx_f * 25) / 25;  // simplified: quarter index
        let rem = (idx_f * 26) / 100;
        let rem = rem.min(25);
        let amp = if idx_f < 25      { Q[rem] }
                  else if idx_f < 50 { Q[25 - (idx_f - 25).min(25)] }
                  else if idx_f < 75 { -(Q[(idx_f - 50).min(25)]) }
                  else               { -(Q[25 - (idx_f - 75).min(25)]) };
        let _ = q;
        amp
    };
    for i in 0..n_samples {
        let s = sine(i);
        unsafe { *buf.add(i * 2)     = s; }  // left
        unsafe { *buf.add(i * 2 + 1) = s; }  // right
    }
}

// ── Global state ─────────────────────────────────────────────────────────────
// Cached after init so userspace writes and volume changes don't need re-init.
static mut HDA_BASE:    u64  = 0;
static mut HDA_SD_OUT:  usize = 0;
static mut HDA_READY:   bool = false;
static mut HDA_OUT_NID: u8   = 0;
static mut CORB_WP_PTR: u8   = 0;

/// Write raw stereo 16-bit PCM into the DMA audio buffer starting at `offset`
/// (byte offset into AUDIO_PHYS). `data` length is clamped to the buffer size.
/// Callers can double-buffer by writing to offset 0 while the first half plays
/// (IOC fires at the half-way boundary). Returns bytes written.
pub fn audio_write(data: &[u8], offset: usize) -> usize {
    unsafe {
        if !HDA_READY { return 0; }
        let max = AUDIO_BYTES as usize;
        if offset >= max { return 0; }
        let avail = max - offset;
        let n = data.len().min(avail);
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            (AUDIO_PHYS + offset as u64) as *mut u8,
            n,
        );
        n
    }
}

/// Set the master output volume (0–127). Writes the gain verb via ICS.
pub fn set_volume(vol: u8) {
    unsafe {
        if !HDA_READY { return; }
        let base = HDA_BASE;
        let nid  = HDA_OUT_NID;
        let v = vol.min(127) as u32;
        // Both output amp verbs: left (0x03) and right (0x04), in-amplifier.
        for gain_verb in [verb20(0, nid, 0x03, v), verb20(0, nid, 0x04, v)] {
            ics_send(base, gain_verb);
        }
    }
}

// Send one verb via the Immediate Command interface (ICS/IC/IR registers).
const IC:  usize = 0x60;
const IR:  usize = 0x64;
const ICS: usize = 0x68;
unsafe fn ics_send(base: u64, v: u32) {
    let mut t = 0u32;
    while r16(base, ICS) & 0x1 != 0 && t < 100_000 { spin(10); t += 1; }
    w32(base, IC, v);
    w16(base, ICS, (r16(base, ICS) & !0x3) | 0x1);
    t = 0;
    while t < 200_000 { let ics = r16(base, ICS); if ics & 0x1 == 0 { break; } spin(10); t += 1; }
}

/// Returns true if HDA is up and DMA is running.
pub fn is_ready() -> bool { unsafe { HDA_READY } }

/// Size of the looping PCM ring buffer in bytes (the rpv player streams into it).
pub fn audio_ring_bytes() -> usize { AUDIO_BYTES as usize }

// ── Main init function ────────────────────────────────────────────────────────
/// Initialise Intel HDA, find the first output widget, and begin playback
/// of a 440 Hz test tone. Returns a ternary Trit indicating the outcome:
/// Pos (+1) = playing, Zero = no HDA device found, Neg = init error.
pub fn init() -> Trit {
    // 1. Find the HDA controller on the PCI bus.
    // QEMU ICH6 HDA: vendor 0x8086, device 0x2668.
    // Try both ICH6 (0x2668, for -machine pc) and ICH9 (0x293E, for q35).
    let found = crate::pci::find(0x8086, 0x2668)
        .or_else(|| crate::pci::find(0x8086, 0x293E));
    let (bus, dev, func) = match found {
        Some(b) => b,
        None => {
            crate::serial::write_str("  [hda] no Intel HDA found\n");
            return Trit::Zero;
        }
    };
    crate::serial::write_str("  [hda] found HDA controller\n");
    crate::pci::enable_bus_master(bus, dev, func);

    // 2. Map MMIO (BAR0). Check if 64-bit BAR (type bits [2:1] = 10b).
    let bar0_raw = crate::pci::bar(bus, dev, func, 0);
    let bar_type = (bar0_raw >> 1) & 0x3;
    let bar0 = if bar_type == 2 {
        // 64-bit BAR: low 32 in BAR0, high 32 in BAR1
        let lo = (bar0_raw & !0xF) as u64;
        let hi = crate::pci::bar(bus, dev, func, 1) as u64;
        lo | (hi << 32)
    } else {
        (bar0_raw & !0xF) as u64
    };
    if bar0 == 0 {
        crate::serial::write_str("  [hda] BAR0 not set\n");
        return Trit::Neg;
    }
    crate::vmm::map_mmio_range(bar0, 0x4000);  // map 16 KB MMIO region

    unsafe { hda_init(bar0) }
}

// Compute the MMIO offset of the first output stream descriptor from GCAP.
// ISS = GCAP[11:8]; each descriptor is 0x20 bytes; inputs come first.
fn sd_out_base(gcap: u32) -> usize {
    let iss = ((gcap >> 8) & 0xF) as usize;
    0x80 + iss * 0x20
}

unsafe fn hda_init(base: u64) -> Trit {
    // 3. Reset the controller (GCTL.CRST = 0, then 1).
    w32(base, GCTL, 0);
    spin(5000);
    w32(base, GCTL, 1);
    let mut tries = 0u32;
    while r32(base, GCTL) & 1 == 0 && tries < 100_000 { spin(100); tries += 1; }
    if r32(base, GCTL) & 1 == 0 {
        crate::serial::write_str("  [hda] controller reset failed\n");
        return Trit::Neg;
    }
    spin(50_000);  // codec detect time

    // Read GCAP to find the output stream descriptor offset.
    let gcap = r32(base, GCAP);
    let sd_out = sd_out_base(gcap);  // offset to first output stream descriptor
    crate::serial::write_str("  [hda] out_sd_offset=0x");
    crate::serial::write_hex_u32(sd_out as u32);
    crate::serial::write_byte(b'\n');

    // Check at least one codec is present via STATESTS.
    let statests = r16(base, STATESTS);
    if statests == 0 {
        crate::serial::write_str("  [hda] no codec detected\n");
        return Trit::Neg;
    }
    crate::serial::write_str("  [hda] codec detected\n");

    // 4. Set up CORB.
    // Stop CORB DMA first, then configure, then start.
    w8(base, CORBCTL, 0x00);
    spin(10_000);
    core::ptr::write_bytes(CORB_PHYS as *mut u8, 0, 1024);
    w32(base, CORBLBASE, CORB_PHYS as u32);
    w32(base, CORBUBASE, 0);
    w8 (base, CORBSIZE, 0x02);           // 256 entries (bits[1:0]=10)
    // Reset CORBWP and CORBRP to 0.
    w16(base, CORBWP, 0x0000);
    w16(base, CORBRP, 0x8000);  // CRST bit — hardware clears it immediately on some impls
    spin(10_000);
    w16(base, CORBRP, 0x0000);
    spin(5_000);
    w8 (base, CORBCTL, 0x02);            // DMA run (bit 1)
    spin(5_000);
    let corbctl_v = r8(base, CORBCTL);
    let corbrp_v  = r16(base, CORBRP);
    let _ = (corbctl_v, corbrp_v);

    // 5. Set up RIRB.
    w8(base, RIRBCTL, 0x00);
    spin(10_000);
    core::ptr::write_bytes(RIRB_PHYS as *mut u8, 0, 2048);
    w32(base, RIRBLBASE, RIRB_PHYS as u32);
    w32(base, RIRBUBASE, 0);
    w8 (base, RIRBSIZE, 0x02);           // 256 entries
    // Reset RIRBWP: set bit 15 to reset
    w16(base, RIRBWP, 0x8000);
    spin(1_000);
    w8 (base, RIRBCTL, 0x02);            // DMA run (bit 1)
    spin(5_000);

    // 6. Send a verb and get a response.
    // Use the Immediate Command Interface (ICS/IR/IRS) instead of CORB/RIRB.
    // The ICH6/ICH9 HDA spec defines at offsets 0x60/0x64/0x68:
    // Enable immediate command mode in GCTL (bit 1 = FCNTRL, not needed; ICS is always available)
    let dummy_wp = 0u16;
    let send_verb = |_corb_wp: &mut u16, v: u32| {
        // Wait until not busy
        let mut t = 0u32;
        while r16(base, ICS) & 0x1 != 0 && t < 100_000 { spin(10); t += 1; }
        w32(base, IC, v);
        // Set ICB bit to trigger the command
        w16(base, ICS, (r16(base, ICS) & !0x3) | 0x1);
        // Wait for ICB to clear (command complete) and IRV to set
        t = 0;
        while t < 200_000 {
            let ics = r16(base, ICS);
            if ics & 0x1 == 0 { break; }  // not busy
            spin(10); t += 1;
        }
    };
    let read_rirb = || -> u32 {
        // IRV bit set means a valid response is available
        let ics = r16(base, ICS);
        if ics & 0x2 != 0 {
            let resp = r32(base, IR);
            // Clear IRV
            w16(base, ICS, ics & !0x2);
            resp
        } else {
            0
        }
    };
    let mut corb_wp = dummy_wp;

    // Get codec vendor/device (verb 0xF00 param 0x00 = Vendor ID)
    send_verb(&mut corb_wp, verb(0, 0, 0xF00, 0x00));
    let codec_id = read_rirb();

    // Get the subordinate node count of the root node (nid=0, F00/04)
    send_verb(&mut corb_wp, verb(0, 0, 0xF00, 0x04));
    let sub = read_rirb();
    let start_nid = (sub >> 16) & 0xFF;
    let count     = sub & 0xFF;

    // Find an Audio Function Group (type=1 in F00/05)
    let mut afg_nid = 0u8;
    for n in start_nid..(start_nid + count) {
        send_verb(&mut corb_wp, verb(0, n as u8, 0xF00, 0x05));
        if read_rirb() & 0xFF == 1 { afg_nid = n as u8; break; }
    }
    if afg_nid == 0 {
        crate::serial::write_str("  [hda] no AFG found\n");
        return Trit::Neg;
    }

    // Power up AFG (D0)
    send_verb(&mut corb_wp, verb20(0, afg_nid, 0x05, 0));  // SetPowerState D0

    // Get AFG subordinate nodes
    send_verb(&mut corb_wp, verb(0, afg_nid, 0xF00, 0x04));
    let afg_sub  = read_rirb();
    let w_start  = (afg_sub >> 16) & 0xFF;
    let w_count  = afg_sub & 0xFF;

    // Find the first audio output converter (type 0 in F00/09)
    let mut out_nid = 0u8;
    for n in w_start..(w_start + w_count) {
        send_verb(&mut corb_wp, verb(0, n as u8, 0xF00, 0x09));
        let typ = (read_rirb() >> 20) & 0xF;
        if typ == 0 { out_nid = n as u8; break; }  // Audio Output
    }
    if out_nid == 0 {
        crate::serial::write_str("  [hda] no output converter\n");
        return Trit::Neg;
    }
    crate::serial::write_str("  [hda] output converter nid=");
    crate::serial::write_byte(b'0' + out_nid % 10);
    crate::serial::write_byte(b'\n');

    // Find and enable a pin widget connected to the output (type 4)
    for n in w_start..(w_start + w_count) {
        send_verb(&mut corb_wp, verb(0, n as u8, 0xF00, 0x09));
        let info = read_rirb();
        if (info >> 20) & 0xF == 4 {
            // SetPinControl: enable output
            send_verb(&mut corb_wp, verb20(0, n as u8, 0x07, 0x40));
            // SetEAPD_BTL: enable EAPD
            send_verb(&mut corb_wp, verb20(0, n as u8, 0x0C, 0x02));
            break;
        }
    }

    // 7. Configure the output converter: 44100 Hz, 16-bit, stereo.
    // SetConverter format: 48 kHz base, /1 divisor, 16-bit, 2ch = 0x0011
    // or 44.1 kHz base (bit 14=1), /1, 16-bit, 2ch = 0x4011
    send_verb(&mut corb_wp, verb(0, out_nid, 0xA00, 0x4011));  // 44.1 kHz, 16-bit, 2ch
    send_verb(&mut corb_wp, verb20(0, out_nid, 0x06, 0x10));   // set stream 1 (bits[7:4]=1), channel 0
    send_verb(&mut corb_wp, verb20(0, out_nid, 0x03, 0x7F));   // set gain = max
    send_verb(&mut corb_wp, verb20(0, out_nid, 0x04, 0x7F));   // set left/right gain

    // 8. Generate a 440 Hz sine wave into the DMA buffer.
    let n_samples = (AUDIO_BYTES / 4) as usize; // stereo 16-bit = 4 bytes/frame
    core::ptr::write_bytes(AUDIO_PHYS as *mut u8, 0, AUDIO_BYTES as usize);
    gen_sine(AUDIO_PHYS as *mut i16, n_samples);

    // 9. Build the BDL (Buffer Descriptor List) using explicit 32-bit writes.
    // Each entry: [u64 phys_addr][u32 length_bytes][u32 ioc_flag]
    let bdl = BDL_PHYS as *mut u32;
    let half = AUDIO_BYTES / 2;
    // Entry 0: first half of audio buffer, no IOC
    *bdl.add(0) = AUDIO_PHYS as u32;           // addr low
    *bdl.add(1) = 0;                           // addr high
    *bdl.add(2) = half;                        // length
    *bdl.add(3) = 0;                           // IOC = 0
    // Entry 1: second half, IOC = 1
    *bdl.add(4) = (AUDIO_PHYS + half as u64) as u32;  // addr low
    *bdl.add(5) = 0;                           // addr high
    *bdl.add(6) = half;                        // length
    *bdl.add(7) = 1;                           // IOC = 1 (interrupt on completion)

    // 10. Configure the first OUTPUT stream descriptor (offset from GCAP).
    w8(base, sd_out + SD_CTL, 0x00);          // stop
    spin(1000);
    w8(base, sd_out + SD_CTL, 0x01);          // SRST — reset descriptor
    spin(5000);
    // Wait for reset to complete (SRST self-clears)
    let mut t2 = 0u32;
    while r8(base, sd_out + SD_CTL) & 0x01 != 0 && t2 < 50_000 { spin(10); t2 += 1; }
    w8(base, sd_out + SD_CTL, 0x00);          // clear

    w32(base, sd_out + SD_CBL,   AUDIO_BYTES);
    w16(base, sd_out + SD_LVI,   1);           // last valid index = 1 (2 BDL entries)
    w16(base, sd_out + SD_FMT,   0x4011);      // 44.1 kHz, 16-bit, 2ch
    w32(base, sd_out + SD_BDLPL, BDL_PHYS as u32);
    w32(base, sd_out + SD_BDLPU, 0);
    w8(base,  sd_out + SD_STS,   0x1C);        // clear status bits
    spin(1_000);
    // Start: stream tag=1 in bits[23:20], IRQ enables in bits[4:2], RUN in bit 1.
    w32(base, sd_out + SD_CTL, (1u32 << 20) | 0x1C | 0x02);
    spin(10_000);

    // Quick sanity: LPIB should be non-zero if DMA is running
    let lpib = r32(base, sd_out + SD_LPIB);
    crate::serial::write_str("  [hda] LPIB="); crate::serial::write_hex_u32(lpib); crate::serial::write_byte(b'\n');
    crate::serial::write_str("  [hda] playback started (440 Hz, 44.1 kHz, stereo)\n");
    // Cache state for runtime audio_write / set_volume calls.
    HDA_BASE    = base;
    HDA_SD_OUT  = sd_out;
    HDA_OUT_NID = out_nid;
    HDA_READY   = true;
    Trit::Pos
}
