fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out = std::env::var("OUT_DIR").unwrap();

    println!("cargo:rerun-if-changed=src/boot.s");
    println!("cargo:rerun-if-changed=linker.ld");

    let boot_s = format!("{}/src/boot.s", manifest);
    let boot_o = format!("{}/boot.o", out);

    let ok = std::process::Command::new("as")
        .args(["--64", &boot_s, "-o", &boot_o])
        .status()
        .expect("GNU as not found — install binutils: sudo apt install binutils")
        .success();

    assert!(ok, "boot.s assembly failed");

    let linker_ld = format!("{}/linker.ld", manifest);
    println!("cargo:rustc-link-arg=-T{}", linker_ld);
    println!("cargo:rustc-link-arg={}", boot_o);
}
