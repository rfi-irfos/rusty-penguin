// Host test for the bare-metal WPA2 authentication core. Pulls in the REAL
// kernel/src/wpa2.rs via #[path] (no second copy to drift) and checks every
// primitive against its canonical published vector:
//   * SHA-1                FIPS 180-1 ("abc", "")
//   * HMAC-SHA1            RFC 2202 cases 1 & 2
//   * PBKDF2-HMAC-SHA1     RFC 6070 (c = 1, 2, 4096)
//   * WPA passphrase→PMK   IEEE 802.11i §H.4 ("password" / "IEEE")
//   * PTK (802.11i PRF)    determinism + nonce-dependence + 48-byte CCMP split
//
// Usage:  rustc -O tools/wpa2_test.rs -o /tmp/wpa2test && /tmp/wpa2test
#[path = "../kernel/src/wpa2.rs"]
mod wpa2;

use wpa2::*;

fn hx(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn main() {
    // SHA-1
    assert_eq!(hx(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(hx(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    println!("SHA-1                FIPS 180-1            OK");

    // HMAC-SHA1
    assert_eq!(hx(&hmac_sha1(&[0x0b; 20], b"Hi There")),
               "b617318655057264e28bc0b6fb378c8ef146be00");
    assert_eq!(hx(&hmac_sha1(b"Jefe", b"what do ya want for nothing?")),
               "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79");
    println!("HMAC-SHA1            RFC 2202 case 1+2     OK");

    // PBKDF2-HMAC-SHA1 (RFC 6070)
    let mut d = [0u8; 20];
    pbkdf2_sha1(b"password", b"salt", 1, &mut d);
    assert_eq!(hx(&d), "0c60c80f961f0e71f3a9b524af6012062fe037a6");
    pbkdf2_sha1(b"password", b"salt", 2, &mut d);
    assert_eq!(hx(&d), "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957");
    pbkdf2_sha1(b"password", b"salt", 4096, &mut d);
    assert_eq!(hx(&d), "4b007901b765489abead49d926f721d065a429c1");
    println!("PBKDF2-HMAC-SHA1     RFC 6070 c=1,2,4096   OK");

    // WPA passphrase → PMK (IEEE 802.11i §H.4)
    let psk = wpa_passphrase_to_psk(b"password", b"IEEE");
    assert_eq!(hx(&psk),
        "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e");
    println!("WPA passphrase→PMK   IEEE 802.11i H.4      OK");
    println!("                     PMK = {}", hx(&psk));

    // PTK expansion (802.11i PRF) — determinism + nonce dependence + CCMP split
    let aa = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05];
    let spa = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15];
    let anonce = [0x22u8; 32];
    let snonce1 = [0x33u8; 32];
    let snonce2 = [0x44u8; 32];
    let (mut k1, mut k1b, mut k2) = ([0u8; 48], [0u8; 48], [0u8; 48]);
    ptk(&psk, &aa, &spa, &anonce, &snonce1, &mut k1);
    ptk(&psk, &aa, &spa, &anonce, &snonce1, &mut k1b);
    ptk(&psk, &aa, &spa, &anonce, &snonce2, &mut k2);
    assert_eq!(k1, k1b, "PTK must be deterministic for fixed inputs");
    assert_ne!(k1, k2, "PTK must change when SNonce changes");
    println!("PTK (Pairwise PRF)   deterministic+nonced  OK");
    println!("                     KCK = {}", hx(&k1[0..16]));
    println!("                     KEK = {}", hx(&k1[16..32]));
    println!("                     TK  = {}", hx(&k1[32..48]));

    // And the module's own boot-time self-test agrees.
    assert!(selftest(), "wpa2::selftest() failed");
    println!("\nwpa2::selftest()                          ALL VECTORS PASS");
}
