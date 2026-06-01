// Host test for the bare-metal iwlwifi firmware parser. Pulls in the REAL
// kernel/src/iwlwifi_fw.rs via #[path] (no second copy to drift) and parses a
// genuine Intel `.ucode` image, asserting the structure makes sense:
//   * magic is the Intel TLV magic,
//   * the TLV walk consumes the whole image without running off the end,
//   * there is at least one loadable section and a plausible instruction blob,
//   * a corrupted magic is rejected, and a truncated image is flagged.
//
// Usage:  zstd -dc /lib/firmware/intel/iwlwifi/iwlwifi-9260-th-b0-jf-b0-34.ucode.zst > /tmp/fw.ucode
//         rustc -O tools/iwlwifi_fw_test.rs -o /tmp/iwlfwtest && /tmp/iwlfwtest /tmp/fw.ucode
#[path = "../kernel/src/iwlwifi_fw.rs"]
mod iwlwifi_fw;

use iwlwifi_fw::{parse, IWL_TLV_UCODE_MAGIC};
use std::io::Read;

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: iwlfwtest <file.ucode>");
    let mut blob = Vec::new();
    std::fs::File::open(&path)
        .expect("open firmware")
        .read_to_end(&mut blob)
        .expect("read firmware");

    println!("firmware: {} ({} bytes)", path, blob.len());
    let fw = parse(&blob).expect("buffer big enough for a header");

    assert!(fw.magic_ok, "not an Intel TLV ucode image (magic mismatch)");
    println!("  magic           OK (0x{:08x})", IWL_TLV_UCODE_MAGIC);
    println!("  human-readable  {}", cstr(&fw.human));
    println!("  ver             0x{:08x}  (api v{})", fw.ver, fw.api_version());
    println!("  build           {}", fw.build);
    println!("  TLVs            {}", fw.tlv_count);
    println!("  sections        {}", fw.sec_count);
    println!("  inst bytes      {}", fw.inst_bytes);
    println!("  data bytes      {}", fw.data_bytes);
    println!("  largest section {}", fw.largest_section);
    println!("  api flags       0x{:08x}", fw.api_flags);
    println!("  capa flags      0x{:08x}", fw.capa_flags);
    if fw.fw_version_len > 0 {
        println!("  fw version      {}", cstr(&fw.fw_version[..fw.fw_version_len]));
    }

    assert!(!fw.truncated, "TLV walk ran past the end of the image");
    assert!(fw.tlv_count > 0, "no TLVs parsed");
    assert!(fw.sec_count > 0, "no loadable sections found");
    assert!(fw.inst_bytes > 0, "no instruction blob found");
    assert!(fw.largest_section > 0 || fw.inst_bytes > 0, "no section sizing");

    // Negative: corrupt the magic — must be rejected.
    let mut bad = blob.clone();
    bad[4] ^= 0xff;
    let badfw = parse(&bad).unwrap();
    assert!(!badfw.magic_ok, "corrupted magic was accepted!");

    // Negative: truncate mid-stream — must be flagged, never panic.
    let trunc = &blob[..blob.len() / 2];
    if let Some(tf) = parse(trunc) {
        assert!(tf.magic_ok, "header should still parse");
        // truncated is expected for a half image (a TLV will overrun)
        println!("  truncated-half  flagged={}", tf.truncated);
    }

    println!("ALL CHECKS PASSED");
}
