//! WPA2 EAPOL-Key 4-way handshake (IEEE 802.11i / 802.1X) — the protocol that,
//! given the PMK, derives the PTK and installs the unicast (PTK) + group (GTK)
//! keys with the AP. This is the supplicant (STA) side, the step between "we know
//! the passphrase" and "we can send encrypted frames". Pure `core`, no alloc,
//! host-testable; built entirely on the already-verified [wpa2] (PTK + HMAC-SHA1)
//! and [aes] (RFC 3394 GTK unwrap) primitives.
//!
//! Frame layout per message (after the 802.1X header) is the EAPOL-Key body:
//!   descriptor_type(1) key_info(2) key_length(2) replay_counter(8) key_nonce(32)
//!   key_iv(16) key_rsc(8) key_id(8) key_mic(16) key_data_length(2) key_data(var)
//! The MIC (WPA2/AKM-SHA1) is HMAC-SHA1(KCK, whole EAPOL frame with MIC zeroed)[0:16].

#![allow(dead_code)]

use crate::wpa2;
use crate::aes;

// key_info bit fields we care about.
const KI_KEY_MIC: u16   = 1 << 8;
const KI_SECURE: u16    = 1 << 9;
const KI_ACK: u16       = 1 << 7;
const KI_INSTALL: u16   = 1 << 6;
const KI_ENCRYPTED: u16 = 1 << 12; // key_data is encrypted (M3 GTK)

// Offsets within the EAPOL-Key frame (4-byte 802.1X header + body).
const H: usize = 4;                 // 802.1X header length
const OFF_KEYINFO: usize  = H + 1;
const OFF_NONCE: usize    = H + 13;
const OFF_MIC: usize      = H + 77;
const OFF_KDLEN: usize    = H + 93;
const OFF_KDATA: usize    = H + 95;
const FRAME_FIXED: usize  = OFF_KDATA;   // bytes before key_data

fn be16(b: &[u8], o: usize) -> u16 { ((b[o] as u16) << 8) | b[o + 1] as u16 }
fn wbe16(b: &mut [u8], o: usize, v: u16) { b[o] = (v >> 8) as u8; b[o + 1] = v as u8; }

/// Compute the WPA2 EAPOL-Key MIC over `frame` (MIC field assumed zeroed) using
/// the KCK (PTK[0:16]); the MIC is HMAC-SHA1 truncated to 16 bytes.
pub fn key_mic(kck: &[u8], frame: &[u8]) -> [u8; 16] {
    let full = wpa2::hmac_sha1(kck, frame);
    let mut m = [0u8; 16];
    m.copy_from_slice(&full[0..16]);
    m
}

/// Result of a completed 4-way handshake.
pub struct Keys {
    pub ptk: [u8; 48], // KCK(16) ‖ KEK(16) ‖ TK(16)
    pub gtk: [u8; 32], // group key (len depends on cipher; up to 32)
    pub gtk_len: usize,
}

/// Supplicant 4-way handshake state machine. Feed it message 1 then message 3
/// (the AP→STA frames); it produces message 2 then message 4 (STA→AP) and, on
/// success, the installed PTK + GTK.
pub struct Supplicant {
    pmk: [u8; 32],
    aa: [u8; 6],   // authenticator (AP) MAC
    spa: [u8; 6],  // supplicant (our) MAC
    snonce: [u8; 32],
    pub rsn_ie: [u8; 32], // our RSN information element (sent in M2 key_data)
    pub rsn_ie_len: usize,
    ptk: [u8; 48],
    have_ptk: bool,
}

impl Supplicant {
    pub fn new(pmk: [u8; 32], aa: [u8; 6], spa: [u8; 6], snonce: [u8; 32]) -> Self {
        Supplicant { pmk, aa, spa, snonce, rsn_ie: [0; 32], rsn_ie_len: 0, ptk: [0; 48], have_ptk: false }
    }

    fn derive_ptk(&mut self, anonce: &[u8; 32]) {
        wpa2::ptk(&self.pmk, &self.aa, &self.spa, anonce, &self.snonce, &mut self.ptk);
        self.have_ptk = true;
    }

    /// Process EAPOL message 1 (AP→STA, carries ANonce). Derives the PTK and
    /// writes message 2 (STA→AP) into `out`, returning its length, or 0 on error.
    pub fn on_message1(&mut self, m1: &[u8], out: &mut [u8]) -> usize {
        if m1.len() < FRAME_FIXED { return 0; }
        let mut anonce = [0u8; 32];
        anonce.copy_from_slice(&m1[OFF_NONCE..OFF_NONCE + 32]);
        self.derive_ptk(&anonce);

        // Build M2: copy the replay counter from M1, set our SNonce + RSN IE,
        // key_info = MIC bit (+ pairwise/version bits mirrored from M1's low bits).
        let kd = self.rsn_ie_len.min(out.len().saturating_sub(FRAME_FIXED));
        let total = FRAME_FIXED + kd;
        if out.len() < total { return 0; }
        for b in out[..total].iter_mut() { *b = 0; }
        // 802.1X header: version 2, type 3 (EAPOL-Key), length = body len.
        out[0] = 2; out[1] = 3; wbe16(out, 2, (total - H) as u16);
        out[H] = m1[H]; // descriptor_type (RSN=2)
        // key_info: keep M1's version/type bits, set MIC, clear ACK.
        let ki = (be16(m1, OFF_KEYINFO) & 0x0007) | KI_KEY_MIC;
        wbe16(out, OFF_KEYINFO, ki);
        // key_length + replay_counter mirrored from M1.
        out[H + 2] = m1[H + 2]; out[H + 3] = m1[H + 3];
        out[H + 5..H + 13].copy_from_slice(&m1[H + 5..H + 13]);
        // SNonce.
        out[OFF_NONCE..OFF_NONCE + 32].copy_from_slice(&self.snonce);
        // key_data = our RSN IE.
        wbe16(out, OFF_KDLEN, kd as u16);
        out[OFF_KDATA..OFF_KDATA + kd].copy_from_slice(&self.rsn_ie[..kd]);
        // MIC over the whole frame with the MIC field zeroed (already zero).
        let mic = key_mic(&self.ptk[0..16], &out[..total]);
        out[OFF_MIC..OFF_MIC + 16].copy_from_slice(&mic);
        total
    }

    /// Verify the MIC on EAPOL message 3 (AP→STA). The KCK is PTK[0:16].
    pub fn verify_message3(&self, m3: &[u8]) -> bool {
        if !self.have_ptk || m3.len() < FRAME_FIXED { return false; }
        let mut tmp = [0u8; 256];
        if m3.len() > tmp.len() { return false; }
        tmp[..m3.len()].copy_from_slice(m3);
        for b in tmp[OFF_MIC..OFF_MIC + 16].iter_mut() { *b = 0; }
        let want = key_mic(&self.ptk[0..16], &tmp[..m3.len()]);
        want[..] == m3[OFF_MIC..OFF_MIC + 16]
    }

    /// Process message 3 (AP→STA): verify its MIC, unwrap the GTK from key_data
    /// (RFC 3394 with the KEK = PTK[16:32]), write message 4 (STA→AP) into `out`.
    /// Returns (m4_len, Keys) on success.
    pub fn on_message3(&self, m3: &[u8], out: &mut [u8]) -> Option<(usize, Keys)> {
        if !self.verify_message3(m3) { return None; }
        let kek = &self.ptk[16..32];
        let kdlen = be16(m3, OFF_KDLEN) as usize;
        if OFF_KDATA + kdlen > m3.len() { return None; }
        let mut keys = Keys { ptk: self.ptk, gtk: [0; 32], gtk_len: 0 };
        // If key_data is encrypted (the GTK KDE wrapped with AES key-wrap), unwrap
        // it. We unwrap the whole key_data blob; the GTK KDE lives inside it.
        if be16(m3, OFF_KEYINFO) & KI_ENCRYPTED != 0 && kdlen >= 16 && kdlen % 8 == 0 {
            let mut kekk = [0u8; 16]; kekk.copy_from_slice(kek);
            let mut unwrapped = [0u8; 64];
            let outlen = kdlen - 8;
            if outlen <= unwrapped.len()
                && aes::key_unwrap(&kekk, &m3[OFF_KDATA..OFF_KDATA + kdlen], &mut unwrapped[..outlen]) {
                // Extract the GTK from the GTK KDE: 00-0F-AC type 1, dl_kid(2),
                // then the GTK. We scan for the KDE OUI/type rather than hard-parse.
                if let Some((g, gl)) = find_gtk_kde(&unwrapped[..outlen]) {
                    keys.gtk[..gl].copy_from_slice(g);
                    keys.gtk_len = gl;
                }
            }
        }
        // Build M4: key_info = MIC|Secure, no nonce, no key_data, MIC over the frame.
        let total = FRAME_FIXED;
        if out.len() < total { return None; }
        for b in out[..total].iter_mut() { *b = 0; }
        out[0] = 2; out[1] = 3; wbe16(out, 2, (total - H) as u16);
        out[H] = m3[H];
        wbe16(out, OFF_KEYINFO, (be16(m3, OFF_KEYINFO) & 0x0007) | KI_KEY_MIC | KI_SECURE);
        out[H + 2] = m3[H + 2]; out[H + 3] = m3[H + 3];
        out[H + 5..H + 13].copy_from_slice(&m3[H + 5..H + 13]); // replay counter
        wbe16(out, OFF_KDLEN, 0);
        let mic = key_mic(&self.ptk[0..16], &out[..total]);
        out[OFF_MIC..OFF_MIC + 16].copy_from_slice(&mic);
        Some((total, keys))
    }
}

/// Find the GTK inside an unwrapped key_data blob: a KDE is dd LEN 00 0F AC <type>
/// <data>; type 1 = GTK (data = key_id/tx(2) + GTK bytes).
fn find_gtk_kde(kd: &[u8]) -> Option<(&[u8], usize)> {
    let mut i = 0;
    while i + 2 <= kd.len() {
        if kd[i] != 0xdd { break; }
        let len = kd[i + 1] as usize;
        if i + 2 + len > kd.len() || len < 4 { break; }
        let body = &kd[i + 2..i + 2 + len];
        if body[0] == 0x00 && body[1] == 0x0f && body[2] == 0xac && body[3] == 0x01 {
            // GTK KDE: after OUI(3)+type(1) comes key-id/tx(2), then the GTK.
            let gtk = &body[6..];
            let gl = gtk.len().min(32);
            return Some((&gtk[..gl], gl));
        }
        i += 2 + len;
    }
    None
}

// ───────────────────────────── Self-test ────────────────────────────────────

/// End-to-end test of the 4-way handshake against a *self-consistent* AP: we play
/// both sides (a tiny authenticator that derives the same PTK, builds M1/M3 and
/// verifies our M2/M4), so every step — PTK derivation, the EAPOL MIC on M2/M4,
/// M3 MIC verification, and the RFC-3394 GTK unwrap — is exercised against a peer
/// that independently recomputes it. The primitives underneath ([wpa2], [aes]) are
/// each separately verified against published vectors.
pub fn selftest() -> bool {
    let pmk = wpa2::wpa_passphrase_to_psk(b"password", b"IEEE");
    let aa = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let spa = [0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb];
    let anonce = [0x11u8; 32];
    let snonce = [0x22u8; 32];
    // Both sides derive the same PTK.
    let mut ptk = [0u8; 48];
    wpa2::ptk(&pmk, &aa, &spa, &anonce, &snonce, &mut ptk);

    let mut sup = Supplicant::new(pmk, aa, spa, snonce);
    sup.rsn_ie_len = 4;
    sup.rsn_ie[..4].copy_from_slice(&[0x30, 0x02, 0x01, 0x00]); // minimal stub RSN IE

    // AP builds M1 (ANonce, ACK).
    let mut m1 = [0u8; FRAME_FIXED];
    m1[0] = 2; m1[1] = 3; wbe16(&mut m1, 2, (FRAME_FIXED - H) as u16);
    m1[H] = 2; // RSN
    wbe16(&mut m1, OFF_KEYINFO, 0x0002 | KI_ACK);
    m1[H + 12] = 1; // replay counter = 1
    m1[OFF_NONCE..OFF_NONCE + 32].copy_from_slice(&anonce);

    // STA processes M1 → M2.
    let mut m2 = [0u8; 128];
    let m2len = sup.on_message1(&m1, &mut m2);
    if m2len == 0 { return false; }
    // AP verifies M2's MIC + that it carries our SNonce.
    {
        let mut t = [0u8; 128]; t[..m2len].copy_from_slice(&m2[..m2len]);
        for b in t[OFF_MIC..OFF_MIC + 16].iter_mut() { *b = 0; }
        let want = key_mic(&ptk[0..16], &t[..m2len]);
        if want[..] != m2[OFF_MIC..OFF_MIC + 16] { return false; }
        if m2[OFF_NONCE..OFF_NONCE + 32] != snonce[..] { return false; }
    }

    // AP builds M3: wrap a GTK in a GTK-KDE with the KEK, set ENCRYPTED|MIC|ACK.
    let gtk = [0xCCu8; 16];
    let mut kde = [0u8; 24];           // dd LEN 00 0F AC 01 keyid(2) GTK(16) → 6+2+16=24 body... build:
    kde[0] = 0xdd; kde[1] = (4 + 2 + 16) as u8; // len = OUI(3)+type(1)+keyid(2)+gtk(16)
    kde[2] = 0x00; kde[3] = 0x0f; kde[4] = 0xac; kde[5] = 0x01; // GTK KDE
    kde[6] = 0; kde[7] = 0;            // key id / tx
    kde[8..24].copy_from_slice(&gtk);
    // pad key_data to an 8-byte multiple for key-wrap.
    let mut plain = [0u8; 24];
    plain.copy_from_slice(&kde);
    let mut kekk = [0u8; 16]; kekk.copy_from_slice(&ptk[16..32]);
    let mut wrapped = [0u8; 32];
    if !aes::key_wrap(&kekk, &plain, &mut wrapped) { return false; }
    let kdlen = 32usize;
    let m3total = FRAME_FIXED + kdlen;
    let mut m3 = [0u8; 160];
    m3[0] = 2; m3[1] = 3; wbe16(&mut m3, 2, (m3total - H) as u16);
    m3[H] = 2;
    wbe16(&mut m3, OFF_KEYINFO, 0x0002 | KI_KEY_MIC | KI_ACK | KI_INSTALL | KI_SECURE | KI_ENCRYPTED);
    m3[H + 12] = 2; // replay counter = 2
    m3[OFF_NONCE..OFF_NONCE + 32].copy_from_slice(&anonce);
    wbe16(&mut m3, OFF_KDLEN, kdlen as u16);
    m3[OFF_KDATA..OFF_KDATA + kdlen].copy_from_slice(&wrapped);
    // AP MICs M3.
    let mic3 = key_mic(&ptk[0..16], &m3[..m3total]);
    m3[OFF_MIC..OFF_MIC + 16].copy_from_slice(&mic3);

    // STA processes M3 → M4 + installs keys.
    let mut m4 = [0u8; 128];
    let (m4len, keys) = match sup.on_message3(&m3[..m3total], &mut m4) { Some(x) => x, None => return false };
    if m4len == 0 { return false; }
    // The unwrapped GTK must match, and the installed PTK must match the AP's.
    if keys.gtk_len != 16 || keys.gtk[..16] != gtk[..] { return false; }
    if keys.ptk[..] != ptk[..] { return false; }
    // AP verifies M4's MIC.
    {
        let mut t = [0u8; 128]; t[..m4len].copy_from_slice(&m4[..m4len]);
        for b in t[OFF_MIC..OFF_MIC + 16].iter_mut() { *b = 0; }
        let want = key_mic(&ptk[0..16], &t[..m4len]);
        if want[..] != m4[OFF_MIC..OFF_MIC + 16] { return false; }
    }
    true
}
