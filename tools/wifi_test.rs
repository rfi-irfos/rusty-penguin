// Host test for the bare-metal WiFi crypto/protocol layer above the radio. Pulls
// in the REAL kernel modules via #[path] (no second copy to drift):
//   * AES-128                FIPS-197 §C.1 known-answer
//   * AES Key Wrap/Unwrap    RFC 3394 §4.1 known-answer (the GTK unwrap)
//   * WPA2 PMK/PTK           IEEE 802.11i (re-checked here)
//   * EAPOL 4-way handshake  end-to-end against a self-consistent AP
//
// Usage:  rustc -O tools/wifi_test.rs -o /tmp/wifitest && /tmp/wifitest
#[path = "../kernel/src/wpa2.rs"]  mod wpa2;
#[path = "../kernel/src/aes.rs"]   mod aes;
#[path = "../kernel/src/eapol.rs"] mod eapol;

fn hx(b: &[u8]) -> String { b.iter().map(|x| format!("{:02x}", x)).collect() }

fn main() {
    // AES-128 FIPS-197
    let key = [0u8,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15];
    let pt  = [0x00,0x11,0x22,0x33,0x44,0x55,0x66,0x77,0x88,0x99,0xaa,0xbb,0xcc,0xdd,0xee,0xff];
    let aesk = aes::Aes128::new(&key);
    assert_eq!(hx(&aesk.encrypt_block(&pt)), "69c4e0d86a7b0430d8cdb78070b4c55a");
    assert_eq!(aesk.decrypt_block(&aesk.encrypt_block(&pt)), pt);
    println!("AES-128              FIPS-197 C.1          OK");

    // RFC 3394 key wrap (128-bit KEK, 128-bit key)
    let kd = [0x00,0x11,0x22,0x33,0x44,0x55,0x66,0x77,0x88,0x99,0xaa,0xbb,0xcc,0xdd,0xee,0xff];
    let mut wrapped = [0u8; 24];
    assert!(aes::key_wrap(&key, &kd, &mut wrapped));
    assert_eq!(hx(&wrapped), "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5");
    let mut un = [0u8; 16];
    assert!(aes::key_unwrap(&key, &wrapped, &mut un));
    assert_eq!(un, kd);
    println!("AES Key Wrap         RFC 3394 4.1          OK");

    assert!(aes::selftest(), "aes::selftest failed");

    // WPA2 PMK (cross-check)
    let pmk = wpa2::wpa_passphrase_to_psk(b"password", b"IEEE");
    assert_eq!(hx(&pmk), "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e");
    println!("WPA2 PMK             IEEE 802.11i H.4      OK");

    // EAPOL 4-way handshake end-to-end (supplicant vs self-consistent AP).
    assert!(eapol::selftest(), "eapol::selftest (4-way handshake) failed");
    println!("EAPOL 4-way          M1->M2, M3 MIC+GTK unwrap->M4, PTK installed  OK");

    println!("\nWiFi crypto/protocol layer (above the radio)  ALL VECTORS PASS");
}
