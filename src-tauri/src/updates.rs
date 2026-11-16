//! Self-update from the GitHub releases.
//!
//! The release workflow publishes `latest.json` next to the installer; both are signed
//! with the project's updater key (public half in `tauri.conf.json`), and an installer
//! whose signature does not match is refused. Lucent checks shortly after startup, then
//! every few hours, while `general.checkUpdates` is on; « Rechercher maintenant » in
//! Réglages checks at once. A new version shows a card in the dock ("Mettre à jour" /
//! "Plus tard"); installing downloads the installer, starts it in passive mode with
//! `/UPDATE /R` (it closes Lucent, installs over it and starts the new version) and exits
//! through `exit_now`, which puts the taskbar back first.
//!
//! The updater plugin's own `install` ends with `std::process::exit`, which unloads the
//! WebViews and the composition stack while they are still alive (where the CoreMessaging
//! fail-fasts were seen): only its `check` and signature-checked `download` are used.
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// First automatic check, after startup settled.
const FIRST_CHECK: Duration = Duration::from_secs(90);
/// Then this often.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Installing,
    Failed,
}

/// `update://state`, and the result of the commands.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub phase: Phase,
    /// Running version.
    pub current: String,
    /// The newer version found (kept while downloading / after a failed install).
    pub version: Option<String>,
    /// Download progress, 0-100 (None: size unknown or not downloading).
    pub progress: Option<u8>,
    pub error: Option<String>,
    /// « Plus tard » on the dock card: the card stays away until the next version.
    pub dismissed: bool,
}

impl UpdateStatus {
    fn new() -> Self {
        Self {
            phase: Phase::Idle,
            current: env!("CARGO_PKG_VERSION").into(),
            version: None,
            progress: None,
            error: None,
            dismissed: false,
        }
    }
}

struct State {
    status: UpdateStatus,
    pending: Option<tauri_plugin_updater::Update>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let st = g.get_or_insert_with(|| State { status: UpdateStatus::new(), pending: None });
    f(st)
}

pub fn status() -> UpdateStatus {
    with(|s| s.status.clone())
}

fn set(app: &AppHandle, f: impl FnOnce(&mut UpdateStatus)) -> UpdateStatus {
    let st = with(|s| {
        f(&mut s.status);
        s.status.clone()
    });
    let _ = app.emit("update://state", st.clone());
    st
}

/// Busy phases: a check or an install already runs.
pub fn busy(p: Phase) -> bool {
    matches!(p, Phase::Checking | Phase::Downloading | Phase::Installing)
}

/// Percent for a download, None while the total size is unknown.
pub fn percent(done: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|&t| t > 0)?;
    Some(((done.min(total) * 100) / total) as u8)
}

/// `true` when `found` is a version the user has not already been told about (a « Plus
/// tard » holds for that version only).
pub fn is_new_announcement(previous: Option<&str>, found: &str) -> bool {
    previous != Some(found)
}

/// Checks GitHub (blocking: call from a worker thread or the blocking pool).
pub fn check_blocking(app: &AppHandle) -> UpdateStatus {
    use tauri_plugin_updater::UpdaterExt;
    let start = with(|s| {
        if busy(s.status.phase) {
            return None;
        }
        Some(())
    });
    if start.is_none() {
        return status();
    }
    set(app, |s| {
        s.phase = Phase::Checking;
        s.error = None;
    });
    let res = tauri::async_runtime::block_on(async {
        let updater = app.updater().map_err(|e| e.to_string())?;
        updater.check().await.map_err(|e| e.to_string())
    });
    match res {
        Ok(Some(update)) => {
            let version = update.version.clone();
            let previous = with(|s| s.status.version.clone());
            let fresh = is_new_announcement(previous.as_deref(), &version);
            with(|s| s.pending = Some(update));
            diag_event(format!("update: {version} available"));
            // New version: the docks peek once so the card is seen (not during a game).
            #[cfg(windows)]
            if fresh && !crate::game::service::is_active() {
                for m in crate::monitors::win::enumerate() {
                    crate::dock_host::peek(m.id, 6000);
                }
            }
            set(app, |s| {
                s.phase = Phase::Available;
                s.version = Some(version);
                s.progress = None;
                if fresh {
                    s.dismissed = false;
                }
            })
        }
        Ok(None) => {
            with(|s| s.pending = None);
            set(app, |s| {
                s.phase = Phase::UpToDate;
                s.version = None;
            })
        }
        Err(e) => {
            diag_event(format!("update: check failed: {e}"));
            set(app, |s| {
                // A failed check keeps an update already found.
                s.phase = if s.version.is_some() { Phase::Available } else { Phase::Failed };
                s.error = Some(e);
            })
        }
    }
}

fn diag_event(msg: String) {
    crate::diag::event(msg);
}

/// Background loop: first check after `FIRST_CHECK`, then every `CHECK_EVERY`, while the
/// setting is on. Debug builds never check (they are not installed).
pub fn start(app: &AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new().name("lucent-updates".into()).spawn(move || {
        std::thread::sleep(FIRST_CHECK);
        loop {
            let on = app.state::<crate::settings::SettingsStore>().get().general.check_updates;
            if on {
                check_blocking(&app);
            }
            std::thread::sleep(CHECK_EVERY);
        }
    });
    if let Err(e) = spawned {
        eprintln!("lucent: cannot spawn update thread: {e}");
    }
}

/// Downloads the pending update (signature checked by the plugin), starts its installer
/// and exits. Returns only on failure (the status then says why).
fn install_blocking(app: &AppHandle) {
    let Some(update) = with(|s| s.pending.clone()) else {
        set(app, |s| {
            s.phase = Phase::Failed;
            s.error = Some("Aucune mise à jour à installer".into());
        });
        return;
    };
    set(app, |s| {
        s.phase = Phase::Downloading;
        s.progress = Some(0);
        s.error = None;
        s.dismissed = false;
    });
    let mut done: u64 = 0;
    let mut last_pct: Option<u8> = Some(0);
    let progress_app = app.clone();
    let bytes = tauri::async_runtime::block_on(update.download(
        |chunk, total| {
            done += chunk as u64;
            let pct = percent(done, total);
            if pct != last_pct {
                last_pct = pct;
                set(&progress_app, |s| s.progress = pct);
            }
        },
        || {},
    ));
    let result = bytes.map_err(|e| e.to_string()).and_then(|b| run_installer(app, &update.version, &b));
    if let Err(e) = result {
        diag_event(format!("update: install failed: {e}"));
        set(app, |s| {
            s.phase = Phase::Failed;
            s.progress = None;
            s.error = Some(e);
        });
    }
}

/// An NSIS installer starts with `MZ`; anything else (an old zipped artifact) is refused.
pub fn looks_like_installer(bytes: &[u8]) -> bool {
    bytes.len() > 1024 && bytes.starts_with(b"MZ")
}

fn run_installer(app: &AppHandle, version: &str, bytes: &[u8]) -> Result<(), String> {
    if !looks_like_installer(bytes) {
        return Err("Le fichier téléchargé n'est pas un installeur".into());
    }
    let dir = std::env::temp_dir().join("lucent-update");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("Lucent_{version}_x64-setup.exe"));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    set(app, |s| {
        s.phase = Phase::Installing;
        s.progress = None;
    });
    // `/P` passive (progress bar only), `/UPDATE` keeps the autostart entry, `/R` starts
    // Lucent again when done.
    std::process::Command::new(&path)
        .args(["/P", "/UPDATE", "/R"])
        .spawn()
        .map_err(|e| format!("Impossible de lancer l'installeur : {e}"))?;
    diag_event(format!("update: installer for {version} started, exiting"));
    crate::exit_now(0)
}

// ---- Commands ----------------------------------------------------------------------

#[tauri::command]
pub fn update_status() -> UpdateStatus {
    status()
}

/// « Rechercher maintenant » (Réglages).
#[tauri::command]
pub async fn update_check(app: AppHandle) -> UpdateStatus {
    tauri::async_runtime::spawn_blocking(move || check_blocking(&app)).await.unwrap_or_else(|_| status())
}

/// « Mettre à jour » (dock card or Réglages). Progress and failure come as `update://state`.
#[tauri::command]
pub fn update_install(app: AppHandle) {
    if busy(status().phase) {
        return;
    }
    let spawned = std::thread::Builder::new().name("lucent-update-install".into()).spawn(move || install_blocking(&app));
    if let Err(e) = spawned {
        eprintln!("lucent: cannot spawn update install thread: {e}");
    }
}

/// « Plus tard » on the dock card.
#[tauri::command]
pub fn update_dismiss(app: AppHandle) {
    set(&app, |s| {
        s.dismissed = true;
        if s.phase == Phase::Failed && s.version.is_some() {
            s.phase = Phase::Available;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_percent() {
        assert_eq!(percent(0, Some(200)), Some(0));
        assert_eq!(percent(100, Some(200)), Some(50));
        assert_eq!(percent(250, Some(200)), Some(100));
        assert_eq!(percent(10, None), None);
        assert_eq!(percent(10, Some(0)), None);
    }

    #[test]
    fn busy_phases() {
        assert!(busy(Phase::Checking));
        assert!(busy(Phase::Downloading));
        assert!(busy(Phase::Installing));
        assert!(!busy(Phase::Available));
        assert!(!busy(Phase::Failed));
        assert!(!busy(Phase::Idle));
    }

    #[test]
    fn later_holds_for_one_version() {
        assert!(is_new_announcement(None, "0.3.0"));
        assert!(!is_new_announcement(Some("0.3.0"), "0.3.0"));
        assert!(is_new_announcement(Some("0.3.0"), "0.3.1"));
    }

    #[test]
    fn installer_check() {
        let mut exe = vec![0u8; 4096];
        exe[0] = b'M';
        exe[1] = b'Z';
        assert!(looks_like_installer(&exe));
        assert!(!looks_like_installer(b"PK\x03\x04 zipped"));
        assert!(!looks_like_installer(b"MZ"));
    }

    #[test]
    fn status_serializes_camel_case() {
        let mut s = UpdateStatus::new();
        s.phase = Phase::UpToDate;
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["phase"], "upToDate");
        assert_eq!(v["current"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["dismissed"], false);
    }
}
