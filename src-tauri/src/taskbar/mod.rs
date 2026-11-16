//! Windows taskbar hide/restore. Safety-critical: the taskbar must never be left hidden.
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub mod watchdog;
#[cfg(windows)]
pub mod win;

pub trait ShellTaskbar {
    /// The shell's taskbar exists (explorer running). Nothing can be applied otherwise.
    fn available(&self) -> bool;
    fn autohide(&self) -> bool;
    fn set_autohide(&mut self, on: bool);
    fn set_visible(&mut self, visible: bool);
    /// Some taskbar window (primary or secondary) is currently shown.
    fn any_visible(&self) -> bool;
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker {
    pub hidden: bool,
    pub original_autohide: bool,
}

/// Reads the marker. `None` when missing or unreadable.
pub fn read_marker(path: &Path) -> Option<Marker> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Atomic write: `<name>.tmp` then rename over the target.
fn write_marker(path: &Path, m: &Marker) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string(m).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)
}

/// Explorer's `Shell_TrayWnd` among every top-level window of that class, given in z-order
/// as `(hwnd, owning pid)`. Lucent's tray host registers a window of the same class
/// (and in front of Explorer's) while the taskbar is hidden, and so would another Lucent
/// instance: only the shell's own window qualifies. `shell_pid` owns `GetShellWindow()`;
/// when it is unknown or owns no tray, fall back to a process named `explorer.exe`.
/// Never our own window.
pub fn pick_explorer_tray(
    candidates: &[(isize, u32)],
    shell_pid: Option<u32>,
    own_pid: u32,
    is_explorer_pid: impl Fn(u32) -> bool,
) -> Option<isize> {
    let others = || candidates.iter().filter(|(_, pid)| *pid != own_pid);
    shell_pid
        .and_then(|sp| others().find(|(_, pid)| *pid == sp))
        .or_else(|| others().find(|(_, pid)| is_explorer_pid(*pid)))
        .map(|(h, _)| *h)
}

/// Whether the taskbar should be hidden: the setting is on *and* at least one dock window
/// is up (its page reported a layout). Each setter returns the new target only when it
/// changed, so callers act on transitions (not on every settings save).
#[derive(Debug, Clone, Copy)]
pub struct HideGate {
    want: bool,
    docks_ready: bool,
}

impl HideGate {
    pub fn new(want: bool) -> Self {
        Self { want, docks_ready: false }
    }

    pub fn target(&self) -> bool {
        self.want && self.docks_ready
    }

    pub fn set_want(&mut self, want: bool) -> Option<bool> {
        self.change(|g| g.want = want)
    }

    pub fn set_docks_ready(&mut self, ready: bool) -> Option<bool> {
        self.change(|g| g.docks_ready = ready)
    }

    fn change(&mut self, f: impl FnOnce(&mut Self)) -> Option<bool> {
        let before = self.target();
        f(self);
        let after = self.target();
        (before != after).then_some(after)
    }
}

/// `%APPDATA%\Lucent\taskbar-state.json`
pub fn marker_path() -> PathBuf {
    crate::paths::config_dir().join("taskbar-state.json")
}

/// Hides the taskbar while remembering the user's original state (in memory and in the
/// marker file, written *before* hiding) so it can always be put back.
///
/// When the shell is unavailable (explorer down) nothing is applied: a restore is kept
/// pending, with the marker left on disk, until `on_explorer_restarted` or next startup.
pub struct TaskbarGuard<T: ShellTaskbar> {
    shell: T,
    marker_path: PathBuf,
    /// `Some` while we keep the taskbar hidden.
    state: Option<Marker>,
    /// Original autohide still to put back once the shell is available again.
    pending_restore: Option<bool>,
}

impl<T: ShellTaskbar> TaskbarGuard<T> {
    pub fn new(shell: T, marker_path: PathBuf) -> Self {
        Self { shell, marker_path, state: None, pending_restore: None }
    }

    /// No-op when already hidden (no shell calls): re-hiding after an Explorer restart
    /// goes through `on_explorer_restarted`.
    pub fn hide(&mut self) {
        if self.state.is_some() {
            return;
        }
        // Explorer down: the original autohide cannot be read reliably; do nothing.
        if !self.shell.available() {
            return;
        }
        // A marker left on disk (previous run, or a pending restore) holds the
        // real original state.
        let original_autohide = match read_marker(&self.marker_path) {
            Some(m) if m.hidden => m.original_autohide,
            _ => self.pending_restore.unwrap_or_else(|| self.shell.autohide()),
        };
        let m = Marker { hidden: true, original_autohide };
        if let Err(e) = write_marker(&self.marker_path, &m) {
            // Without a marker neither the watchdog nor startup recovery could
            // restore after a kill: refuse to hide.
            eprintln!("lucent: cannot write taskbar marker, not hiding: {e}");
            return;
        }
        self.state = Some(m);
        self.pending_restore = None;
        self.shell.set_autohide(true);
        self.shell.set_visible(false);
    }

    pub fn restore(&mut self) {
        if let Some(m) = self.state.take() {
            self.put_back(m.original_autohide);
        }
    }

    pub fn recover_on_startup(&mut self) {
        if !self.marker_path.exists() {
            return;
        }
        self.state = None;
        match read_marker(&self.marker_path) {
            Some(m) if !m.hidden => {
                let _ = fs::remove_file(&self.marker_path);
            }
            Some(m) => self.put_back(m.original_autohide),
            // Unreadable marker: original unknown, fall back to the Windows default
            // (visible, not auto-hidden). Never leave the taskbar hidden.
            None => self.put_back(false),
        }
    }

    /// Explorer (re)created its taskbar: finish a pending restore, or re-hide.
    pub fn on_explorer_restarted(&mut self) {
        if let Some(original) = self.pending_restore {
            self.put_back(original);
        } else if self.state.is_some() && self.shell.available() {
            self.shell.set_autohide(true);
            self.shell.set_visible(false);
        }
    }

    /// While hidden: Windows 11 shows its taskbars again on its own (Explorer start, Start
    /// menu, monitor changes...). Re-hides them, and re-asserts autohide. True when it acted.
    pub fn enforce(&mut self) -> bool {
        if self.state.is_none() || !self.shell.available() {
            return false;
        }
        let mut acted = false;
        if !self.shell.autohide() {
            self.shell.set_autohide(true);
            acted = true;
        }
        if self.shell.any_visible() {
            self.shell.set_visible(false);
            acted = true;
        }
        acted
    }

    pub fn is_hidden(&self) -> bool {
        self.state.is_some()
    }

    /// Applies the original state and deletes the marker, only if the shell is there.
    /// Otherwise keeps the marker and remembers the restore as pending.
    fn put_back(&mut self, original_autohide: bool) {
        if !self.shell.available() {
            self.pending_restore = Some(original_autohide);
            return;
        }
        self.shell.set_autohide(original_autohide);
        self.shell.set_visible(true);
        self.pending_restore = None;
        let _ = fs::remove_file(&self.marker_path);
    }
}

#[cfg(test)]
mod tests {
    use super::watchdog::should_restore;
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct FakeTaskbar {
        available: bool,
        autohide: bool,
        visible: bool,
        calls: Vec<String>,
    }

    impl FakeTaskbar {
        fn new(autohide: bool) -> Self {
            Self { available: true, autohide, visible: true, calls: Vec::new() }
        }
    }

    impl ShellTaskbar for FakeTaskbar {
        fn available(&self) -> bool {
            self.available
        }
        fn autohide(&self) -> bool {
            self.autohide
        }
        fn set_autohide(&mut self, on: bool) {
            self.calls.push(format!("set_autohide({on})"));
            self.autohide = on;
        }
        fn set_visible(&mut self, visible: bool) {
            self.calls.push(format!("set_visible({visible})"));
            self.visible = visible;
        }
        fn any_visible(&self) -> bool {
            self.visible
        }
    }

    #[test]
    fn enforce_rehides_only_while_hidden() {
        let p = marker_path();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(false), p.clone());
        assert!(!g.enforce()); // not hidden: never touches the taskbar
        g.hide();
        assert!(!g.enforce()); // still hidden: nothing to do
        g.shell.visible = true; // Windows showed it again
        assert!(g.enforce());
        assert!(!g.shell.visible);
        g.shell.autohide = false;
        assert!(g.enforce());
        assert!(g.shell.autohide);
        g.restore();
        g.shell.visible = true;
        assert!(!g.enforce());
        assert!(g.shell.visible);
    }

    static N: AtomicU32 = AtomicU32::new(0);
    fn marker_path() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lucent-taskbar-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&d).unwrap();
        d.join("taskbar-state.json")
    }

    fn write_marker(p: &PathBuf, m: Marker) {
        fs::write(p, serde_json::to_string(&m).unwrap()).unwrap();
    }

    #[test]
    fn hide_saves_original_state_then_hides() {
        let p = marker_path();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(false), p.clone());
        g.hide();
        assert!(!g.shell.visible);
        assert!(g.shell.autohide);
        assert!(g.is_hidden());
        assert_eq!(read_marker(&p), Some(Marker { hidden: true, original_autohide: false }));
    }

    #[test]
    fn restore_puts_back_original_autohide_and_deletes_marker() {
        let p = marker_path();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(true), p.clone());
        g.hide();
        g.restore();
        assert!(g.shell.autohide);
        assert!(g.shell.visible);
        assert!(!p.exists());
        assert!(!g.is_hidden());
    }

    #[test]
    fn restore_after_hide_from_non_autohide_turns_autohide_off() {
        let p = marker_path();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(false), p.clone());
        g.hide();
        g.restore();
        assert!(!g.shell.autohide);
        assert!(g.shell.visible);
        assert!(!p.exists());
    }

    #[test]
    fn hide_twice_keeps_first_original_state() {
        let p = marker_path();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(false), p.clone());
        g.hide();
        g.hide();
        assert_eq!(read_marker(&p), Some(Marker { hidden: true, original_autohide: false }));
        g.restore();
        assert!(!g.shell.autohide);
        assert!(g.shell.visible);
    }

    #[test]
    fn recover_on_startup_restores_when_marker_says_hidden() {
        let p = marker_path();
        write_marker(&p, Marker { hidden: true, original_autohide: false });
        let mut fake = FakeTaskbar::new(true);
        fake.visible = false;
        let mut g = TaskbarGuard::new(fake, p.clone());
        g.recover_on_startup();
        assert!(g.shell.visible);
        assert!(!g.shell.autohide);
        assert!(!p.exists());
        assert!(!g.is_hidden());
    }

    #[test]
    fn recover_on_startup_noop_without_marker() {
        let p = marker_path();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(false), p.clone());
        g.recover_on_startup();
        assert!(g.shell.calls.is_empty());
    }

    #[test]
    fn recover_on_startup_with_corrupt_marker_shows_taskbar() {
        let p = marker_path();
        fs::write(&p, "{garbage").unwrap();
        let mut fake = FakeTaskbar::new(true);
        fake.visible = false;
        let mut g = TaskbarGuard::new(fake, p.clone());
        g.recover_on_startup();
        assert!(g.shell.visible);
        assert!(!g.shell.autohide);
        assert!(!p.exists());
    }

    #[test]
    fn explorer_restart_rehides_when_hidden() {
        let p = marker_path();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(false), p.clone());
        g.hide();
        g.shell.visible = true; // explorer restarted: fresh, visible taskbar
        g.shell.autohide = false;
        g.on_explorer_restarted();
        assert!(!g.shell.visible);
        assert!(g.shell.autohide);
        // original state must survive the re-hide
        assert_eq!(read_marker(&p), Some(Marker { hidden: true, original_autohide: false }));
    }

    #[test]
    fn explorer_restart_noop_when_not_hidden() {
        let p = marker_path();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(false), p.clone());
        g.on_explorer_restarted();
        assert!(g.shell.calls.is_empty());
        assert!(g.shell.visible);
    }

    #[test]
    fn hide_does_not_hide_when_marker_cannot_be_written() {
        // Parent of the marker is a regular file: the marker cannot be created.
        let p = marker_path();
        let blocker = p.with_file_name("blocker");
        fs::write(&blocker, "x").unwrap();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(false), blocker.join("taskbar-state.json"));
        g.hide();
        assert!(g.shell.visible);
        assert!(!g.shell.autohide);
        assert!(!g.is_hidden());
        assert!(!g.shell.calls.iter().any(|c| c.starts_with("set_")));
    }

    #[test]
    fn hide_with_shell_unavailable_does_nothing() {
        let p = marker_path();
        let mut fake = FakeTaskbar::new(false);
        fake.available = false;
        let mut g = TaskbarGuard::new(fake, p.clone());
        g.hide();
        assert!(g.shell.calls.is_empty());
        assert!(!p.exists());
        assert!(!g.is_hidden());
    }

    #[test]
    fn restore_with_shell_unavailable_keeps_marker() {
        let p = marker_path();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(false), p.clone());
        g.hide();
        g.shell.available = false; // explorer down
        g.shell.calls.clear();
        g.restore();
        assert!(g.shell.calls.is_empty());
        assert_eq!(read_marker(&p), Some(Marker { hidden: true, original_autohide: false }));
    }

    #[test]
    fn pending_restore_completes_on_explorer_restart() {
        let p = marker_path();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(false), p.clone());
        g.hide();
        g.shell.available = false;
        g.restore();
        // explorer comes back with a fresh taskbar still auto-hidden from our setting
        g.shell.available = true;
        g.shell.visible = true;
        g.on_explorer_restarted();
        assert!(g.shell.visible);
        assert!(!g.shell.autohide);
        assert!(!p.exists());
        assert!(!g.is_hidden());
    }

    #[test]
    fn recover_on_startup_with_shell_unavailable_keeps_marker() {
        let p = marker_path();
        write_marker(&p, Marker { hidden: true, original_autohide: false });
        let mut fake = FakeTaskbar::new(true);
        fake.available = false;
        let mut g = TaskbarGuard::new(fake, p.clone());
        g.recover_on_startup();
        assert!(g.shell.calls.is_empty());
        assert!(p.exists());
        // explorer appears later: the pending restore completes
        g.shell.available = true;
        g.on_explorer_restarted();
        assert!(g.shell.visible);
        assert!(!g.shell.autohide);
        assert!(!p.exists());
    }

    #[test]
    fn explorer_tray_skips_our_own_shell_traywnd() {
        let none = |_: u32| false;
        // z-order: ours (pid 10) is in front of Explorer's (pid 4); the shell is pid 4.
        assert_eq!(pick_explorer_tray(&[(111, 10), (222, 4)], Some(4), 10, none), Some(222));
        assert_eq!(pick_explorer_tray(&[(222, 4)], Some(4), 10, none), Some(222));
        assert_eq!(pick_explorer_tray(&[(111, 10)], Some(4), 10, none), None);
        assert_eq!(pick_explorer_tray(&[], Some(4), 10, none), None);
    }

    #[test]
    fn explorer_tray_ignores_another_lucent_instance() {
        // A second Lucent (pid 20) also owns a Shell_TrayWnd, in front of Explorer's.
        let none = |_: u32| false;
        assert_eq!(pick_explorer_tray(&[(333, 20), (111, 10), (222, 4)], Some(4), 10, none), Some(222));
        assert_eq!(pick_explorer_tray(&[(333, 20)], Some(4), 10, none), None);
    }

    #[test]
    fn explorer_tray_falls_back_to_explorer_exe_without_shell_window() {
        let is_explorer = |pid: u32| pid == 4;
        assert_eq!(pick_explorer_tray(&[(333, 20), (222, 4)], None, 10, is_explorer), Some(222));
        // Shell pid known but owning no tray (shell restarting): same fallback.
        assert_eq!(pick_explorer_tray(&[(333, 20), (222, 4)], Some(99), 10, is_explorer), Some(222));
        assert_eq!(pick_explorer_tray(&[(333, 20)], None, 10, is_explorer), None);
        // Never our own window, even if the check would accept it.
        assert_eq!(pick_explorer_tray(&[(111, 10)], Some(10), 10, |_| true), None);
    }

    #[test]
    fn second_hide_makes_no_shell_calls() {
        let p = marker_path();
        let mut g = TaskbarGuard::new(FakeTaskbar::new(false), p.clone());
        g.hide();
        g.shell.calls.clear();
        g.hide();
        assert!(g.shell.calls.is_empty(), "{:?}", g.shell.calls);
        assert!(g.is_hidden());
    }

    #[test]
    fn gate_hides_only_once_a_dock_is_ready() {
        let mut gate = HideGate::new(true);
        assert!(!gate.target());
        assert_eq!(gate.set_docks_ready(true), Some(true));
        assert!(gate.target());
    }

    #[test]
    fn gate_restores_when_docks_vanish_and_rehides_when_back() {
        let mut gate = HideGate::new(true);
        gate.set_docks_ready(true);
        assert_eq!(gate.set_docks_ready(false), Some(false));
        assert_eq!(gate.set_docks_ready(false), None);
        assert_eq!(gate.set_docks_ready(true), Some(true));
    }

    #[test]
    fn gate_acts_only_when_the_setting_changes() {
        let mut gate = HideGate::new(true);
        gate.set_docks_ready(true);
        assert_eq!(gate.set_want(true), None, "unrelated settings save");
        assert_eq!(gate.set_want(false), Some(false));
        assert_eq!(gate.set_want(false), None);
        assert_eq!(gate.set_want(true), Some(true));
    }

    #[test]
    fn gate_setting_on_without_docks_does_nothing() {
        let mut gate = HideGate::new(false);
        assert_eq!(gate.set_want(true), None);
        assert_eq!(gate.set_docks_ready(true), Some(true));
        let mut off = HideGate::new(false);
        assert_eq!(off.set_docks_ready(true), None);
    }

    #[test]
    fn watchdog_restores_when_parent_dies_with_marker() {
        assert!(should_restore(false, Some(Marker { hidden: true, original_autohide: false })));
    }

    #[test]
    fn watchdog_noop_when_parent_alive_or_no_marker() {
        assert!(!should_restore(true, Some(Marker { hidden: true, original_autohide: false })));
        assert!(!should_restore(false, None));
        assert!(!should_restore(false, Some(Marker { hidden: false, original_autohide: false })));
    }
}
