//! High performance power plan while a game session runs (`gameMode.powerPlan`).
//!
//! Safety: the user's plan must always come back. The previous plan is written to
//! `%APPDATA%\Lucent\power.marker` *before* switching, deleted after restoring, and a
//! marker left by a killed run is restored at the next startup (same pattern as the
//! taskbar marker).
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

/// A power scheme GUID (portable stand-in for the Win32 `GUID`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid(pub u128);

/// High performance (`GUID_MIN_POWER_SAVINGS`).
pub const HIGH_PERFORMANCE: Guid = Guid(0x8c5e7fda_e8bf_4a96_9a85_a6e23a8c635c);

impl Guid {
    /// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, braces and case ignored.
    pub fn parse(s: &str) -> Option<Guid> {
        let s = s.trim().trim_start_matches('{').trim_end_matches('}');
        let hex: String = s.chars().filter(|&c| c != '-').collect();
        if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u128::from_str_radix(&hex, 16).ok().map(Guid)
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.0;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (v >> 96) as u32,
            (v >> 80) as u16,
            (v >> 64) as u16,
            (v >> 48) as u16,
            v & 0xffff_ffff_ffff
        )
    }
}

pub trait PowerApi {
    fn active(&self) -> Option<Guid>;
    fn exists(&self, g: Guid) -> bool;
    /// True on success.
    fn set_active(&mut self, g: Guid) -> bool;
}

/// `%APPDATA%\Lucent\power.marker`
pub fn marker_path() -> PathBuf {
    crate::paths::config_dir().join("power.marker")
}

/// The plan saved in the marker. `None` when missing or unreadable.
pub fn read_marker(path: &Path) -> Option<Guid> {
    Guid::parse(&fs::read_to_string(path).ok()?)
}

/// Atomic write: `power.marker.tmp` then rename over the target.
fn write_marker(path: &Path, g: Guid) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("marker.tmp");
    fs::write(&tmp, g.to_string())?;
    fs::rename(&tmp, path)
}

static MISSING_LOGGED: AtomicBool = AtomicBool::new(false);

/// Switches to High performance. Returns the plan to put back, `None` when nothing was
/// changed (already High performance, scheme missing, marker not writable, switch failed).
pub fn enter_with(api: &mut impl PowerApi, marker: &Path) -> Option<Guid> {
    if !api.exists(HIGH_PERFORMANCE) {
        // Some laptops only ship Balanced.
        if !MISSING_LOGGED.swap(true, Ordering::Relaxed) {
            eprintln!("lucent: no High performance power plan on this machine, game mode leaves the plan alone");
        }
        return None;
    }
    // A marker left by a run that never restored holds the real original plan.
    let prev = match read_marker(marker) {
        Some(g) => g,
        None => api.active()?,
    };
    if prev == HIGH_PERFORMANCE {
        return None;
    }
    if let Err(e) = write_marker(marker, prev) {
        // Without a marker a crash could leave the plan switched: refuse.
        eprintln!("lucent: cannot write power marker, plan left alone: {e}");
        return None;
    }
    if !api.set_active(HIGH_PERFORMANCE) {
        eprintln!("lucent: cannot switch to the High performance power plan");
        let _ = fs::remove_file(marker);
        return None;
    }
    Some(prev)
}

/// Puts `prev` back and deletes the marker (even if the plan no longer exists: retrying
/// at every startup would not help).
pub fn restore_with(api: &mut impl PowerApi, marker: &Path, prev: Guid) {
    if !api.set_active(prev) {
        eprintln!("lucent: cannot restore power plan {prev}");
    }
    if let Err(e) = fs::remove_file(marker) {
        if e.kind() != io::ErrorKind::NotFound {
            eprintln!("lucent: cannot delete power marker: {e}");
        }
    }
}

/// Startup: restores the plan from a leftover marker. True when one was restored.
pub fn recover_with(api: &mut impl PowerApi, marker: &Path) -> bool {
    if !marker.exists() {
        return false;
    }
    match read_marker(marker) {
        Some(prev) => {
            restore_with(api, marker, prev);
            true
        }
        None => {
            eprintln!("lucent: unreadable power marker deleted");
            let _ = fs::remove_file(marker);
            false
        }
    }
}

/// Plan to restore while High performance is applied by us.
static PREV: Mutex<Option<Guid>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<Guid>> {
    PREV.lock().unwrap_or_else(|e| e.into_inner())
}

/// Switches to High performance (real system, real marker). Returns the previous plan.
pub fn enter() -> Option<Guid> {
    let mut cur = lock();
    let prev = enter_with(&mut win::Win32Power, &marker_path());
    if prev.is_some() {
        *cur = prev;
    }
    prev
}

/// Puts `prev` back, unless an exit path already did.
pub fn restore(prev: Guid) {
    let mut cur = lock();
    if cur.take().is_some() {
        restore_with(&mut win::Win32Power, &marker_path(), prev);
    }
}

/// App exit: restores whatever `enter` switched.
pub fn restore_for_exit() {
    let mut cur = lock();
    if let Some(prev) = cur.take() {
        restore_with(&mut win::Win32Power, &marker_path(), prev);
    }
}

/// Startup: a marker means a previous run was killed mid-session.
pub fn recover_on_startup() {
    let _cur = lock();
    recover_with(&mut win::Win32Power, &marker_path());
}

#[cfg(windows)]
mod win {
    use super::{Guid, PowerApi};
    use windows::Win32::Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree};
    use windows::Win32::System::Power::{PowerGetActiveScheme, PowerReadFriendlyName, PowerSetActiveScheme};
    use windows::core::GUID;

    pub struct Win32Power;

    impl PowerApi for Win32Power {
        fn active(&self) -> Option<Guid> {
            unsafe {
                let mut p: *mut GUID = std::ptr::null_mut();
                if PowerGetActiveScheme(None, &mut p) != ERROR_SUCCESS || p.is_null() {
                    return None;
                }
                let g = (*p).to_u128();
                let _ = LocalFree(Some(HLOCAL(p as *mut _)));
                Some(Guid(g))
            }
        }

        fn exists(&self, g: Guid) -> bool {
            let guid = GUID::from_u128(g.0);
            let mut size = 0u32;
            // Size query only: fails (ERROR_FILE_NOT_FOUND) when the scheme is absent.
            unsafe { PowerReadFriendlyName(None, Some(&guid), None, None, None, &mut size) == ERROR_SUCCESS }
        }

        fn set_active(&mut self, g: Guid) -> bool {
            let guid = GUID::from_u128(g.0);
            unsafe { PowerSetActiveScheme(None, Some(&guid)) == ERROR_SUCCESS }
        }
    }
}

#[cfg(not(windows))]
mod win {
    use super::{Guid, PowerApi};

    /// No power plans: nothing is ever switched.
    pub struct Win32Power;

    impl PowerApi for Win32Power {
        fn active(&self) -> Option<Guid> {
            None
        }
        fn exists(&self, _: Guid) -> bool {
            false
        }
        fn set_active(&mut self, _: Guid) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const BALANCED: Guid = Guid(0x381b4222_f694_41f0_9685_ff5bb260df2e);

    struct Fake {
        active: Option<Guid>,
        schemes: Vec<Guid>,
        sets: Vec<Guid>,
        fail_set: bool,
    }

    impl Fake {
        fn new(active: Guid) -> Self {
            Self { active: Some(active), schemes: vec![BALANCED, HIGH_PERFORMANCE], sets: Vec::new(), fail_set: false }
        }
    }

    impl PowerApi for Fake {
        fn active(&self) -> Option<Guid> {
            self.active
        }
        fn exists(&self, g: Guid) -> bool {
            self.schemes.contains(&g)
        }
        fn set_active(&mut self, g: Guid) -> bool {
            self.sets.push(g);
            if self.fail_set {
                return false;
            }
            self.active = Some(g);
            true
        }
    }

    fn temp_marker(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lucent-power-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("power.marker")
    }

    #[test]
    fn guid_text_round_trips() {
        assert_eq!(BALANCED.to_string(), "381b4222-f694-41f0-9685-ff5bb260df2e");
        assert_eq!(Guid::parse("381b4222-f694-41f0-9685-ff5bb260df2e"), Some(BALANCED));
        assert_eq!(Guid::parse("{381B4222-F694-41F0-9685-FF5BB260DF2E}\r\n"), Some(BALANCED));
        assert_eq!(Guid::parse("+81b4222-f694-41f0-9685-ff5bb260df2e"), None);
        assert_eq!(Guid::parse("garbage"), None);
    }

    #[test]
    fn enter_writes_the_previous_plan_then_switches() {
        let m = temp_marker("enter");
        let mut api = Fake::new(BALANCED);
        assert_eq!(enter_with(&mut api, &m), Some(BALANCED));
        assert_eq!(api.active, Some(HIGH_PERFORMANCE));
        assert_eq!(read_marker(&m), Some(BALANCED));
    }

    #[test]
    fn restore_puts_the_plan_back_and_deletes_the_marker() {
        let m = temp_marker("restore");
        let mut api = Fake::new(BALANCED);
        let prev = enter_with(&mut api, &m).unwrap();
        restore_with(&mut api, &m, prev);
        assert_eq!(api.active, Some(BALANCED));
        assert!(!m.exists());
    }

    #[test]
    fn missing_high_performance_scheme_does_nothing() {
        let m = temp_marker("missing");
        let mut api = Fake::new(BALANCED);
        api.schemes = vec![BALANCED];
        assert_eq!(enter_with(&mut api, &m), None);
        assert!(api.sets.is_empty());
        assert!(!m.exists());
    }

    #[test]
    fn already_high_performance_needs_no_restore() {
        let m = temp_marker("already");
        let mut api = Fake::new(HIGH_PERFORMANCE);
        assert_eq!(enter_with(&mut api, &m), None);
        assert!(api.sets.is_empty());
        assert!(!m.exists());
    }

    #[test]
    fn failed_switch_removes_the_marker() {
        let m = temp_marker("fail");
        let mut api = Fake::new(BALANCED);
        api.fail_set = true;
        assert_eq!(enter_with(&mut api, &m), None);
        assert!(!m.exists());
    }

    #[test]
    fn a_leftover_marker_holds_the_real_original_plan() {
        // Killed mid-session: the marker says Balanced while High performance is active.
        let m = temp_marker("leftover");
        std::fs::write(&m, BALANCED.to_string()).unwrap();
        let mut api = Fake::new(HIGH_PERFORMANCE);
        assert_eq!(enter_with(&mut api, &m), Some(BALANCED));
        assert_eq!(read_marker(&m), Some(BALANCED));
    }

    #[test]
    fn startup_recovery_restores_and_deletes_the_marker() {
        let m = temp_marker("recover");
        std::fs::write(&m, BALANCED.to_string()).unwrap();
        let mut api = Fake::new(HIGH_PERFORMANCE);
        assert!(recover_with(&mut api, &m));
        assert_eq!(api.active, Some(BALANCED));
        assert!(!m.exists());
    }

    #[test]
    fn startup_recovery_without_marker_touches_nothing() {
        let m = temp_marker("none");
        let mut api = Fake::new(HIGH_PERFORMANCE);
        assert!(!recover_with(&mut api, &m));
        assert!(api.sets.is_empty());
    }

    #[test]
    fn startup_recovery_drops_an_unreadable_marker() {
        let m = temp_marker("garbage");
        std::fs::write(&m, "not a guid").unwrap();
        let mut api = Fake::new(HIGH_PERFORMANCE);
        assert!(!recover_with(&mut api, &m));
        assert!(api.sets.is_empty());
        assert!(!m.exists());
    }
}
