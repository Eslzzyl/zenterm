use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
    let repository_root = manifest_dir.join("../..");
    let icon = manifest_dir.join("../../assets/windows/zenterm.ico");

    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rerun-if-changed={}",
        repository_root.join(".git/HEAD").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repository_root.join(".git/packed-refs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repository_root.join(".git/refs/heads").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repository_root.join(".git/refs/tags").display()
    );

    let package_version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION must be set");
    let commit = git_output(&repository_root, &["rev-parse", "--short=7", "HEAD"]);
    let display_version = commit
        .map(|commit| format!("{package_version}+g{commit}"))
        .unwrap_or_else(|| format!("{package_version}+dev"));
    let generated = format!("pub const DISPLAY_VERSION: &str = {display_version:?};\n");
    let output_path =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set")).join("build_info.rs");
    fs::write(output_path, generated).expect("failed to write generated build metadata");

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

fn git_output(repository_root: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_root)
        .args(arguments)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty() && !value.contains(['\r', '\n'])).then_some(value)
}
