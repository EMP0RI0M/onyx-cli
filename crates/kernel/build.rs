// Assemble the AArch64 kernel with system `as`, bundle with `ar`, link statically.
use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    let out = env::var("OUT_DIR").unwrap();
    let obj = Path::new(&out).join("dot_kernel.o");
    let ar_obj = Path::new(&out).join("dot_kernel_ar.o");
    let lib = Path::new(&out).join("libdotkernel.a");

    let st = Command::new("as")
        .arg("src/dot_kernel.s")
        .arg("-o")
        .arg(&obj)
        .status()
        .expect("failed to run `as`");
    assert!(st.success(), "assembler failed");

    std::fs::copy(&obj, &ar_obj).unwrap();
    let _ = std::fs::remove_file(&lib);
    let st = Command::new("ar")
        .arg("crs")
        .arg(&lib)
        .arg(&ar_obj)
        .status()
        .expect("failed to run `ar`");
    assert!(st.success(), "archiver failed");

    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-lib=static=dotkernel");
    println!("cargo:rerun-if-changed=src/dot_kernel.s");
    println!("cargo:rerun-if-changed=build.rs");
}
