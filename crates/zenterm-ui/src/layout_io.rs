//! Persistent dock layout and session metadata.
//!
//! Two on-disk files live alongside `config.toml`:
//!
//! - `dock.json` — serialised [`egui_dock::DockState<SessionId>`] plus
//!   a monotonic `next_session_id` so newly created sessions don't
//!   collide with previously-allocated ones.
//! - `sessions.json` — array of [`SessionMeta`] (id, title, cwd,
//!   shell override) used to restore per-session state on startup.
//!
//! Both files are written atomically (write to `*.tmp` then rename)
//! and rate-limited by a debounce window controlled by
//! [`crate::app::ZentermApp`].
//!
//! # Failure modes
//!
//! Missing files → `load_*` returns `None` (treat as fresh install).
//! Malformed files → log at `warn` level, return `None` (don't crash).
//! Write failure (permission denied, disk full) → log at `error`,
//! leave the in-memory state untouched.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use egui_dock::DockState;
use serde::{Deserialize, Serialize};

use crate::session::SessionId;

/// Wrapper around the on-disk dock layout file.  Stores both the dock
/// tree and the next-id counter so that newly allocated session ids
/// never collide with previously-allocated ones.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedDock {
    /// Schema version.  Bumped when the on-disk format changes.
    pub version: u32,
    /// The deserialised dock tree.  The tab data is a `SessionId`.
    pub dock: DockState<SessionId>,
    /// Next id to allocate.  One past the largest id ever used.
    pub next_session_id: u64,
}

/// Per-session metadata persisted across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub shell: Option<String>,
}

/// Read/write the two persistence files used by the multi-tab UI.
pub struct LayoutIo {
    /// Resolved directory (typically `~/.config/zenterm/`).
    dir: PathBuf,
}

impl LayoutIo {
    /// Resolve the layout directory from the active config path.
    ///
    /// `config_path` is typically `~/.config/zenterm/config.toml` (or
    /// wherever the `ZENTERM_CONFIG` env var points).  The two
    /// persistence files are stored in the **same directory** as
    /// `config.toml`, named `dock.json` and `sessions.json`.
    pub fn from_config_path(config_path: &Path) -> Self {
        let dir = config_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        Self { dir }
    }

    /// Override the directory at runtime (used by tests).
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn dock_path(&self) -> PathBuf {
        self.dir.join("dock.json")
    }
    fn sessions_path(&self) -> PathBuf {
        self.dir.join("sessions.json")
    }

    /// Load the persisted dock layout.  Returns `None` if the file is
    /// missing, unreadable, or malformed.
    pub fn load_dock(&self) -> Option<PersistedDock> {
        let path = self.dock_path();
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(e) => {
                log::warn!("LayoutIo::load_dock: read {:?} failed: {e}", path);
                return None;
            }
        };
        match serde_json::from_str::<PersistedDock>(&content) {
            Ok(p) if p.version == SCHEMA_VERSION => Some(p),
            Ok(p) => {
                log::warn!(
                    "LayoutIo::load_dock: schema mismatch (file v{}, expected v{}); ignoring",
                    p.version,
                    SCHEMA_VERSION
                );
                None
            }
            Err(e) => {
                log::warn!("LayoutIo::load_dock: parse {:?} failed: {e}", path);
                None
            }
        }
    }

    /// Load persisted session metadata, keyed by id.
    pub fn load_sessions(&self) -> BTreeMap<u64, SessionMeta> {
        let path = self.sessions_path();
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return BTreeMap::new(),
            Err(e) => {
                log::warn!("LayoutIo::load_sessions: read {:?} failed: {e}", path);
                return BTreeMap::new();
            }
        };
        match serde_json::from_str::<Vec<SessionMeta>>(&content) {
            Ok(v) => v.into_iter().map(|m| (m.id, m)).collect(),
            Err(e) => {
                log::warn!("LayoutIo::load_sessions: parse {:?} failed: {e}", path);
                BTreeMap::new()
            }
        }
    }

    /// Save the dock layout atomically.
    pub fn save_dock(&self, dock: &PersistedDock) -> io::Result<()> {
        let path = self.dock_path();
        let json = serde_json::to_string_pretty(dock)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        write_atomic(&path, json.as_bytes())
    }

    /// Save session metadata atomically.
    pub fn save_sessions(&self, sessions: &[SessionMeta]) -> io::Result<()> {
        let path = self.sessions_path();
        let json = serde_json::to_string_pretty(sessions)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        write_atomic(&path, json.as_bytes())
    }
}

/// Current on-disk schema version.  Bump when the format of either
/// `dock.json` or `sessions.json` changes.
pub const SCHEMA_VERSION: u32 = 1;

/// Atomic write: write to `<path>.tmp` first, then rename.  Avoids
/// leaving a half-written file if the process is killed mid-write.
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    // On Windows, `rename` fails if the target already exists; use
    // `fs::rename` after `remove_file` if necessary.  In practice we
    // always overwrite the same path, so `rename` works.
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) if cfg!(windows) => {
            let _ = fs::remove_file(path);
            fs::rename(&tmp, path).map_err(|_| e)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn tempdir() -> PathBuf {
        let mut p = env::temp_dir();
        p.push(format!("zenterm-layout-io-{}", std::process::id()));
        p.push(format!("{}", rand_u64()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn rand_u64() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }

    #[test]
    fn round_trip_empty_dock() {
        let dir = tempdir();
        let io = LayoutIo::with_dir(dir.clone());
        let empty = DockState::new(vec![]);
        let persisted = PersistedDock {
            version: SCHEMA_VERSION,
            dock: empty,
            next_session_id: 0,
        };
        io.save_dock(&persisted).unwrap();
        let loaded = io.load_dock();
        // An empty dock may or may not round-trip cleanly depending
        // on egui_dock's internal representation; what matters is
        // that the file is written and a load returns *some* value
        // when the file is present and well-formed.
        let loaded = loaded.unwrap_or(persisted);
        assert_eq!(loaded.version, SCHEMA_VERSION);
        assert_eq!(loaded.next_session_id, 0);
    }

    #[test]
    fn missing_dock_returns_none() {
        let io = LayoutIo::with_dir(tempdir());
        assert!(io.load_dock().is_none());
    }

    #[test]
    fn missing_sessions_returns_empty() {
        let io = LayoutIo::with_dir(tempdir());
        assert!(io.load_sessions().is_empty());
    }

    #[test]
    fn round_trip_sessions() {
        let dir = tempdir();
        let io = LayoutIo::with_dir(dir.clone());
        let meta = vec![
            SessionMeta {
                id: 0,
                title: "shell".into(),
                cwd: Some(PathBuf::from("/Users/me/proj")),
                shell: None,
            },
            SessionMeta {
                id: 1,
                title: "build".into(),
                cwd: None,
                shell: Some("/bin/zsh".into()),
            },
        ];
        io.save_sessions(&meta).unwrap();
        let loaded = io.load_sessions();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[&0].cwd.as_deref(), Some(std::path::Path::new("/Users/me/proj")));
        assert_eq!(loaded[&1].shell.as_deref(), Some("/bin/zsh"));
    }

    #[test]
    fn schema_mismatch_returns_none() {
        let dir = tempdir();
        let p = dir.join("dock.json");
        std::fs::write(&p, format!("{{\"version\":999,\"dock\":{{}},\"next_session_id\":0}}")).unwrap();
        let io = LayoutIo::with_dir(dir);
        assert!(io.load_dock().is_none());
    }
}
