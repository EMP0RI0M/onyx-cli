// Assemble the AArch64 kernel via `cc`, bundle and link statically.
use std::env;

fn main() {
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    println!("cargo:rerun-if-changed=src/dot_kernel.s");
    println!("cargo:rerun-if-changed=build.rs");

    // Only assemble the AArch64 ELF NEON kernel when compiling for aarch64 Linux
    if target_arch != "aarch64" || target_os != "linux" {
        return;
    }

    cc::Build::new()
        .file("src/dot_kernel.s")
        .compile("dotkernel");
}
