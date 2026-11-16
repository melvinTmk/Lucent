//! Per-user storage locations. Never hardcoded: resolved from the OS.
use std::fs;
use std::path::PathBuf;

const APP_DIR: &str = "Lucent";
/// The app's former name: its settings are carried over once (`migrate_legacy`).
const LEGACY_DIR: &str = "Glassdock";

/// Files of the former `%APPDATA%\Glassdock` worth keeping: settings, search history,
/// project list and the taskbar marker (so a taskbar hidden by the old version still
/// comes back).
const LEGACY_FILES: [&str; 4] = ["settings.json", "search-history.json", "projects.json", "taskbar-state.json"];

/// Copies the legacy config files into `new` when `new` has no settings yet. Returns the
/// names copied. Never removes the old folder (the old install may still be there).
pub fn migrate_legacy(old: &std::path::Path, new: &std::path::Path) -> Vec<&'static str> {
    if new.join("settings.json").exists() || !old.is_dir() {
        return Vec::new();
    }
    let _ = fs::create_dir_all(new);
    LEGACY_FILES
        .into_iter()
        .filter(|f| old.join(f).is_file() && !new.join(f).exists() && fs::copy(old.join(f), new.join(f)).is_ok())
        .collect()
}

fn ensure(dir: PathBuf) -> PathBuf {
    let _ = fs::create_dir_all(&dir);
    dir
}

/// `%APPDATA%\Lucent` (settings, taskbar marker). Created if missing; filled from the
/// former `%APPDATA%\Glassdock` the first time.
pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join(APP_DIR);
    static MIGRATED: std::sync::Once = std::sync::Once::new();
    MIGRATED.call_once(|| {
        let copied = migrate_legacy(&base.join(LEGACY_DIR), &dir);
        if !copied.is_empty() {
            eprintln!("lucent: settings carried over from Glassdock: {copied:?}");
        }
    });
    ensure(dir)
}

/// `%LOCALAPPDATA%\Lucent` (icon cache). Created if missing.
pub fn cache_dir() -> PathBuf {
    ensure(
        dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(APP_DIR),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lucent-paths-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn legacy_settings_copied_once() {
        let (old, new) = (tmp("old"), tmp("new"));
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("settings.json"), "{}").unwrap();
        fs::write(old.join("taskbar-state.json"), "{\"hidden\":true}").unwrap();
        fs::write(old.join("other.txt"), "x").unwrap();
        assert_eq!(migrate_legacy(&old, &new), vec!["settings.json", "taskbar-state.json"]);
        assert!(new.join("taskbar-state.json").is_file());
        assert!(!new.join("other.txt").exists());
        // Already migrated (settings present): nothing more, the old folder stays.
        assert!(migrate_legacy(&old, &new).is_empty());
        assert!(old.join("settings.json").is_file());
        // No old folder: nothing.
        assert!(migrate_legacy(&tmp("none"), &tmp("new2")).is_empty());
        let _ = fs::remove_dir_all(&old);
        let _ = fs::remove_dir_all(&new);
    }
}
