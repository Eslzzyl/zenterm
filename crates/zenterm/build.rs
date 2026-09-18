use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
    let icon = manifest_dir.join("../../assets/windows/zenterm.ico");

    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=build.rs");

    // CARGO_CFG_TARGET_OS describes the binary being built. This remains
    // correct when a Windows binary is cross-compiled from another host.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let icon = icon
        .to_str()
        .expect("Windows icon path must be valid UTF-8");

    winresource::WindowsResource::new()
        .set_icon(icon)
        .compile()
        .expect("failed to embed assets/windows/zenterm.ico into the Windows executable");
}
