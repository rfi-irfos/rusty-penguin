fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg=-T{}/linker.ld", manifest);
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=src/main.rs");
}
