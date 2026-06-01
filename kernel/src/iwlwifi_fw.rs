//! Intel iwlwifi firmware-image (`.ucode`) parser — brick 1 of bare-metal WiFi.
//!
//! Before an Intel WiFi card can do anything it must be handed a firmware image
//! the kernel loads from disk and DMAs into the device. Those images use Intel's
//! TLV ucode format. This module parses one: validate the magic, read the
//! version, and walk every TLV to inventory the instruction/data/section blobs
//! the driver will later upload. It's the very first thing a real iwlwifi driver
//! does, and — unlike the device bring-up — it's pure byte-wrangling, so it is
//! fully testable on the host against a genuine Intel blob (`tools/iwlwifi_fw_test.rs`).
//!
//! Format (from Linux `iwl-fw-file.h`):
//!   struct iwl_tlv_ucode_header { le32 zero; le32 magic; u8 human[64];
//!                                 le32 ver; le32 build; le64 ignore; u8 data[]; }
//!   struct iwl_ucode_tlv        { le32 type; le32 length; u8 data[]; }  // 4-byte aligned
//!
//! No kernel deps, no alloc — only `core` — so it drops straight into a host
//! test the same way `bignum.rs` does.

#![allow(dead_code)]

pub const IWL_TLV_UCODE_MAGIC: u32 = 0x0a4c_5749; // "IWL\n"

// The TLV types we care to recognise (there are ~60; these are the load-bearing
// ones for inventorying an image).
const TLV_INST: u32 = 1; // legacy runtime instructions
const TLV_DATA: u32 = 2; // legacy runtime data
const TLV_INIT: u32 = 3; // legacy init instructions
const TLV_INIT_DATA: u32 = 4; // legacy init data
const TLV_PROBE_MAX_LEN: u32 = 6;
const TLV_SEC_RT: u32 = 19; // modern runtime section (offset-prefixed)
const TLV_SEC_INIT: u32 = 20; // modern init section
const TLV_SEC_WOWLAN: u32 = 21; // wake-on-wlan section
const TLV_API_CHANGES_SET: u32 = 29;
const TLV_ENABLED_CAPABILITIES: u32 = 30;
const TLV_FW_VERSION: u32 = 36;

const HEADER_LEN: usize = 88; // zero+magic+human(64)+ver+build+ignore(8)

#[inline]
fn rd_u32(b: &[u8], off: usize) -> u32 {
    (b[off] as u32)
        | ((b[off + 1] as u32) << 8)
        | ((b[off + 2] as u32) << 16)
        | ((b[off + 3] as u32) << 24)
}

/// What we learned about a firmware image.
#[derive(Clone)]
pub struct FwInfo {
    pub magic_ok: bool,
    pub ver: u32,
    pub build: u32,
    pub human: [u8; 64], // human-readable version string (NUL-padded)
    pub tlv_count: u32,
    pub sec_count: u32,        // number of loadable sections (modern + legacy)
    pub inst_bytes: u32,       // total instruction bytes (INST + SEC_RT payloads)
    pub data_bytes: u32,       // total data bytes (DATA + INIT_DATA payloads)
    pub largest_section: u32,  // biggest single section — sizes the DMA staging
    pub api_flags: u32,        // first dword of API_CHANGES_SET, if present
    pub capa_flags: u32,       // first dword of ENABLED_CAPABILITIES, if present
    pub fw_version: [u8; 48],  // FW_VERSION TLV text (NUL-padded)
    pub fw_version_len: usize,
    pub truncated: bool,       // a TLV ran past the end of the blob
}

impl FwInfo {
    fn empty() -> Self {
        FwInfo {
            magic_ok: false,
            ver: 0,
            build: 0,
            human: [0; 64],
            tlv_count: 0,
            sec_count: 0,
            inst_bytes: 0,
            data_bytes: 0,
            largest_section: 0,
            api_flags: 0,
            capa_flags: 0,
            fw_version: [0; 48],
            fw_version_len: 0,
            truncated: false,
        }
    }
    /// In the modern TLV layout the header `ver` dword is the plain firmware API
    /// number — e.g. 0x22 (34) for `iwlwifi-9260-…-34.ucode`. The driver matches
    /// this against the API range its host commands speak.
    pub fn api_version(&self) -> u32 {
        self.ver
    }
}

/// Parse an iwlwifi `.ucode` image. Returns None only if the buffer is too small
/// to hold a header; otherwise returns FwInfo with `magic_ok` reflecting whether
/// it's actually an Intel TLV image (so callers can log a precise reason).
pub fn parse(blob: &[u8]) -> Option<FwInfo> {
    if blob.len() < HEADER_LEN {
        return None;
    }
    let mut fw = FwInfo::empty();

    let zero = rd_u32(blob, 0);
    let magic = rd_u32(blob, 4);
    // Legacy (pre-2012) images have a non-zero first dword; we only handle the
    // modern TLV layout, which begins with a zero dword then the magic.
    fw.magic_ok = zero == 0 && magic == IWL_TLV_UCODE_MAGIC;
    if !fw.magic_ok {
        return Some(fw);
    }

    fw.human.copy_from_slice(&blob[8..72]);
    fw.ver = rd_u32(blob, 72);
    fw.build = rd_u32(blob, 76);

    // Walk the TLV stream.
    let mut off = HEADER_LEN;
    while off + 8 <= blob.len() {
        let ttype = rd_u32(blob, off);
        let tlen = rd_u32(blob, off + 4) as usize;
        let data_off = off + 8;
        if data_off + tlen > blob.len() {
            fw.truncated = true;
            break;
        }
        let data = &blob[data_off..data_off + tlen];
        fw.tlv_count += 1;

        match ttype {
            TLV_INST | TLV_INIT => fw.inst_bytes = fw.inst_bytes.wrapping_add(tlen as u32),
            TLV_DATA | TLV_INIT_DATA => fw.data_bytes = fw.data_bytes.wrapping_add(tlen as u32),
            TLV_SEC_RT | TLV_SEC_INIT | TLV_SEC_WOWLAN => {
                // Section TLVs are a le32 destination offset followed by payload.
                let payload = tlen.saturating_sub(4) as u32;
                fw.sec_count += 1;
                if ttype == TLV_SEC_RT {
                    fw.inst_bytes = fw.inst_bytes.wrapping_add(payload);
                }
                if payload > fw.largest_section {
                    fw.largest_section = payload;
                }
            }
            TLV_API_CHANGES_SET => {
                if tlen >= 4 {
                    fw.api_flags = rd_u32(data, 0);
                }
            }
            TLV_ENABLED_CAPABILITIES => {
                if tlen >= 4 {
                    fw.capa_flags = rd_u32(data, 0);
                }
            }
            TLV_FW_VERSION => {
                let n = tlen.min(fw.fw_version.len() - 1);
                fw.fw_version[..n].copy_from_slice(&data[..n]);
                fw.fw_version_len = n;
            }
            _ => {}
        }

        // Advance: TLV payloads are padded up to a 4-byte boundary.
        let padded = (tlen + 3) & !3;
        off = data_off + padded;
    }

    // Count the legacy monolithic blobs as "sections" too, for a useful total.
    if fw.sec_count == 0 {
        if fw.inst_bytes > 0 {
            fw.sec_count += 1;
        }
        if fw.data_bytes > 0 {
            fw.sec_count += 1;
        }
    }
    Some(fw)
}
