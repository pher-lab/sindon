//! Per-user JSON config helpers.
//!
//! Maps an app name to the OS config directory (`%APPDATA%` / `~/Library/
//! Application Support` / `~/.config`) via the `dirs` crate, and provides
//! `read_json` / `write_json_atomic` for typed round-trips.
//!
//! Atomic writes go through a sibling `<file>.tmp` then `rename` — on the
//! same volume this is atomic on all three OS, so readers never see a
//! half-written file.
//!
//! Intended for non-secret settings (theme, font-size, language, sort order,
//! auto-lock interval). Do **not** put secrets here — there's no encryption
//! and the file lives at a stable, predictable path.
use std::io;
use std::path::{Path, PathBuf};

use serde::{Serialize, de::DeserializeOwned};

/// Return `<OS-config-dir>/<app_name>`, creating the directory if missing.
///
/// Returns `NotFound` if the OS doesn't expose a config dir (very rare —
/// headless containers, broken `$HOME`).
pub fn config_dir(app_name: &str) -> io::Result<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "OS config directory not available")
    })?;
    let dir = base.join(app_name);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Read and decode JSON from `path`. Returns `Ok(None)` if the file does not
/// exist (so first-launch isn't an error), `Err` on I/O or decode failure.
pub fn read_json<T: DeserializeOwned>(path: impl AsRef<Path>) -> io::Result<Option<T>> {
    let path = path.as_ref();
    match std::fs::read(path) {
        Ok(bytes) => {
            let value = serde_json::from_slice::<T>(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(Some(value))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Encode `value` as pretty JSON and write atomically to `path`.
///
/// Writes to `<path>.tmp` then `rename`s — readers either see the previous
/// content or the new content, never a partial write. Creates parent
/// directories on demand.
pub fn write_json_atomic<T: Serialize>(path: impl AsRef<Path>, value: &T) -> io::Result<()> {
    let path = path.as_ref();
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path_for(path);
    std::fs::write(&tmp, &bytes)?;
    // On Windows, fs::rename fails if the destination exists — std::fs uses
    // MoveFileExW with MOVEFILE_REPLACE_EXISTING since Rust 1.5, so this is
    // atomic + overwriting on all three OS.
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut buf = path.as_os_str().to_os_string();
    buf.push(".tmp");
    PathBuf::from(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_tempdir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("sindon_storage_test_{pid}_{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
    struct DemoSettings {
        theme: String,
        font_scale: u32,
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = unique_tempdir();
        let path = dir.join("settings.json");

        let original = DemoSettings {
            theme: "dark".to_string(),
            font_scale: 110,
        };

        write_json_atomic(&path, &original).unwrap();
        let loaded: Option<DemoSettings> = read_json(&path).unwrap();
        assert_eq!(loaded.as_ref(), Some(&original));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_missing_returns_none() {
        let dir = unique_tempdir();
        let path = dir.join("never_written.json");
        let loaded: Option<DemoSettings> = read_json(&path).unwrap();
        assert!(loaded.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_creates_parent_directories() {
        let dir = unique_tempdir();
        let path = dir.join("nested").join("deeper").join("settings.json");
        let v = DemoSettings {
            theme: "light".to_string(),
            font_scale: 90,
        };
        write_json_atomic(&path, &v).unwrap();
        assert!(path.exists());
        let loaded: DemoSettings = read_json(&path).unwrap().unwrap();
        assert_eq!(loaded, v);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_replaces_existing_file() {
        let dir = unique_tempdir();
        let path = dir.join("settings.json");

        let a = DemoSettings {
            theme: "dark".to_string(),
            font_scale: 100,
        };
        let b = DemoSettings {
            theme: "light".to_string(),
            font_scale: 120,
        };

        write_json_atomic(&path, &a).unwrap();
        write_json_atomic(&path, &b).unwrap();

        let loaded: DemoSettings = read_json(&path).unwrap().unwrap();
        assert_eq!(loaded, b);

        // Tmp sibling must be cleaned up by the successful rename.
        let tmp = tmp_path_for(&path);
        assert!(!tmp.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_invalid_json_returns_error() {
        let dir = unique_tempdir();
        let path = dir.join("bad.json");
        std::fs::write(&path, b"not valid json").unwrap();
        let err = read_json::<DemoSettings>(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn config_dir_creates_namespaced_directory() {
        // We can't predict the user's $XDG_CONFIG_HOME / %APPDATA% during the
        // test run, but we can verify the returned path exists and ends with
        // our app name.
        let app_name = format!("sindon_test_{}", COUNTER.fetch_add(1, Ordering::Relaxed));
        let dir = config_dir(&app_name).unwrap();
        assert!(dir.exists());
        assert!(dir.is_dir());
        assert_eq!(dir.file_name().unwrap(), app_name.as_str());
        std::fs::remove_dir_all(&dir).ok();
    }
}
