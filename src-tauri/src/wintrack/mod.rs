//! Top-level app window tracking. Pure logic lives here; `win` is the Win32 adapter.
use crate::settings::AppId;
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;

#[cfg(windows)]
pub mod win;

pub mod badge;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawWindow {
    pub hwnd: isize,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub visible: bool,
    pub cloaked: bool,
    pub has_owner: bool,
    pub tool_window: bool,
    pub app_window: bool,
    pub exe_path: Option<String>,
    pub aumid: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackedWindow {
    pub hwnd: isize,
    pub title: String,
    pub app: AppId,
}

pub const DEBOUNCE_QUIET: Duration = Duration::from_millis(50);
pub const DEBOUNCE_MAX_WAIT: Duration = Duration::from_millis(250);

/// How long to keep waiting for more events, given time elapsed since the burst's first
/// event. `None` means flush now (max wait reached).
pub fn debounce_timeout(elapsed: Duration, quiet: Duration, max_wait: Duration) -> Option<Duration> {
    let left = max_wait.checked_sub(elapsed).filter(|d| !d.is_zero())?;
    Some(quiet.min(left))
}

const EXCLUDED_CLASSES: [&str; 4] = ["Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Progman", "WorkerW"];

/// Alt-Tab rule: which windows deserve a dock entry.
pub fn is_dock_window(w: &RawWindow, own_pid: u32) -> bool {
    w.visible
        && !w.cloaked
        && !w.title.is_empty()
        && w.pid != own_pid
        && (w.app_window || (!w.tool_window && !w.has_owner))
        && !EXCLUDED_CLASSES.contains(&w.class.as_str())
}

/// `ApplicationFrameHost.exe` hosts every UWP app window.
pub fn is_frame_host(exe_path: &str) -> bool {
    exe_path.to_lowercase().ends_with("applicationframehost.exe")
}

/// True for exes inside `Program Files\WindowsApps`: Windows refuses to launch them by path.
pub fn is_windowsapps_path(exe_path: &str) -> bool {
    exe_path.to_lowercase().contains("\\windowsapps\\")
}

/// Package family name (`<name>_<publisherid>`, lowercase) of an exe under `WindowsApps`,
/// from its package folder `<name>_<version>_<arch>_<resourceid>_<publisherid>`.
pub fn packaged_family(exe_path: &str) -> Option<String> {
    const MARK: &str = "\\windowsapps\\";
    let lower = exe_path.to_lowercase();
    let rest = &lower[lower.find(MARK)? + MARK.len()..];
    // The package folder must contain the exe (a separator follows it).
    let (folder, _) = rest.split_once('\\')?;
    let parts: Vec<&str> = folder.split('_').collect();
    match parts.as_slice() {
        [name, version, _arch, _resource, publisher] if !name.is_empty() && !version.is_empty() && !publisher.is_empty() => {
            Some(format!("{name}_{publisher}"))
        }
        _ => None,
    }
}

/// First AUMID (`<family>!<app>`) whose family matches, case-insensitively.
pub fn aumid_for_family<'a>(family: &str, aumids: impl IntoIterator<Item = &'a str>) -> Option<String> {
    if family.is_empty() {
        return None;
    }
    aumids
        .into_iter()
        .find(|a| a.split_once('!').is_some_and(|(f, _)| f.eq_ignore_ascii_case(family)))
        .map(str::to_string)
}

/// Packaged (MSIX/UWP) apps live under `WindowsApps` or run inside the frame host: only
/// their AUMID can be resolved through `shell:AppsFolder` and the package manager.
fn is_packaged(exe_path: Option<&String>) -> bool {
    exe_path.is_none_or(|p| is_frame_host(p) || is_windowsapps_path(p))
}

pub fn app_id_of(w: &RawWindow) -> AppId {
    // Win32 apps may set an explicit AUMID of their own (Brave/Chrome profiles:
    // "Brave.UserData.Profile1") that no shortcut or package knows: the exe identifies them.
    if let Some(a) = w.aumid.as_ref().filter(|a| !a.is_empty()) {
        if is_packaged(w.exe_path.as_ref()) {
            return AppId::Aumid(a.clone());
        }
    }
    match w.exe_path.as_ref().filter(|p| !p.is_empty()) {
        // A UWP frame whose app could not be identified: never lump every such app
        // together under the frame host.
        Some(p) if is_frame_host(p) => AppId::Exe("unknown".into()),
        Some(p) => AppId::Exe(p.to_lowercase()),
        None => AppId::Exe("unknown".into()),
    }
}

/// AUMIDs of UWP frames, remembered per frame window. A minimised UWP app's
/// `CoreWindow` detaches from its `ApplicationFrameHost` frame, so the frame can only be
/// identified from what was resolved while it was restored.
#[derive(Debug, Default)]
pub struct AumidCache {
    map: std::collections::HashMap<isize, String>,
}

impl AumidCache {
    /// `core`: AUMID of the frame's `CoreWindow` child, if attached (remembered). Else the
    /// value remembered for this frame, else `frame`: the frame's own AUMID (asked last and
    /// not remembered: only a `CoreWindow` answer is trusted enough to stick).
    pub fn resolve(&mut self, hwnd: isize, core: Option<String>, frame: impl FnOnce() -> Option<String>) -> Option<String> {
        if let Some(a) = core {
            self.map.insert(hwnd, a.clone());
            return Some(a);
        }
        self.map.get(&hwnd).cloned().or_else(frame)
    }

    /// Forgets windows that no longer exist (handles get reused).
    pub fn retain_alive(&mut self, alive: &[isize]) {
        self.map.retain(|h, _| alive.contains(h));
    }
}

type Callback = Box<dyn Fn(Vec<TrackedWindow>) + Send + 'static>;

#[derive(Default)]
pub struct WindowTracker {
    /// First-seen order, stable across updates.
    windows: Mutex<Vec<TrackedWindow>>,
    callbacks: Mutex<Vec<Callback>>,
}

impl WindowTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<TrackedWindow> {
        lock(&self.windows).clone()
    }

    pub fn on_change(&self, cb: impl Fn(Vec<TrackedWindow>) + Send + 'static) {
        lock(&self.callbacks).push(Box::new(cb));
    }

    /// Feeds a fresh enumeration (z-order). Known windows keep their slot, new ones
    /// are appended in z-order, vanished ones drop out. Callbacks fire only on change.
    pub fn update(&self, raws: &[RawWindow], own_pid: u32) {
        let fresh: Vec<TrackedWindow> = raws
            .iter()
            .filter(|w| is_dock_window(w, own_pid))
            .map(|w| TrackedWindow { hwnd: w.hwnd, title: w.title.clone(), app: app_id_of(w) })
            .collect();
        let next = {
            let mut cur = lock(&self.windows);
            let mut next: Vec<TrackedWindow> = cur
                .iter()
                .filter_map(|old| fresh.iter().find(|f| f.hwnd == old.hwnd).cloned())
                .collect();
            for f in &fresh {
                if !next.iter().any(|n| n.hwnd == f.hwnd) {
                    next.push(f.clone());
                }
            }
            let changed = next.len() != cur.len()
                || next.iter().zip(cur.iter()).any(|(a, b)| {
                    a.hwnd != b.hwnd || a.title != b.title || a.app != b.app
                });
            if !changed {
                return;
            }
            *cur = next.clone();
            next
        };
        for cb in lock(&self.callbacks).iter() {
            cb(next.clone());
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

    fn raw(exe: &str, title: &str) -> RawWindow {
        RawWindow {
            hwnd: 100,
            pid: 42,
            title: title.into(),
            class: "Chrome_WidgetWin_1".into(),
            visible: true,
            cloaked: false,
            has_owner: false,
            tool_window: false,
            app_window: false,
            exe_path: Some(format!(r"C:\Apps\{exe}")),
            aumid: None,
        }
    }
    fn with_exe(p: &str) -> RawWindow {
        let mut w = raw("x.exe", "t");
        w.exe_path = Some(p.into());
        w
    }

    #[test] fn normal_app_window_included() { assert!(is_dock_window(&raw("Code.exe", "awax - VS Code"), 1)); }
    #[test] fn tool_window_excluded() {
        let mut w = raw("a.exe", "t"); w.tool_window = true;
        assert!(!is_dock_window(&w, 1));
    }
    #[test] fn owned_window_excluded_unless_appwindow() {
        let mut w = raw("a.exe", "t"); w.has_owner = true;
        assert!(!is_dock_window(&w, 1));
        w.app_window = true;
        assert!(is_dock_window(&w, 1));
    }
    #[test] fn cloaked_and_untitled_excluded() {
        let mut w = raw("a.exe", "t"); w.cloaked = true;
        assert!(!is_dock_window(&w, 1));
        let w = raw("a.exe", "");
        assert!(!is_dock_window(&w, 1));
        let mut w = raw("a.exe", "t"); w.visible = false;
        assert!(!is_dock_window(&w, 1));
    }
    #[test] fn shell_classes_excluded() {
        for c in ["Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Progman", "WorkerW"] {
            let mut w = raw("a.exe", "t"); w.class = c.into();
            assert!(!is_dock_window(&w, 1), "{c}");
        }
    }
    #[test] fn own_process_excluded() {
        let w = raw("a.exe", "t");
        assert!(!is_dock_window(&w, w.pid));
    }
    #[test] fn exe_grouping_is_case_insensitive() {
        assert_eq!(app_id_of(&with_exe(r"C:\X\Code.exe")), app_id_of(&with_exe(r"c:\x\code.exe")));
    }
    #[test] fn uwp_frame_host_grouped_by_aumid() {
        let mut w = raw("ApplicationFrameHost.exe", "Calculatrice");
        w.aumid = Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into());
        assert_eq!(app_id_of(&w), AppId::Aumid("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()));
    }
    #[test] fn win32_explicit_aumid_is_identified_by_exe() {
        let mut w = with_exe(r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe");
        w.aumid = Some("Brave.UserData.Profile1".into());
        assert_eq!(app_id_of(&w), AppId::Exe(r"c:\program files\bravesoftware\brave-browser\application\brave.exe".into()));
    }
    #[test] fn packaged_win32_app_keeps_its_aumid() {
        let mut w = with_exe(r"C:\Program Files\WindowsApps\SpotifyAB.SpotifyMusic_1.0_x64__zpdnekdrzrea0\Spotify.exe");
        w.aumid = Some("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify".into());
        assert_eq!(app_id_of(&w), AppId::Aumid("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify".into()));
    }
    const SPOTIFY_EXE: &str = r"c:\program files\windowsapps\spotifyab.spotifymusic_1.308.620.0_x64__zpdnekdrzrea0\spotify.exe";
    #[test] fn packaged_family_of_store_exe() {
        assert_eq!(packaged_family(SPOTIFY_EXE).as_deref(), Some("spotifyab.spotifymusic_zpdnekdrzrea0"));
        // Case-insensitive, resource id present, nested exe.
        assert_eq!(
            packaged_family(r"C:\Program Files\WindowsApps\Foo.Bar_2.0.0.0_neutral_split.scale-100_abc123\app\bin\Foo.exe").as_deref(),
            Some("foo.bar_abc123"),
        );
    }
    #[test] fn packaged_family_rejects_other_paths() {
        assert_eq!(packaged_family(r"c:\program files\spotify\spotify.exe"), None);
        assert_eq!(packaged_family(r"c:\program files\windowsapps\notapackage\x.exe"), None);
        assert_eq!(packaged_family(r"c:\program files\windowsapps\a_1_x64__\x.exe"), None);
        assert_eq!(packaged_family(r"c:\program files\windowsapps\_1_x64__pub\x.exe"), None);
        assert_eq!(packaged_family(r"c:\program files\windowsapps\spotify.exe"), None);
        assert_eq!(packaged_family(""), None);
    }
    #[test] fn aumid_for_family_matches_case_insensitively() {
        let list = ["Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify", "Mozilla.Firefox"];
        assert_eq!(
            aumid_for_family("spotifyab.spotifymusic_zpdnekdrzrea0", list).as_deref(),
            Some("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"),
        );
        assert_eq!(aumid_for_family("spotifyab.spotifymusic", list), None);
        assert_eq!(aumid_for_family("mozilla.firefox", list), None);
        assert_eq!(aumid_for_family("", list), None);
    }
    #[test] fn windowsapps_path_detection() {
        assert!(is_windowsapps_path(SPOTIFY_EXE));
        assert!(!is_windowsapps_path(r"c:\windows\explorer.exe"));
    }
    #[test] fn unresolved_uwp_frame_is_never_grouped_under_frame_host() {
        let mut w = raw("ApplicationFrameHost.exe", "Paramètres");
        w.exe_path = Some(r"C:\Windows\System32\ApplicationFrameHost.exe".into());
        w.aumid = None;
        assert_eq!(app_id_of(&w), AppId::Exe("unknown".into()));
    }
    #[test] fn aumid_cache_prefers_core_window_then_cache_then_frame() {
        let mut c = AumidCache::default();
        assert_eq!(c.resolve(1, Some("Core!App".into()), || Some("Frame!App".into())), Some("Core!App".into()));
        // Detached CoreWindow: what the CoreWindow said beats whatever the frame says.
        assert_eq!(c.resolve(1, None, || Some("Frame!App".into())), Some("Core!App".into()));
        // Never seen restored: the frame's own AUMID, not cached.
        assert_eq!(c.resolve(2, None, || Some("Frame!App".into())), Some("Frame!App".into()));
        assert_eq!(c.resolve(2, None, || None), None);
    }
    #[test] fn aumid_cache_remembers_last_resolution_when_minimised() {
        let mut c = AumidCache::default();
        c.resolve(1, Some("Calc!App".into()), || None);
        // Minimised: the CoreWindow detached, the frame has no AUMID of its own.
        assert_eq!(c.resolve(1, None, || None), Some("Calc!App".into()));
        assert_eq!(c.resolve(2, None, || None), None);
        // A fresh resolution replaces the cached one.
        c.resolve(1, Some("Other!App".into()), || None);
        assert_eq!(c.resolve(1, None, || None), Some("Other!App".into()));
    }
    #[test] fn aumid_cache_forgets_dead_windows() {
        let mut c = AumidCache::default();
        c.resolve(1, Some("A!App".into()), || None);
        c.resolve(2, Some("B!App".into()), || None);
        c.retain_alive(&[2]);
        assert_eq!(c.resolve(1, None, || None), None);
        assert_eq!(c.resolve(2, None, || None), Some("B!App".into()));
    }
    #[test] fn elevated_window_without_exe_path_does_not_panic() {
        let mut w = raw("a.exe", "t"); w.exe_path = None; w.aumid = None;
        assert_eq!(app_id_of(&w), AppId::Exe("unknown".into()));
    }
    #[test] fn debounce_waits_quiet_period_early_in_burst() {
        let q = Duration::from_millis(50); let m = Duration::from_millis(250);
        assert_eq!(debounce_timeout(Duration::ZERO, q, m), Some(q));
        assert_eq!(debounce_timeout(Duration::from_millis(230), q, m), Some(Duration::from_millis(20)));
    }
    #[test] fn debounce_continuous_stream_flushes_by_deadline() {
        let q = Duration::from_millis(50); let m = Duration::from_millis(250);
        // An event every 10 ms never lets the quiet period elapse.
        let mut elapsed = Duration::ZERO;
        let mut flushed = None;
        for _ in 0..1000 {
            match debounce_timeout(elapsed, q, m) {
                None => { flushed = Some(elapsed); break; }
                Some(t) => elapsed += t.min(Duration::from_millis(10)),
            }
        }
        let f = flushed.expect("must flush");
        assert!(f <= m, "{f:?}");
    }
    #[test] fn cheap_fields_decide_eligibility_without_identity() {
        // Eligibility must not depend on exe_path/aumid (resolved lazily after the filter).
        let mut a = raw("a.exe", "t"); a.exe_path = None; a.aumid = None;
        assert!(is_dock_window(&a, 1));
    }
    fn at(hwnd: isize, title: &str) -> RawWindow {
        let mut w = raw("a.exe", title); w.hwnd = hwnd; w
    }
    #[test] fn order_is_first_seen_and_stable() {
        let t = WindowTracker::new();
        t.update(&[at(1, "a"), at(2, "b")], 0);
        t.update(&[at(2, "b"), at(3, "c"), at(1, "a")], 0);
        let h: Vec<isize> = t.snapshot().iter().map(|w| w.hwnd).collect();
        assert_eq!(h, vec![1, 2, 3]);
        t.update(&[at(3, "c"), at(1, "a")], 0);
        let h: Vec<isize> = t.snapshot().iter().map(|w| w.hwnd).collect();
        assert_eq!(h, vec![1, 3]);
    }
    #[test] fn callback_fires_only_on_change() {
        let t = WindowTracker::new();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        t.on_change(move |_| { n2.fetch_add(1, Ordering::SeqCst); });
        t.update(&[at(1, "a")], 0);
        t.update(&[at(1, "a")], 0);
        t.update(&[at(1, "renamed")], 0);
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }
}
