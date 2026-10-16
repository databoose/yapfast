//! Gives the Windows executable its icon: compiles packaging/windows/yapfast.rc
//! with LLVM's resource compiler and links the result.

use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=packaging/windows");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let resources = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("yapfast.res");
    // The script names only the icon beside it, so it needs no preprocessor.
    let status = Command::new("llvm-rc")
        .args(["-no-preprocess", "-FO"])
        .arg(&resources)
        .arg("packaging/windows/yapfast.rc")
        .status()
        .unwrap_or_else(|error| panic!("could not run llvm-rc: {error}"));
    assert!(status.success(), "llvm-rc could not compile yapfast.rc");
    // The MSVC linker takes compiled resources as an input.
    println!("cargo:rustc-link-arg-bins={}", resources.display());
}
