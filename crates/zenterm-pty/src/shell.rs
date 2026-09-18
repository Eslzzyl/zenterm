//! Shell discovery and fixed-shell resolution.
//!
//! Discovery is deliberately filesystem-only.  It never executes a candidate
//! shell; the PTY spawn remains the final availability check.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;

/// Where a shell candidate was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellSource {
    /// The platform default returned by `portable-pty`.
    PlatformDefault,
    /// An environment variable such as `SHELL` or `ComSpec`.
    Environment,
    /// The path persisted in the user's configuration.
    Configured,
    /// An entry in `/etc/shells` on Unix platforms.
    EtcShells,
    /// A well-known executable found in `PATH`.
    Path,
    /// A well-known installation location.
    KnownLocation,
}

impl ShellSource {
    /// Human-readable source label for the settings UI.
    pub const fn label(self) -> &'static str {
        match self {
            Self::PlatformDefault => "Platform default",
            Self::Environment => "Environment",
            Self::Configured => "Configured",
            Self::EtcShells => "/etc/shells",
            Self::Path => "PATH",
            Self::KnownLocation => "Known location",
        }
    }
}

/// A discovered executable that can be selected as the fixed shell for new
/// terminal sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCandidate {
    /// Canonical executable path when the filesystem allows it.
    pub program: PathBuf,
    /// Stable, user-facing name derived from the executable and its path.
    pub display_name: String,
    /// Discovery source shown in the settings panel.
    pub source: ShellSource,
    /// Whether this is the platform's current default shell.
    pub is_platform_default: bool,
}

/// Return the platform default shell using the same resolver as
/// `portable-pty::CommandBuilder::new_default_prog()`.
pub fn default_shell() -> Option<PathBuf> {
    let command = CommandBuilder::new_default_prog();
    let path = PathBuf::from(command.get_shell());
    is_available(&path).then(|| canonical_path(&path))
}

/// Discover available shells from environment hints, platform metadata,
/// `PATH`, and bounded well-known locations.
pub fn detect_shells() -> Vec<ShellCandidate> {
    let mut collector = Collector::default();

    if let Some(path) = default_shell() {
        collector.push(path, ShellSource::PlatformDefault, true);
    }

    for name in environment_shell_names() {
        if let Some(path) = std::env::var_os(name).map(PathBuf::from) {
            collector.push(path, ShellSource::Environment, false);
        }
    }

    #[cfg(unix)]
    add_etc_shells(&mut collector);

    add_path_shells(&mut collector);
    add_known_locations(&mut collector);

    collector.candidates
}

/// Describe one explicitly configured shell path when it is currently
/// available.  This keeps custom configured shells visible even when their
/// basename is not in the built-in discovery list.
pub fn candidate_for_path(path: &Path, source: ShellSource) -> Option<ShellCandidate> {
    is_available(path).then(|| {
        let program = canonical_path(path);
        ShellCandidate {
            display_name: display_name(&program),
            program,
            source,
            is_platform_default: false,
        }
    })
}

#[derive(Default)]
struct Collector {
    candidates: Vec<ShellCandidate>,
    keys: HashMap<String, usize>,
}

impl Collector {
    fn push(&mut self, path: PathBuf, source: ShellSource, is_platform_default: bool) {
        if !is_available(&path) {
            return;
        }

        let program = canonical_path(&path);
        let key = path_key(&program);
        if let Some(index) = self.keys.get(&key).copied() {
            let candidate = &mut self.candidates[index];
            if is_platform_default {
                candidate.is_platform_default = true;
                candidate.source = ShellSource::PlatformDefault;
            }
            return;
        }

        let index = self.candidates.len();
        self.keys.insert(key, index);
        self.candidates.push(ShellCandidate {
            display_name: display_name(&program),
            program,
            source,
            is_platform_default,
        });
    }
}

fn environment_shell_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["SHELL", "ComSpec"]
    }
    #[cfg(not(windows))]
    {
        &["SHELL"]
    }
}

fn add_path_shells(collector: &mut Collector) {
    let Some(path) = std::env::var_os("PATH") else {
        return;
    };

    for directory in std::env::split_paths(&path) {
        if directory.as_os_str().is_empty() || is_network_path(&directory) {
            continue;
        }
        for name in path_shell_names() {
            collector.push(directory.join(name), ShellSource::Path, false);
        }
    }
}

#[cfg(unix)]
fn add_etc_shells(collector: &mut Collector) {
    let Ok(content) = fs::read_to_string("/etc/shells") else {
        return;
    };

    for line in content.lines() {
        let entry = line.split('#').next().unwrap_or_default().trim();
        if !entry.is_empty() {
            collector.push(PathBuf::from(entry), ShellSource::EtcShells, false);
        }
    }
}

#[cfg(windows)]
fn add_known_locations(collector: &mut Collector) {
    let mut bases = Vec::new();
    for name in [
        "WINDIR",
        "ProgramFiles",
        "ProgramW6432",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
    ] {
        if let Some(path) = std::env::var_os(name).map(PathBuf::from) {
            bases.push(path);
        }
    }

    if let Some(windir) = std::env::var_os("WINDIR").map(PathBuf::from) {
        collector.push(
            windir.join("System32/cmd.exe"),
            ShellSource::KnownLocation,
            false,
        );
        collector.push(
            windir.join("System32/WindowsPowerShell/v1.0/powershell.exe"),
            ShellSource::KnownLocation,
            false,
        );
        collector.push(
            windir.join("System32/wsl.exe"),
            ShellSource::KnownLocation,
            false,
        );
    }

    for base in &bases {
        collector.push(
            base.join("Git/bin/bash.exe"),
            ShellSource::KnownLocation,
            false,
        );
        collector.push(
            base.join("Git/usr/bin/bash.exe"),
            ShellSource::KnownLocation,
            false,
        );
        collector.push(
            base.join("nushell/nu.exe"),
            ShellSource::KnownLocation,
            false,
        );
        collector.push(
            base.join("nu/bin/nu.exe"),
            ShellSource::KnownLocation,
            false,
        );

        let powershell_root = base.join("PowerShell");
        if let Ok(entries) = fs::read_dir(powershell_root) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    collector.push(
                        entry.path().join("pwsh.exe"),
                        ShellSource::KnownLocation,
                        false,
                    );
                }
            }
        }
    }

    for path in [
        PathBuf::from(r"C:\msys64\usr\bin\bash.exe"),
        PathBuf::from(r"C:\msys32\usr\bin\bash.exe"),
        PathBuf::from(r"C:\cygwin64\bin\bash.exe"),
        PathBuf::from(r"C:\cygwin\bin\bash.exe"),
    ] {
        collector.push(path, ShellSource::KnownLocation, false);
    }
}

#[cfg(not(windows))]
fn add_known_locations(collector: &mut Collector) {
    for directory in [
        "/bin",
        "/usr/bin",
        "/usr/local/bin",
        "/opt/homebrew/bin",
        "/opt/local/bin",
    ] {
        for name in path_shell_names() {
            collector.push(
                Path::new(directory).join(name),
                ShellSource::KnownLocation,
                false,
            );
        }
    }
}

#[cfg(windows)]
fn path_shell_names() -> &'static [&'static str] {
    &[
        "cmd.exe",
        "powershell.exe",
        "pwsh.exe",
        "bash.exe",
        "zsh.exe",
        "fish.exe",
        "nu.exe",
        "wsl.exe",
    ]
}

#[cfg(not(windows))]
fn path_shell_names() -> &'static [&'static str] {
    &[
        "sh", "bash", "dash", "zsh", "fish", "ksh", "mksh", "tcsh", "csh", "nu", "elvish", "xonsh",
        "yash", "ion",
    ]
}

fn is_available(path: &Path) -> bool {
    if is_non_interactive_name(path) {
        return false;
    }
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(windows)]
    {
        matches!(
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.to_ascii_lowercase())
                .as_deref(),
            Some("exe" | "com")
        )
    }
}

fn is_non_interactive_name(path: &Path) -> bool {
    matches!(
        path.file_stem()
            .and_then(|name| name.to_str())
            .map(|name| name.to_ascii_lowercase())
            .as_deref(),
        Some("false" | "true" | "nologin" | "sync" | "halt" | "reboot" | "shutdown")
    )
}

fn canonical_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn path_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    {
        value.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        value
    }
}

fn display_name(path: &Path) -> String {
    let filename = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Shell")
        .to_ascii_lowercase();
    let full_path = path
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('\\', "/");

    match filename.as_str() {
        "cmd" => "Command Prompt".into(),
        "powershell" => "Windows PowerShell".into(),
        "pwsh" => "PowerShell 7+".into(),
        "bash" if full_path.contains("/git/") => "Git Bash".into(),
        "bash" if full_path.contains("msys") => "MSYS2 Bash".into(),
        "bash" if full_path.contains("cygwin") => "Cygwin Bash".into(),
        "bash" => "Bash".into(),
        "zsh" => "Zsh".into(),
        "fish" => "Fish".into(),
        "nu" => "Nushell".into(),
        "wsl" => "WSL".into(),
        other => other.to_owned(),
    }
}

fn is_network_path(path: &Path) -> bool {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        value.starts_with(r"\\")
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{display_name, path_key};
    use std::path::Path;

    #[test]
    fn display_names_classify_common_shells() {
        assert_eq!(display_name(Path::new("/bin/bash")), "Bash");
        assert_eq!(display_name(Path::new("/usr/bin/zsh")), "Zsh");
        assert_eq!(display_name(Path::new("/usr/bin/nu")), "Nushell");
    }

    #[test]
    fn path_keys_normalize_separators() {
        assert_eq!(path_key(Path::new("/tmp/shell")), "/tmp/shell");
    }
}
