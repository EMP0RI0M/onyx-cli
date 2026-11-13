// Assemble the AArch64 kernel via `cc`, bundle and link statically.
use std::env;

fn main() {
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    println!("cargo:rerun-if-changed=src/dot_kernel.s");
    println!("cargo:rerun-if-changed=build.rs");

    // Only assemble the AArch64 NEON kernel when compiling for aarch64
    if target_arch != "aarch64" {
        return;
    }

    cc::Build::new()
        .file("src/dot_kernel.s")
        .compile("dotkernel");
}
