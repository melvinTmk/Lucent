//! Tray host: our own `Shell_TrayWnd` receives `Shell_NotifyIcon` traffic while the Windows
//! taskbar is hidden. `HostCore` is the pure logic (tested); `win` is the Win32 shell.
//!
//! Everything we receive is also forwarded to Explorer's (hidden) tray, so Explorer stays
//! consistent and shows the icons again as soon as Lucent quits or dies.
use super::protocol::{NimMessage, SHELLTRAYDATA_SIGNATURE, parse_copydata};
use super::registry::{TrayIconView, TrayKey, TrayRegistry};
use crate::monitors::Rect;
use std::collections::HashMap;

#[cfg(windows)]
pub mod win;

/// `WM_COPYDATA.dwData` values sent to the `Shell_TrayWnd`.
pub const COPYDATA_APPBAR: usize = 0;
pub const COPYDATA_NOTIFY: usize = 1;
pub const COPYDATA_ICON_RECT: usize = 3;
/// `tray://icons` is emitted at most once per window, at its end.
pub const DEBOUNCE_MS: u64 = 100;
/// A `TaskbarCreated` this soon after our own broadcast is that broadcast coming back.
pub const ECHO_MS: u64 = 1000;
/// Dead owner windows are pruned on this period.
pub const PRUNE_MS: u64 = 5000;

/// Talks to Explorer's real tray. Implemented on Win32 by `win::Win32Forwarder`.
pub trait Forwarder {
    /// Re-sends a `WM_COPYDATA` payload to Explorer's `Shell_TrayWnd`; returns its result.
    fn forward(&self, dw: usize, bytes: &[u8]) -> isize;
    /// `SendNotifyMessageW(HWND_BROADCAST, TaskbarCreated)`: apps re-add their icons.
    fn broadcast_taskbar_created(&self);
}

/// What `tray_click` needs to talk to an icon's owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClickTarget {
    pub hwnd: u32,
    pub uid: u32,
    pub callback_message: u32,
    pub version: u32,
}

pub struct HostCore<F: Forwarder> {
    fwd: F,
    registry: TrayRegistry,
    /// Deadline of the pending `tray://icons` emit (bounded: never pushed back).
    emit_at: Option<u64>,
    last_broadcast: Option<u64>,
    /// Last on-screen rect of each icon (physical px), keyed by `TrayKey::as_string`.
    icon_rects: HashMap<String, Rect>,
    /// PNG revision per key, appended to the URL so the webview reloads changed icons.
    revs: HashMap<String, u32>,
    next_rev: u32,
}

impl<F: Forwarder> HostCore<F> {
    pub fn new(fwd: F) -> Self {
        Self {
            fwd,
            registry: TrayRegistry::new(),
            emit_at: None,
            last_broadcast: None,
            icon_rects: HashMap::new(),
            revs: HashMap::new(),
            next_rev: 1,
        }
    }

    pub fn forwarder(&self) -> &F {
        &self.fwd
    }

    pub fn registry(&self) -> &TrayRegistry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut TrayRegistry {
        &mut self.registry
    }

    /// Handles a `WM_COPYDATA` received by our `Shell_TrayWnd`; returns the `LRESULT`.
    pub fn on_copydata(&mut self, dw: usize, bytes: &[u8], now: u64) -> isize {
        match dw {
            COPYDATA_NOTIFY => {
                let ok = match parse_copydata(dw, bytes) {
                    Some((msg, data)) => {
                        let ok = self.registry.apply(msg, data);
                        if ok && msg != NimMessage::SetFocus {
                            self.mark_changed(now);
                        }
                        ok
                    }
                    None => false,
                };
                // Explorer's answer is ignored: the app talks to us.
                self.fwd.forward(dw, bytes);
                ok as isize
            }
            COPYDATA_ICON_RECT => match self.icon_rect_reply(bytes) {
                Some(v) => v,
                None => self.fwd.forward(dw, bytes),
            },
            _ => self.fwd.forward(dw, bytes),
        }
    }

    /// `Shell_NotifyIconGetRect` (`WINNOTIFYICONIDENTIFIER`): message 1 asks for the
    /// top-left corner, 2 for the bottom-right, each packed as `MAKELONG(x, y)`.
    fn icon_rect_reply(&self, b: &[u8]) -> Option<isize> {
        let u = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
        if u(0)? != SHELLTRAYDATA_SIGNATURE {
            return None;
        }
        let (msg, hwnd, uid) = (u(4)?, u(16)?, u(20)?);
        let guid: Option<[u8; 16]> = b.get(24..40).and_then(|g| {
            let mut a = [0u8; 16];
            a.copy_from_slice(g);
            (a != [0; 16]).then_some(a)
        });
        let icon = self.registry.icons().iter().find(|i| match (guid, &i.key) {
            (Some(g), TrayKey::Guid(k)) => g == *k,
            _ => i.data.hwnd == hwnd && i.data.uid == uid,
        })?;
        let r = self.icon_rects.get(&icon.key.as_string())?;
        let (x, y) = match msg {
            1 => (r.x, r.y),
            2 => (r.x + r.w, r.y + r.h),
            _ => return None,
        };
        Some(((x as u16 as u32) | ((y as u16 as u32) << 16)) as i32 as isize)
    }

    /// Our window exists: make running apps re-add their icons to it.
    pub fn start(&mut self, now: u64) {
        self.broadcast(now);
    }

    /// `TaskbarCreated` received. Returns true when it was Explorer's (restart), in which
    /// case the caller re-asserts our z-order; we have already rebroadcast.
    pub fn on_taskbar_created(&mut self, now: u64) -> bool {
        if self.last_broadcast.is_some_and(|t| now.saturating_sub(t) < ECHO_MS) {
            return false;
        }
        self.broadcast(now);
        true
    }

    fn broadcast(&mut self, now: u64) {
        self.last_broadcast = Some(now);
        self.fwd.broadcast_taskbar_created();
    }

    pub fn mark_changed(&mut self, now: u64) {
        if self.emit_at.is_none() {
            self.emit_at = Some(now + DEBOUNCE_MS);
        }
    }

    /// Time left before the pending emit, if any.
    pub fn emit_due_in(&self, now: u64) -> Option<u64> {
        self.emit_at.map(|t| t.saturating_sub(now))
    }

    /// True (once) when a pending emit is due.
    pub fn take_emit(&mut self, now: u64) -> bool {
        match self.emit_at {
            Some(t) if now >= t => {
                self.emit_at = None;
                true
            }
            _ => false,
        }
    }

    /// Drops icons whose owner window is gone (and their rect/revision).
    pub fn prune(&mut self, is_alive: impl Fn(u32) -> bool, now: u64) -> bool {
        if !self.registry.prune(is_alive) {
            return false;
        }
        let live: Vec<String> = self.registry.icons().iter().map(|i| i.key.as_string()).collect();
        self.icon_rects.retain(|k, _| live.contains(k));
        self.revs.retain(|k, _| live.contains(k));
        self.mark_changed(now);
        true
    }

    pub fn set_icon_rect(&mut self, key: &str, r: Rect) {
        self.icon_rects.insert(key.to_string(), r);
    }

    /// Stores the PNG of icon `idx` and bumps its URL revision.
    pub fn set_png(&mut self, idx: usize, png: Vec<u8>) {
        let Some(icon) = self.registry.icons_mut().get_mut(idx) else { return };
        icon.icon_png = Some(png);
        self.revs.insert(icon.key.as_string(), self.next_rev);
        self.next_rev = self.next_rev.wrapping_add(1);
    }

    pub fn views(&self) -> Vec<TrayIconView> {
        let mut v = self.registry.views();
        for view in &mut v {
            let rev = self.revs.get(&view.key).copied().unwrap_or(0);
            view.icon_url = format!("{}?v={rev}", view.icon_url);
        }
        v
    }

    pub fn click_target(&self, key: &str) -> Option<ClickTarget> {
        let i = self.registry.icons().iter().find(|i| i.key.as_string() == key)?;
        Some(ClickTarget {
            hwnd: i.data.hwnd,
            uid: i.data.uid,
            callback_message: i.data.callback_message,
            version: i.data.version,
        })
    }
}

/// Straight RGBA from an icon's bitmaps read as 32 bpp top-down BGRA (`GetDIBits`).
/// - `color` with any alpha: used as is (icon color bitmaps carry straight alpha);
/// - `color` without alpha: alpha from the AND mask (white = transparent);
/// - no `color` (monochrome icon): `mask` is AND rows then XOR rows (2 × `h`).
pub fn icon_rgba(color: Option<&[u8]>, mask: &[u8], w: u32, h: u32) -> Vec<u8> {
    let n = (w * h) as usize;
    let px = |buf: &[u8], i: usize| -> [u8; 4] {
        buf.get(i * 4..i * 4 + 4).map(|p| [p[0], p[1], p[2], p[3]]).unwrap_or([0; 4])
    };
    let white = |p: [u8; 4]| p[0] > 127;
    let mut out = Vec::with_capacity(n * 4);
    match color {
        Some(c) => {
            let has_alpha = (0..n).any(|i| px(c, i)[3] != 0);
            for i in 0..n {
                let p = px(c, i);
                let a = if has_alpha { p[3] } else if white(px(mask, i)) { 0 } else { 255 };
                out.extend_from_slice(&[p[2], p[1], p[0], a]);
            }
        }
        None => {
            for i in 0..n {
                let and = white(px(mask, i));
                let xor = px(mask, n + i);
                if and && !white(xor) {
                    out.extend_from_slice(&[0, 0, 0, 0]);
                } else {
                    out.extend_from_slice(&[xor[2], xor[1], xor[0], 255]);
                }
            }
        }
    }
    out
}

/// One `HKCU\Control Panel\NotifyIconSettings\<id>` entry (Windows 11).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotifySetting {
    /// Full path, known-folder prefix already expanded.
    pub exe: Option<String>,
    pub uid: Option<u32>,
    pub guid: Option<[u8; 16]>,
    pub promoted: bool,
}

/// `"{6D809377-6AF0-444B-8957-A3773F02200E}"` → GUID bytes in memory order (as in
/// `NOTIFYICONDATA.guidItem`).
pub fn parse_guid(s: &str) -> Option<[u8; 16]> {
    let s = s.trim().trim_start_matches('{').trim_end_matches('}');
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 || [8, 4, 4, 4, 12] != [parts[0].len(), parts[1].len(), parts[2].len(), parts[3].len(), parts[4].len()] {
        return None;
    }
    let d1 = u32::from_str_radix(parts[0], 16).ok()?;
    let d2 = u16::from_str_radix(parts[1], 16).ok()?;
    let d3 = u16::from_str_radix(parts[2], 16).ok()?;
    let tail = format!("{}{}", parts[3], parts[4]);
    let mut g = [0u8; 16];
    g[0..4].copy_from_slice(&d1.to_le_bytes());
    g[4..6].copy_from_slice(&d2.to_le_bytes());
    g[6..8].copy_from_slice(&d3.to_le_bytes());
    for i in 0..8 {
        g[8 + i] = u8::from_str_radix(tail.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(g)
}

/// `ExecutablePath` values may start with a known-folder GUID (`{GUID}\Steam\steam.exe`).
pub fn expand_known_folder(path: &str, mut resolve: impl FnMut(&str) -> Option<String>) -> String {
    if path.starts_with('{') {
        if let Some(end) = path.find('}') {
            if let Some(base) = resolve(&path[..=end]) {
                return format!("{base}{}", &path[end + 1..]);
            }
        }
    }
    path.to_string()
}

/// Windows 11 "show in taskbar" state for an icon: match by GUID first, else by owner
/// exe path (case-insensitive) and UID when the entry records one. Default false.
pub fn is_promoted(settings: &[NotifySetting], guid: Option<[u8; 16]>, exe: Option<&str>, uid: u32) -> bool {
    if let Some(g) = guid {
        if let Some(s) = settings.iter().find(|s| s.guid == Some(g)) {
            return s.promoted;
        }
    }
    let Some(exe) = exe else { return false };
    settings
        .iter()
        .find(|s| s.exe.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(exe)) && s.uid.is_none_or(|u| u == uid))
        .is_some_and(|s| s.promoted)
}


// ---- Window message routing (our `Shell_TrayWnd` stands in for Explorer's) -------------

const WM_CLOSE: u32 = 0x0010;
const WM_COPYDATA: u32 = 0x004A;
const WM_NCDESTROY: u32 = 0x0082;
const WM_COMMAND: u32 = 0x0111;
const WM_TIMER: u32 = 0x0113;
const WM_USER: u32 = 0x0400;
const WM_APP: u32 = 0x8000;
/// Private message that stops the host (`WM_CLOSE` from anyone else is ignored).
pub const WM_APP_STOP: u32 = WM_APP + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    CopyData,
    Timer,
    TaskbarCreated,
    /// Our own stop request: destroy the window.
    Stop,
    /// Genuine end of the window: leave the message loop.
    Quit,
    /// Swallow (answer 0).
    Ignore,
    /// Taskbar-directed traffic meant for Explorer (`WM_COMMAND` such as 407/415/419,
    /// `WM_USER + n` queries): relay to Explorer's tray and return its answer.
    Forward,
    /// System/window-manager traffic about our own window: `DefWindowProcW`.
    Default,
}

/// Decides what our `Shell_TrayWnd` does with `msg`. `foreign`: sent from another thread
/// (`InSendMessage`). Broadcasts and system messages are never relayed (Explorer gets
/// them itself); only the messages apps aim at "the taskbar" are.
pub fn route(msg: u32, taskbar_created: u32, foreign: bool) -> Route {
    match msg {
        WM_COPYDATA => Route::CopyData,
        WM_TIMER => Route::Timer,
        WM_APP_STOP => Route::Stop,
        WM_CLOSE => Route::Ignore,
        // A real WM_NCDESTROY is sent by the system on our own thread, never from outside.
        WM_NCDESTROY if foreign => Route::Ignore,
        WM_NCDESTROY => Route::Quit,
        m if taskbar_created != 0 && m == taskbar_created => Route::TaskbarCreated,
        WM_COMMAND => Route::Forward,
        m if (WM_USER..WM_APP).contains(&m) => Route::Forward,
        _ => Route::Default,
    }
}

/// `WM_APP_STOP` only stops the host when `stop()` asked for it (any process can post
/// `WM_APP + 1` to our window).
pub fn route_stop(requested_by_us: bool) -> Route {
    if requested_by_us { Route::Stop } else { Route::Ignore }
}

/// Host thread lifecycle. Each run gets a generation so the exit of an old thread never
/// clears a newer run, and an unexpected exit leaves the host startable again.
#[derive(Debug, Default)]
pub struct Lifecycle {
    running: Option<u64>,
    next: u64,
}

impl Lifecycle {
    pub const fn new() -> Self {
        Self { running: None, next: 0 }
    }

    /// A new run's generation, or `None` if one is already running.
    pub fn begin(&mut self) -> Option<u64> {
        if self.running.is_some() {
            return None;
        }
        self.next += 1;
        self.running = Some(self.next);
        self.running
    }

    /// `stop()`: the generation to stop, if running (the slot is freed right away).
    pub fn take(&mut self) -> Option<u64> {
        self.running.take()
    }

    /// The host thread of `generation` ended, for any reason. `true` when it ended on its
    /// own (still the running generation: `stop()` did not take it), i.e. unexpectedly.
    pub fn exited(&mut self, generation: u64) -> bool {
        if self.running == Some(generation) {
            self.running = None;
            return true;
        }
        false
    }

    pub fn running(&self) -> Option<u64> {
        self.running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::protocol::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NotifyIconData, fixture};
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeFwd {
        forwarded: RefCell<Vec<(usize, Vec<u8>)>>,
        broadcasts: RefCell<u32>,
    }
    impl Forwarder for FakeFwd {
        fn forward(&self, dw: usize, bytes: &[u8]) -> isize {
            self.forwarded.borrow_mut().push((dw, bytes.to_vec()));
            42
        }
        fn broadcast_taskbar_created(&self) {
            *self.broadcasts.borrow_mut() += 1;
        }
    }

    fn core() -> HostCore<FakeFwd> {
        HostCore::new(FakeFwd::default())
    }


    #[test]
    fn unhandled_message_is_forwarded_to_explorer() {
        let tc = 0xC0DE;
        assert_eq!(route(0x0111, tc, true), Route::Forward); // WM_COMMAND 407/415/419
        assert_eq!(route(0x0111, tc, false), Route::Forward); // posted
        assert_eq!(route(0x0400 + 13, tc, true), Route::Forward); // WM_USER + n
        assert_eq!(route(0x7FFF, tc, true), Route::Forward);
        // Our own and system traffic is not relayed.
        assert_eq!(route(0x004A, tc, true), Route::CopyData);
        assert_eq!(route(0x0113, tc, false), Route::Timer);
        assert_eq!(route(tc, tc, true), Route::TaskbarCreated);
        assert_eq!(route(0x0081, tc, false), Route::Default); // WM_NCCREATE
        assert_eq!(route(0x0046, tc, false), Route::Default); // WM_WINDOWPOSCHANGING
        assert_eq!(route(0x001A, tc, true), Route::Default); // WM_SETTINGCHANGE broadcast
        assert_eq!(route(0x8000 + 5, tc, true), Route::Default); // WM_APP range
        assert_eq!(route(0xC123, tc, true), Route::Default); // other registered messages
    }

    #[test]
    fn foreign_close_is_ignored() {
        assert_eq!(route(0x0010, 0, true), Route::Ignore);
        assert_eq!(route(0x0010, 0, false), Route::Ignore);
        assert_eq!(route(0x0082, 0, true), Route::Ignore); // fake WM_NCDESTROY
        assert_eq!(route(0x0002, 0, true), Route::Default); // WM_DESTROY: no effect
        assert_eq!(route(0x0082, 0, false), Route::Quit);
    }

    #[test]
    fn stop_is_honoured_only_when_requested_by_us() {
        assert_eq!(route_stop(true), Route::Stop);
        // WM_APP + 1 posted by any other process: ignored.
        assert_eq!(route_stop(false), Route::Ignore);
        assert_eq!(route(WM_APP_STOP, 0, true), Route::Stop, "route() leaves the check to route_stop");
    }

    #[test]
    fn lifecycle_reports_unexpected_exits_only() {
        let mut l = Lifecycle::default();
        let g1 = l.begin().unwrap();
        assert!(l.exited(g1), "thread died on its own: restart wanted");
        let g2 = l.begin().unwrap();
        l.take(); // stop()
        assert!(!l.exited(g2), "stopped by us");
        assert!(!l.exited(g1), "stale");
    }

    #[test]
    fn host_lifecycle_allows_restart_after_unexpected_exit() {
        let mut l = Lifecycle::default();
        let g1 = l.begin().unwrap();
        assert_eq!(l.begin(), None, "already running");
        l.exited(g1); // thread died on its own
        let g2 = l.begin().expect("startable again");
        assert_ne!(g1, g2);
        l.exited(g1); // stale exit of the old thread: no effect
        assert_eq!(l.running(), Some(g2));
        assert_eq!(l.take(), Some(g2)); // stop()
        assert_eq!(l.take(), None);
        l.exited(g2);
        assert!(l.begin().is_some());
    }

    #[test]
    fn copydata_type1_is_applied_and_forwarded() {
        let mut c = core();
        let add = fixture::discord_add();
        assert_eq!(c.on_copydata(1, &add, 0), 1);
        assert_eq!(c.registry().icons().len(), 1);
        assert_eq!(c.forwarder().forwarded.borrow().as_slice(), &[(1, add.clone())]);
        // Modify of an unknown icon: registry says false, still forwarded once more.
        let unknown = fixture::build(1, &NotifyIconData { hwnd: 9, uid: 9, flags: NIF_TIP, ..Default::default() });
        assert_eq!(c.on_copydata(1, &unknown, 0), 0);
        assert_eq!(c.forwarder().forwarded.borrow().len(), 2);
        // Garbage type-1 payload: not applied, forwarded, reports failure.
        assert_eq!(c.on_copydata(1, &[1, 2, 3], 0), 0);
        assert_eq!(c.forwarder().forwarded.borrow().len(), 3);
    }

    #[test]
    fn copydata_type0_and_3_forwarded_untouched() {
        let mut c = core();
        let appbar = vec![7u8; 48];
        assert_eq!(c.on_copydata(0, &appbar, 0), 42);
        let query = icon_query(1, 0x10A2C, 7);
        assert_eq!(c.on_copydata(3, &query, 0), 42);
        assert_eq!(
            c.forwarder().forwarded.borrow().as_slice(),
            &[(0, appbar), (3, query)]
        );
        assert!(c.registry().icons().is_empty());
        assert!(!c.take_emit(10_000));
    }

    fn icon_query(msg: u32, hwnd: u32, uid: u32) -> Vec<u8> {
        let mut b = vec![0u8; 40];
        b[0..4].copy_from_slice(&0x3475_3423u32.to_le_bytes());
        b[4..8].copy_from_slice(&msg.to_le_bytes());
        b[8..12].copy_from_slice(&40u32.to_le_bytes());
        b[16..20].copy_from_slice(&hwnd.to_le_bytes());
        b[20..24].copy_from_slice(&uid.to_le_bytes());
        b
    }

    #[test]
    fn icon_rect_query_answers_from_last_known_rect() {
        let mut c = core();
        c.on_copydata(1, &fixture::discord_add(), 0);
        let key = c.registry().icons()[0].key.as_string();
        c.set_icon_rect(&key, Rect { x: 100, y: 900, w: 24, h: 24 });
        let tl = c.on_copydata(3, &icon_query(1, 0x10A2C, 7), 0);
        assert_eq!((tl as u32 & 0xFFFF, (tl as u32) >> 16), (100, 900));
        let br = c.on_copydata(3, &icon_query(2, 0x10A2C, 7), 0);
        assert_eq!((br as u32 & 0xFFFF, (br as u32) >> 16), (124, 924));
        assert_eq!(c.forwarder().forwarded.borrow().len(), 1, "only the NIM_ADD was forwarded");
    }

    #[test]
    fn taskbar_created_triggers_rebroadcast() {
        let mut c = core();
        c.start(0);
        assert_eq!(*c.forwarder().broadcasts.borrow(), 1);
        // Our own broadcast comes back to us: ignored.
        assert!(!c.on_taskbar_created(300));
        assert_eq!(*c.forwarder().broadcasts.borrow(), 1);
        // Explorer restarted later: rebroadcast.
        assert!(c.on_taskbar_created(5_000));
        assert_eq!(*c.forwarder().broadcasts.borrow(), 2);
        assert!(!c.on_taskbar_created(5_500));
        assert!(c.on_taskbar_created(9_000));
        assert_eq!(*c.forwarder().broadcasts.borrow(), 3);
    }

    #[test]
    fn changes_emit_once_per_debounce_window() {
        let mut c = core();
        let add = |c: &mut HostCore<FakeFwd>, uid: u32, now: u64| {
            let d = NotifyIconData { hwnd: 1, uid, flags: NIF_MESSAGE | NIF_ICON, ..Default::default() };
            c.on_copydata(1, &fixture::build(0, &d), now);
        };
        assert!(!c.take_emit(0));
        add(&mut c, 1, 0);
        add(&mut c, 2, 10);
        add(&mut c, 3, 50);
        assert_eq!(c.emit_due_in(50), Some(50));
        assert!(!c.take_emit(99));
        assert!(c.take_emit(100));
        assert!(!c.take_emit(150));
        assert_eq!(c.emit_due_in(150), None);
        add(&mut c, 4, 200);
        assert!(!c.take_emit(250));
        assert!(c.take_emit(300));
        // Prune removing icons is a change too.
        assert!(c.prune(|_| false, 400));
        assert!(c.take_emit(500));
        assert!(!c.prune(|_| false, 600));
        assert!(!c.take_emit(700));
    }

    #[test]
    fn views_bust_cache_when_png_changes() {
        let mut c = core();
        c.on_copydata(1, &fixture::discord_add(), 0);
        c.set_png(0, vec![1]);
        let v1 = c.views()[0].icon_url.clone();
        assert!(v1.starts_with("http://icon.localhost/tray-h"));
        assert!(v1.contains(".png?v="));
        c.set_png(0, vec![2]);
        assert_ne!(c.views()[0].icon_url, v1);
    }

    #[test]
    fn click_target_uses_live_registry_data() {
        let mut c = core();
        c.on_copydata(1, &fixture::discord_add(), 0);
        let key = c.registry().icons()[0].key.as_string();
        let t = c.click_target(&key).unwrap();
        assert_eq!((t.hwnd, t.uid, t.callback_message, t.version), (0x10A2C, 7, 0x8001, 0));
        assert!(c.click_target("nope").is_none());
    }

    #[test]
    fn icon_rgba_uses_mask_when_no_alpha() {
        // 2x1: color pixels without alpha; mask white (transparent) for the 2nd pixel.
        let color = [10, 20, 30, 0, 40, 50, 60, 0];
        let mask = [0, 0, 0, 0, 255, 255, 255, 0];
        assert_eq!(icon_rgba(Some(&color), &mask, 2, 1), vec![30, 20, 10, 255, 60, 50, 40, 0]);
        // With alpha: kept as is (straight alpha), mask ignored.
        let color = [10, 20, 30, 128, 40, 50, 60, 0];
        assert_eq!(icon_rgba(Some(&color), &mask, 2, 1), vec![30, 20, 10, 128, 60, 50, 40, 0]);
    }

    #[test]
    fn icon_rgba_monochrome_and_xor() {
        // 1x1 monochrome: mask is 2 rows (AND then XOR).
        let and_white_xor_black = [255, 255, 255, 0, 0, 0, 0, 0];
        assert_eq!(icon_rgba(None, &and_white_xor_black, 1, 1)[3], 0);
        let and_black_xor_white = [0, 0, 0, 0, 255, 255, 255, 0];
        assert_eq!(icon_rgba(None, &and_black_xor_white, 1, 1), vec![255, 255, 255, 255]);
    }

    #[test]
    fn parse_guid_string_to_memory_layout() {
        let g = parse_guid("{6D809377-6AF0-444B-8957-A3773F02200E}").unwrap();
        assert_eq!(g, [0x77, 0x93, 0x80, 0x6D, 0xF0, 0x6A, 0x4B, 0x44, 0x89, 0x57, 0xA3, 0x77, 0x3F, 0x02, 0x20, 0x0E]);
        assert!(parse_guid("nope").is_none());
    }

    #[test]
    fn known_folder_prefix_expanded() {
        let r = |g: &str| (g == "{6D809377-6AF0-444B-8957-A3773F02200E}").then(|| r"C:\Program Files".to_string());
        assert_eq!(
            expand_known_folder(r"{6D809377-6AF0-444B-8957-A3773F02200E}\Steam\steam.exe", r),
            r"C:\Program Files\Steam\steam.exe"
        );
        assert_eq!(expand_known_folder(r"C:\a\b.exe", r), r"C:\a\b.exe");
    }

    #[test]
    fn promoted_matches_guid_then_exe_and_uid() {
        let s = vec![
            NotifySetting { exe: Some(r"C:\Discord\Discord.exe".into()), uid: Some(7), guid: None, promoted: true },
            NotifySetting { exe: Some(r"C:\Steam\steam.exe".into()), uid: Some(1), guid: None, promoted: false },
            NotifySetting { exe: None, uid: None, guid: Some([5; 16]), promoted: true },
        ];
        assert!(is_promoted(&s, None, Some(r"c:\discord\discord.exe"), 7));
        assert!(!is_promoted(&s, None, Some(r"c:\discord\discord.exe"), 8));
        assert!(!is_promoted(&s, None, Some(r"C:\Steam\steam.exe"), 1));
        assert!(is_promoted(&s, Some([5; 16]), Some(r"C:\Steam\steam.exe"), 1));
        assert!(!is_promoted(&s, None, None, 0));
        assert!(!is_promoted(&[], None, Some("x"), 0));
    }
}
