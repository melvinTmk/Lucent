//! Grouped app switcher (`Alt+²`, macOS Cmd+Tab): one glass island centred on the cursor's
//! monitor, one icon per app that has windows, in most-recently-used order. Hold Alt and
//! press ² to cycle (Shift backwards), release Alt to switch; `←/→` move, `↓` shows the
//! selected app's windows as live thumbnails, `↑` hides them, Échap cancels, Entrée picks.
//!
//! Pure logic here (MRU, grouping, selection model, key edges); the window lives in `win`.
//! Keys are read with `GetAsyncKeyState` every 16 ms, only while the switcher is open (no
//! low-level hook). A quick Alt+² tap switches to the previous app without showing anything.
use crate::dock_model::DockItem;
use serde::Serialize;
use std::time::Duration;

/// Label of the switcher webview window.
pub const LABEL: &str = "switcher";

/// Logical px. One slot per app: the 64 px icon plus its selection plate.
pub const ICON: f32 = 64.0;
pub const CELL: f32 = 88.0;
/// Smallest slot when many apps must fit in `MAX_WIDTH_SHARE` of the monitor.
pub const MIN_CELL: f32 = 44.0;
/// Inner padding of the island.
pub const PAD: f32 = 14.0;
/// Room under the icons for the selected app's name.
pub const NAME_H: f32 = 30.0;
/// Corner radius of the island.
pub const RADIUS: f32 = 26.0;
/// The island never gets wider than this share of the monitor.
pub const MAX_WIDTH_SHARE: f32 = 0.9;
/// Window thumbnails row (`↓`): thumbnail height, gap, title strip.
pub const THUMB_H: f32 = 150.0;
pub const THUMB_GAP: f32 = 14.0;
pub const THUMB_TITLE_H: f32 = 24.0;

/// Key poll cadence while open.
pub const POLL: Duration = Duration::from_millis(16);
/// Alt released sooner than this after the first ²: switch without ever showing the island.
pub const SHOW_DELAY: Duration = Duration::from_millis(110);
/// The page did not confirm it rendered the new payload: show anyway after this.
pub const ACK_TIMEOUT: Duration = Duration::from_millis(150);
/// Hidden this long → the webview is destroyed (RAM budget), rebuilt on next use.
pub const IDLE_DESTROY: Duration = Duration::from_secs(60);
/// Foreground history length.
const MRU_CAP: usize = 64;
/// Sticky session (hot corner, menu): once shown, the foreground may be elsewhere this
/// long (activation race) before the session closes.
pub const STICKY_FOCUS_GRACE: Duration = Duration::from_millis(150);
/// Sticky session: closes after this long without any key (backstop).
pub const STICKY_IDLE: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SwitcherWindow {
    pub hwnd: isize,
    pub title: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SwitcherApp {
    /// Dock item id.
    pub id: String,
    pub name: String,
    pub icon_url: String,
    /// Most recent first.
    pub windows: Vec<SwitcherWindow>,
}

/// Foreground history, most recent first (fed by the wintrack foreground hook).
#[derive(Debug, Default, Clone)]
pub struct Mru {
    order: Vec<isize>,
}

impl Mru {
    /// `hwnd` just became the foreground window.
    pub fn touch(&mut self, hwnd: isize) {
        self.order.retain(|&h| h != hwnd);
        self.order.insert(0, hwnd);
        self.order.truncate(MRU_CAP);
    }

    pub fn history(&self) -> &[isize] {
        &self.order
    }
}

/// `windows` ordered most recent first: those seen in the foreground `history` first (most
/// recent first), then the others in `zorder` (front to back), then the rest as given.
pub fn mru_order(windows: &[isize], history: &[isize], zorder: &[isize]) -> Vec<isize> {
    let mut out: Vec<isize> = Vec::with_capacity(windows.len());
    for &h in history.iter().chain(zorder).chain(windows) {
        if windows.contains(&h) && !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// One entry per dock item that has windows, apps ordered by their most recent window,
/// each app's windows most recent first. Windows absent from `order` sort last.
pub fn group_apps(items: &[DockItem], order: &[isize]) -> Vec<SwitcherApp> {
    let rank = |h: isize| order.iter().position(|&o| o == h).unwrap_or(usize::MAX);
    let mut apps: Vec<(usize, SwitcherApp)> = items
        .iter()
        .filter(|i| !i.windows.is_empty())
        .map(|i| {
            let mut windows: Vec<SwitcherWindow> =
                i.windows.iter().map(|w| SwitcherWindow { hwnd: w.hwnd, title: w.title.clone() }).collect();
            windows.sort_by_key(|w| rank(w.hwnd));
            let best = windows.first().map_or(usize::MAX, |w| rank(w.hwnd));
            (best, SwitcherApp { id: i.id.clone(), name: i.name.clone(), icon_url: i.icon_url.clone(), windows })
        })
        .collect();
    // Stable: equal ranks keep the dock order.
    apps.sort_by_key(|(r, _)| *r);
    apps.into_iter().map(|(_, a)| a).collect()
}

/// First highlighted app. Forward: the previous app (index 1) when the front app leads the
/// list, else the first one. Backward (Shift): the last one.
pub fn initial_index(len: usize, back: bool, front_is_first: bool) -> usize {
    match len {
        0 => 0,
        _ if back => len - 1,
        1 => 0,
        _ if front_is_first => 1,
        _ => 0,
    }
}

/// Selection state while the switcher is open.
#[derive(Debug, Clone, PartialEq)]
pub struct SwitcherModel {
    pub apps: Vec<SwitcherApp>,
    pub index: usize,
    /// `Some(i)`: the selected app's windows are shown (`↓`), window `i` is selected.
    pub window: Option<usize>,
}

impl SwitcherModel {
    pub fn new(apps: Vec<SwitcherApp>, index: usize) -> Self {
        let index = if apps.is_empty() { 0 } else { index.min(apps.len() - 1) };
        Self { apps, index, window: None }
    }

    fn wrap(i: usize, delta: i32, len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        (i as i64 + delta as i64).rem_euclid(len as i64) as usize
    }

    /// Moves the app selection (wrapping) and leaves the windows row.
    pub fn step_app(&mut self, delta: i32) {
        self.window = None;
        self.index = Self::wrap(self.index, delta, self.apps.len());
    }

    /// `←/→`: moves among the shown windows when the row is open, else among apps.
    pub fn step(&mut self, delta: i32) {
        match self.window {
            Some(w) => {
                let n = self.apps.get(self.index).map_or(0, |a| a.windows.len());
                self.window = Some(Self::wrap(w, delta, n));
            }
            None => self.step_app(delta),
        }
    }

    /// `↓`: shows the selected app's windows. False when there is nothing to show (or
    /// already shown).
    pub fn expand(&mut self) -> bool {
        if self.window.is_some() || self.apps.get(self.index).is_none_or(|a| a.windows.is_empty()) {
            return false;
        }
        self.window = Some(0);
        true
    }

    /// `↑`: hides the windows row. False when it was not shown.
    pub fn collapse(&mut self) -> bool {
        self.window.take().is_some()
    }

    pub fn selected(&self) -> Option<&SwitcherApp> {
        self.apps.get(self.index)
    }

    /// What a pick activates: (app id, window): the selected window when the row is open,
    /// else the app's most recent window.
    pub fn pick(&self) -> Option<(String, isize)> {
        let app = self.selected()?;
        let w = app.windows.get(self.window.unwrap_or(0))?;
        Some((app.id.clone(), w.hwnd))
    }
}

/// Target of a `switcher_pick { appId, hwnd? }` from the page: `hwnd` when it belongs to
/// the app, else the app's most recent window.
pub fn pick_target(apps: &[SwitcherApp], app_id: &str, hwnd: Option<isize>) -> Option<isize> {
    let app = apps.iter().find(|a| a.id == app_id)?;
    match hwnd {
        Some(h) if app.windows.iter().any(|w| w.hwnd == h) => Some(h),
        _ => app.windows.first().map(|w| w.hwnd),
    }
}

/// Keys polled while the switcher is open (true = down).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Keys {
    pub alt: bool,
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    pub esc: bool,
    pub enter: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    /// Alt went up.
    Release,
    Left,
    Right,
    Up,
    Down,
    Esc,
    Enter,
}

/// Transitions between two polls: key presses (up → down) and the Alt release.
pub fn key_edges(prev: Keys, now: Keys) -> Vec<KeyEvent> {
    let mut out = Vec::new();
    let pressed = |a: bool, b: bool| !a && b;
    if pressed(prev.esc, now.esc) {
        out.push(KeyEvent::Esc);
    }
    if pressed(prev.left, now.left) {
        out.push(KeyEvent::Left);
    }
    if pressed(prev.right, now.right) {
        out.push(KeyEvent::Right);
    }
    if pressed(prev.up, now.up) {
        out.push(KeyEvent::Up);
    }
    if pressed(prev.down, now.down) {
        out.push(KeyEvent::Down);
    }
    if pressed(prev.enter, now.enter) {
        out.push(KeyEvent::Enter);
    }
    if prev.alt && !now.alt {
        out.push(KeyEvent::Release);
    }
    out
}

/// A key edge is acted on. Alt-held sessions own the keyboard (we took the foreground);
/// a sticky session only reacts while its window is the foreground, so keys typed in
/// another app (Entrée, arrows, Échap) are never stolen.
pub fn accepts_key(sticky: bool, own_foreground: bool) -> bool {
    !sticky || own_foreground
}

/// The session closes because the foreground moved. `fg`: current foreground root (0 =
/// none, e.g. during a switch); `shown_for`: time since the island was revealed (None =
/// not shown yet). Alt-held: any window other than ours / the one we opened over.
/// Sticky: anything but ours once the grace has passed (no `prev_fg` exemption: clicking
/// back into the app below must close it).
pub fn focus_lost(sticky: bool, fg: isize, own: isize, prev_fg: isize, shown_for: Option<Duration>) -> bool {
    let Some(shown_for) = shown_for else { return false };
    if fg == 0 || fg == own {
        return false;
    }
    if sticky { shown_for >= STICKY_FOCUS_GRACE } else { fg != prev_fg }
}

/// Sticky backstop: no key for `STICKY_IDLE`.
pub fn idle_expired(sticky: bool, since_input: Duration) -> bool {
    sticky && since_input >= STICKY_IDLE
}

/// Slot size (logical px) for `n` apps on a monitor `mon_w_logical` wide.
pub fn cell_for(n: usize, mon_w_logical: f32) -> f32 {
    if n == 0 {
        return CELL;
    }
    let max_inner = mon_w_logical * MAX_WIDTH_SHARE - 2.0 * PAD;
    (max_inner / n as f32).clamp(MIN_CELL, CELL)
}

/// Island size (logical px) without the windows row.
pub fn island_size(n: usize, cell: f32) -> (f32, f32) {
    (2.0 * PAD + n.max(1) as f32 * cell, PAD + cell + NAME_H + PAD / 2.0)
}

/// Payload of `switcher://show`.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ShowPayload {
    pub apps: Vec<SwitcherApp>,
    pub index: usize,
    /// Slot size, CSS px.
    pub cell: f32,
}

/// Payload of `switcher://windows` (null when the row closes).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WindowsPayload {
    pub app_index: usize,
    pub hwnds: Vec<isize>,
    pub titles: Vec<String>,
    /// Physical px, relative to the window.
    pub rects: Vec<crate::monitors::Rect>,
    pub index: usize,
    pub scale: f32,
    pub title_h: i32,
}

/// Switcher hotkey (or `run_action`): opens, or moves the selection when already open.
/// `sticky`: opened without Alt held (hot corner, menu): stays open until a pick or Échap.
/// Never blocks.
pub fn open(app: &tauri::AppHandle, back: bool, sticky: bool) {
    #[cfg(windows)]
    win::send(app, win::Msg::Open { back, sticky });
    #[cfg(not(windows))]
    let _ = (app, back, sticky);
}

/// Installs the hotkey handlers and the foreground history. From `run()` setup.
pub fn init(app: &tauri::AppHandle) {
    use crate::hotkey::{self, HotkeyId};
    let a = app.clone();
    hotkey::set_handler(HotkeyId::Switcher, move |_| open(&a, false, false));
    let a = app.clone();
    hotkey::set_handler(HotkeyId::SwitcherBack, move |_| open(&a, true, false));
    #[cfg(windows)]
    crate::wintrack::win::on_foreground(win::on_foreground);
}

/// Click on an app (or one of its window cards) in the page.
#[tauri::command]
pub fn switcher_pick(app: tauri::AppHandle, app_id: String, hwnd: Option<isize>) {
    #[cfg(windows)]
    win::send(&app, win::Msg::Pick { app_id, hwnd });
    #[cfg(not(windows))]
    let _ = (app, app_id, hwnd);
}

/// The page is loaded and listening, or has rendered the last `switcher://show`.
#[tauri::command]
pub fn switcher_ready(app: tauri::AppHandle) {
    #[cfg(windows)]
    win::send(&app, win::Msg::Ready);
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(windows)]
pub(crate) mod win {
    use super::*;
    use crate::dock_host::set_pos;
    use crate::monitors::{self, MonitorInfo, Rect};
    use crate::region;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    use tauri::{AppHandle, Emitter, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, TRUE};
    use windows::Win32::Graphics::Dwm::{
        DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_VISIBLE, DwmQueryThumbnailSourceSize,
        DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
    };
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VIRTUAL_KEY, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT, VK_UP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GA_ROOTOWNER, GWL_EXSTYLE, GetAncestor, GetCursorPos, GetForegroundWindow,
        GetWindowLongPtrW, GetWindowThreadProcessId, HWND_TOPMOST, IsWindow, SWP_HIDEWINDOW, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, SWP_SHOWWINDOW, SetForegroundWindow, SetWindowLongPtrW, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };
    use windows::core::BOOL;

    /// Accent tint used only when native glass is unavailable (same as the dock).
    const ACCENT_TINT: u32 = 0x10_80_80_80;

    static MRU: Mutex<Mru> = Mutex::new(Mru { order: Vec::new() });

    /// Foreground hook callback (hook thread): only touches the history.
    pub fn on_foreground(hwnd: isize) {
        let root = unsafe { GetAncestor(HWND(hwnd as *mut _), GA_ROOTOWNER) };
        let h = if root.0.is_null() { hwnd } else { root.0 as isize };
        MRU.lock().unwrap_or_else(|e| e.into_inner()).touch(h);
    }

    fn mru_history() -> Vec<isize> {
        MRU.lock().unwrap_or_else(|e| e.into_inner()).history().to_vec()
    }

    /// Top-level windows, front to back.
    pub(crate) fn zorder() -> Vec<isize> {
        unsafe extern "system" fn cb(hwnd: HWND, lp: LPARAM) -> BOOL {
            let out = unsafe { &mut *(lp.0 as *mut Vec<isize>) };
            out.push(hwnd.0 as isize);
            TRUE
        }
        let mut out: Vec<isize> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
        }
        out
    }

    pub(crate) fn foreground_root() -> isize {
        unsafe {
            let fg = GetForegroundWindow();
            let root = GetAncestor(fg, GA_ROOTOWNER);
            if root.0.is_null() { fg.0 as isize } else { root.0 as isize }
        }
    }

    pub(crate) fn cursor() -> Option<(i32, i32)> {
        let mut pt = POINT::default();
        unsafe { GetCursorPos(&mut pt) }.ok().map(|_| (pt.x, pt.y))
    }

    pub(crate) fn down(vk: VIRTUAL_KEY) -> bool {
        (unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000) != 0
    }

    pub(crate) fn esc_down() -> bool {
        down(VK_ESCAPE)
    }

    fn keys() -> Keys {
        Keys {
            alt: down(VK_MENU),
            left: down(VK_LEFT),
            right: down(VK_RIGHT),
            up: down(VK_UP),
            down: down(VK_DOWN),
            esc: down(VK_ESCAPE),
            enter: down(VK_RETURN),
        }
    }

    /// Out of Alt+Tab and the taskbar, but still activatable (unlike the dock).
    pub(crate) fn make_tool_window(hwnd: isize) {
        unsafe {
            let h = HWND(hwnd as *mut _);
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
            let want = (ex | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
            if want != ex {
                SetWindowLongPtrW(h, GWL_EXSTYLE, want);
            }
        }
    }

    /// Brings `hwnd` to the front (restoring it if minimised). We just handled our own
    /// hotkey or a click in our window, so Windows normally lets us; if it still refuses,
    /// attach to the foreground thread's input for the call (documented API, no injection).
    pub(crate) fn focus_window(hwnd: isize) {
        if unsafe { !IsWindow(Some(HWND(hwnd as *mut _))).as_bool() } {
            return;
        }
        crate::actions::focus(hwnd);
        if foreground_root() == hwnd {
            return;
        }
        unsafe {
            let h = HWND(hwnd as *mut _);
            let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
            let me = GetCurrentThreadId();
            let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
            let _ = BringWindowToTop(h);
            let ok = SetForegroundWindow(h).as_bool();
            if attached {
                let _ = AttachThreadInput(me, fg_thread, false);
            }
            if !ok {
                eprintln!("lucent: SetForegroundWindow refused for the picked window");
            }
        }
    }

    /// Activates one of our own windows from the thread owning it.
    pub(crate) fn activate_own(window: &WebviewWindow, hwnd: isize) {
        let _ = window.run_on_main_thread(move || unsafe {
            if !SetForegroundWindow(HWND(hwnd as *mut _)).as_bool() {
                eprintln!("lucent: SetForegroundWindow refused for an overlay");
            }
        });
    }

    pub enum Msg {
        Open { back: bool, sticky: bool },
        Pick { app_id: String, hwnd: Option<isize> },
        Ready,
    }

    static TX: OnceLock<Sender<Msg>> = OnceLock::new();

    /// Starts the worker on first use. Only pushes to a channel: never blocks.
    pub fn send(app: &AppHandle, msg: Msg) {
        let tx = TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            let app = app.clone();
            if let Err(e) = std::thread::Builder::new().name("lucent-switcher".into()).spawn(move || Host::new(app).run(rx)) {
                eprintln!("lucent: cannot spawn switcher thread: {e}");
            }
            tx
        });
        let _ = tx.send(msg);
    }

    struct Win {
        window: WebviewWindow,
        hwnd: isize,
        /// The page listens to our events.
        loaded: bool,
    }

    impl Win {
        fn hwnd(&self) -> HWND {
            HWND(self.hwnd as *mut _)
        }
    }

    struct Session {
        model: SwitcherModel,
        sticky: bool,
        keys: Keys,
        monitor: MonitorInfo,
        /// Not before this (quick taps switch without showing anything).
        reveal_at: Instant,
        /// `switcher://show` sent, waiting for the page's render ack (or `ACK_TIMEOUT`).
        awaiting: Option<Instant>,
        visible: bool,
        /// Island size without the windows row, physical px.
        base: (i32, i32),
        /// Current window rect.
        rect: Rect,
        /// Foreground window at open time (focus goes back there on Échap).
        prev_fg: isize,
        /// The switcher took the foreground.
        took_focus: bool,
        /// When the island was shown.
        shown_at: Option<Instant>,
        /// Last accepted key (or open time).
        last_input: Instant,
        thumbs: Vec<isize>,
    }

    struct Host {
        app: AppHandle,
        win: Option<Win>,
        session: Option<Session>,
        hidden_since: Option<Instant>,
        last_poll: Instant,
    }

    impl Host {
        fn new(app: AppHandle) -> Self {
            Self { app, win: None, session: None, hidden_since: None, last_poll: Instant::now() }
        }

        fn run(mut self, rx: Receiver<Msg>) {
            loop {
                let timeout = if self.session.is_some() {
                    Some(POLL)
                } else {
                    self.hidden_since.map(|t| (t + IDLE_DESTROY).saturating_duration_since(Instant::now()))
                };
                let msg = match timeout {
                    Some(t) => rx.recv_timeout(t),
                    None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match msg {
                    Ok(m) => self.handle(m),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                if self.session.is_none() || self.last_poll.elapsed() >= POLL {
                    self.tick();
                }
            }
        }

        fn handle(&mut self, m: Msg) {
            match m {
                Msg::Open { back, sticky } => match &mut self.session {
                    Some(s) => {
                        s.model.step_app(if back { -1 } else { 1 });
                        let collapsed = s.thumbs.len();
                        if collapsed > 0 {
                            self.show_windows();
                        }
                        self.emit_move();
                    }
                    None => self.open(back, sticky),
                },
                Msg::Pick { app_id, hwnd } => {
                    let target = self.session.as_ref().and_then(|s| pick_target(&s.model.apps, &app_id, hwnd));
                    if let Some(h) = target {
                        self.close(Some(h), false);
                    }
                }
                Msg::Ready => {
                    let Some(w) = &mut self.win else { return };
                    if !w.loaded {
                        w.loaded = true;
                    } else if self.session.as_ref().is_some_and(|s| s.awaiting.is_some()) {
                        self.reveal();
                    }
                }
            }
        }

        fn open(&mut self, back: bool, sticky: bool) {
            let g = crate::game::service::state();
            if g.active && g.focused {
                return;
            }
            let items = crate::current_items(&self.app);
            let all: Vec<isize> = items.iter().flat_map(|i| i.windows.iter().map(|w| w.hwnd)).collect();
            let order = mru_order(&all, &mru_history(), &zorder());
            let apps = group_apps(&items, &order);
            if apps.is_empty() {
                return;
            }
            let fg = foreground_root();
            let front_is_first = apps[0].windows.iter().any(|w| w.hwnd == fg);
            let index = initial_index(apps.len(), back, front_is_first);
            let all_mons = monitors::win::enumerate();
            let Some(monitor) = crate::search_window::monitor_at(&all_mons, cursor()).cloned() else { return };
            if self.win.is_none() {
                match self.create() {
                    Ok(w) => self.win = Some(w),
                    Err(e) => {
                        eprintln!("lucent: cannot create the switcher window: {e}");
                        return;
                    }
                }
            }
            self.hidden_since = None;
            let mut k = keys();
            // Opened by the hotkey: Alt is (or just was) down, a release already counts.
            k.alt = !sticky;
            let now = Instant::now();
            self.session = Some(Session {
                model: SwitcherModel::new(apps, index),
                sticky,
                keys: k,
                monitor,
                reveal_at: if sticky { now } else { now + SHOW_DELAY },
                awaiting: None,
                visible: false,
                base: (0, 0),
                rect: Rect::default(),
                prev_fg: fg,
                took_focus: false,
                shown_at: None,
                last_input: now,
                thumbs: Vec::new(),
            });
            self.last_poll = now - POLL;
        }

        fn tick(&mut self) {
            self.last_poll = Instant::now();
            let Some(s) = &mut self.session else {
                if self.hidden_since.is_some_and(|t| t.elapsed() >= IDLE_DESTROY) {
                    self.destroy();
                }
                return;
            };
            let now_keys = keys();
            let edges = key_edges(s.keys, now_keys);
            s.keys = now_keys;
            let own = self.win.as_ref().map_or(0, |w| w.hwnd);
            let own_fg = own != 0 && foreground_root() == own;
            if !edges.is_empty() && accepts_key(s.sticky, own_fg) {
                s.last_input = Instant::now();
            }
            for e in edges {
                let Some(s) = &mut self.session else { return };
                if e != KeyEvent::Release && !accepts_key(s.sticky, own_fg) {
                    continue;
                }
                match e {
                    KeyEvent::Release if !s.sticky => return self.pick_selected(),
                    KeyEvent::Release => {}
                    KeyEvent::Enter => return self.pick_selected(),
                    KeyEvent::Esc => return self.close(None, true),
                    KeyEvent::Left | KeyEvent::Right => {
                        s.model.step(if e == KeyEvent::Left { -1 } else { 1 });
                        if s.model.window.is_some() {
                            self.emit_windows();
                        } else {
                            self.emit_move();
                        }
                    }
                    KeyEvent::Down => {
                        // Asking for windows shows the island right away.
                        s.reveal_at = s.reveal_at.min(Instant::now());
                        if s.model.expand() && s.visible {
                            self.show_windows();
                        }
                    }
                    KeyEvent::Up => {
                        if s.model.collapse() && s.visible {
                            self.show_windows();
                        }
                    }
                }
            }
            let Some(s) = &self.session else { return };
            // Another window took the foreground (a click elsewhere, Win key): just close.
            let shown_for = s.shown_at.filter(|_| s.visible).map(|t| t.elapsed());
            if focus_lost(s.sticky, foreground_root(), own, s.prev_fg, shown_for) {
                return self.close(None, false);
            }
            if idle_expired(s.sticky, s.last_input.elapsed()) {
                return self.close(None, false);
            }
            let loaded = self.win.as_ref().is_some_and(|w| w.loaded);
            match s.awaiting {
                None if !s.visible && loaded && Instant::now() >= s.reveal_at => self.prepare(),
                Some(t) if t.elapsed() >= ACK_TIMEOUT => self.reveal(),
                _ => {}
            }
        }

        fn pick_selected(&mut self) {
            let target = self.session.as_ref().and_then(|s| s.model.pick()).map(|(_, h)| h);
            self.close(target, false);
        }

        /// Sizes the (hidden) window and sends the payload; shown on the page's ack.
        fn prepare(&mut self) {
            let (Some(w), Some(s)) = (&self.win, &mut self.session) else { return };
            let m = &s.monitor;
            let n = s.model.apps.len();
            let cell = cell_for(n, m.rect.w as f32 / m.scale);
            let (lw, lh) = island_size(n, cell);
            s.base = ((lw * m.scale).round() as i32, (lh * m.scale).round() as i32);
            s.rect = centred(m, s.base);
            for _ in 0..2 {
                set_pos(w.hwnd(), Some(HWND_TOPMOST), s.rect, SWP_HIDEWINDOW);
            }
            let payload = ShowPayload { apps: s.model.apps.clone(), index: s.model.index, cell };
            s.awaiting = Some(Instant::now());
            if let Err(e) = self.app.emit_to(LABEL, "switcher://show", payload) {
                eprintln!("lucent: cannot emit switcher://show: {e}");
            }
        }

        fn reveal(&mut self) {
            let (Some(w), Some(s)) = (&self.win, &mut self.session) else { return };
            s.awaiting = None;
            s.visible = true;
            s.shown_at = Some(Instant::now());
            for _ in 0..2 {
                set_pos(w.hwnd(), Some(HWND_TOPMOST), s.rect, SWP_SHOWWINDOW);
            }
            apply_shape(&w.window, w.hwnd, s.rect.w, s.rect.h, s.monitor.scale);
            // Keys pressed while Alt is held must reach us, not the app below (Alt+← is
            // "back" in a browser): take the foreground, as the native Alt+Tab does.
            activate_own(&w.window, w.hwnd);
            s.took_focus = true;
            if s.model.window.is_some() {
                self.show_windows();
            }
        }

        fn emit_move(&self) {
            let Some(s) = &self.session else { return };
            if !s.visible && s.awaiting.is_none() {
                return;
            }
            if let Err(e) = self.app.emit_to(LABEL, "switcher://move", serde_json::json!({ "index": s.model.index })) {
                eprintln!("lucent: cannot emit switcher://move: {e}");
            }
        }

        fn unregister(&mut self) {
            if let Some(s) = &mut self.session {
                for t in s.thumbs.drain(..) {
                    unsafe {
                        let _ = DwmUnregisterThumbnail(t);
                    }
                }
            }
        }

        /// Opens/refreshes/closes the windows row to match `model.window`.
        fn show_windows(&mut self) {
            self.unregister();
            let (Some(w), Some(s)) = (&self.win, &mut self.session) else { return };
            let sc = s.monitor.scale;
            let px = |v: f32| (v * sc).round() as i32;
            let Some(app) = s.model.selected().cloned().filter(|_| s.model.window.is_some()) else {
                s.rect = centred(&s.monitor, s.base);
                set_pos(w.hwnd(), Some(HWND_TOPMOST), s.rect, SWP_SHOWWINDOW);
                apply_shape(&w.window, w.hwnd, s.rect.w, s.rect.h, sc);
                if let Err(e) = self.app.emit_to(LABEL, "switcher://windows", ()) {
                    eprintln!("lucent: cannot emit switcher://windows: {e}");
                }
                return;
            };
            let mut regs: Vec<(isize, &SwitcherWindow, (i32, i32))> = Vec::new();
            for win in &app.windows {
                let Ok(t) = (unsafe { DwmRegisterThumbnail(w.hwnd(), HWND(win.hwnd as *mut _)) }) else { continue };
                let size = unsafe { DwmQueryThumbnailSourceSize(t) }.map_or((0, 0), |s| (s.cx, s.cy));
                regs.push((t, win, size));
            }
            s.thumbs = regs.iter().map(|r| r.0).collect();
            let sizes: Vec<(i32, i32)> = regs.iter().map(|r| r.2).collect();
            let max_w = (s.monitor.rect.w as f32 * MAX_WIDTH_SHARE) as i32;
            let (row, mut rects) =
                crate::thumbs::layout_thumbs(&sizes, max_w, px(THUMB_H), px(THUMB_GAP), px(THUMB_TITLE_H));
            let width = s.base.0.max(row.w);
            // The row's own top margin overlaps the island's bottom padding.
            let top = s.base.1 - px(THUMB_GAP);
            let dx = (width - row.w) / 2;
            for r in &mut rects {
                r.x += dx;
                r.y += top;
            }
            s.rect = centred(&s.monitor, (width, top + row.h));
            set_pos(w.hwnd(), Some(HWND_TOPMOST), s.rect, SWP_SHOWWINDOW);
            apply_shape(&w.window, w.hwnd, s.rect.w, s.rect.h, sc);
            for (r, dest) in regs.iter().zip(&rects) {
                let props = DWM_THUMBNAIL_PROPERTIES {
                    dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY,
                    rcDestination: RECT { left: dest.x, top: dest.y, right: dest.x + dest.w, bottom: dest.y + dest.h },
                    opacity: 255,
                    fVisible: true.into(),
                    ..Default::default()
                };
                unsafe {
                    let _ = DwmUpdateThumbnailProperties(r.0, &props);
                }
            }
            let payload = WindowsPayload {
                app_index: s.model.index,
                hwnds: regs.iter().map(|r| r.1.hwnd).collect(),
                titles: regs.iter().map(|r| r.1.title.clone()).collect(),
                rects,
                index: s.model.window.unwrap_or(0),
                scale: sc,
                title_h: px(THUMB_TITLE_H),
            };
            if let Err(e) = self.app.emit_to(LABEL, "switcher://windows", payload) {
                eprintln!("lucent: cannot emit switcher://windows: {e}");
            }
        }

        /// Selection moved inside the windows row: only the highlight changes.
        fn emit_windows(&self) {
            let Some(s) = &self.session else { return };
            if let Err(e) =
                self.app.emit_to(LABEL, "switcher://window-index", serde_json::json!({ "index": s.model.window.unwrap_or(0) }))
            {
                eprintln!("lucent: cannot emit switcher://window-index: {e}");
            }
        }

        /// Ends the session: activates `target` (else, with `restore`, gives the focus back
        /// to the window that had it), hides.
        fn close(&mut self, target: Option<isize>, restore: bool) {
            self.unregister();
            let Some(s) = self.session.take() else { return };
            match target {
                Some(h) => focus_window(h),
                None if restore && s.took_focus && s.prev_fg != 0 => focus_window(s.prev_fg),
                None => {}
            }
            if let Some(w) = &self.win {
                set_pos(w.hwnd(), None, Rect::default(), SWP_NOZORDER | SWP_NOMOVE | SWP_NOSIZE | SWP_HIDEWINDOW);
            }
            if s.visible || s.awaiting.is_some() {
                if let Err(e) = self.app.emit_to(LABEL, "switcher://hide", ()) {
                    eprintln!("lucent: cannot emit switcher://hide: {e}");
                }
            }
            self.hidden_since = Some(Instant::now());
        }

        fn destroy(&mut self) {
            self.hidden_since = None;
            let Some(w) = self.win.take() else { return };
            let hwnd = w.hwnd;
            let _ = w.window.run_on_main_thread(move || crate::backdrop::detach(hwnd));
            if let Err(e) = w.window.destroy() {
                eprintln!("lucent: cannot destroy the switcher window: {e}");
            }
        }

        fn create(&self) -> tauri::Result<Win> {
            let window = WebviewWindowBuilder::new(&self.app, LABEL, WebviewUrl::App("switcher.html".into()))
                .title("Lucent switcher")
                .inner_size(CELL as f64, CELL as f64)
                .transparent(true)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .focusable(true)
                .focused(false)
                .shadow(false)
                .resizable(false)
                .visible(false)
                .build()?;
            let hwnd = window.hwnd()?.0 as isize;
            make_tool_window(hwnd);
            let _ = window.run_on_main_thread(move || crate::backdrop::attach_or_accent(hwnd, ACCENT_TINT));
            Ok(Win { window, hwnd, loaded: false })
        }
    }

    /// `size` centred on the monitor (both axes).
    fn centred(m: &MonitorInfo, size: (i32, i32)) -> Rect {
        let (w, h) = (size.0.min(m.rect.w), size.1.min(m.rect.h));
        Rect { x: m.rect.x + (m.rect.w - w) / 2, y: m.rect.y + (m.rect.h - h) / 2, w, h }
    }

    /// One glass island over the whole window (as the search window).
    fn apply_shape(window: &WebviewWindow, hwnd: isize, w: i32, h: i32, scale: f32) {
        let island = Rect { x: 0, y: 0, w, h };
        let pad = (region::REGION_PAD * scale).round() as i32;
        let radius = RADIUS * scale;
        region::apply(HWND(hwnd as *mut _), &[region::grown(island, pad)], radius.round() as i32 + pad);
        let _ = window.run_on_main_thread(move || {
            if let Err(e) = crate::backdrop::set_islands(hwnd, &[island], radius) {
                eprintln!("lucent: cannot reshape the switcher glass: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock_model::WindowRef;
    use crate::settings::AppId;

    fn item(id: &str, hwnds: &[isize]) -> DockItem {
        DockItem {
            id: id.into(),
            app: AppId::Exe(format!("c:\\{id}.exe")),
            name: id.to_uppercase(),
            icon_url: format!("icon://{id}"),
            pinned: false,
            is_game: false,
            windows: hwnds.iter().map(|&h| WindowRef { hwnd: h, title: format!("w{h}") }).collect(),
            badge: None,
        }
    }

    fn app(id: &str, hwnds: &[isize]) -> SwitcherApp {
        SwitcherApp {
            id: id.into(),
            name: id.into(),
            icon_url: String::new(),
            windows: hwnds.iter().map(|&h| SwitcherWindow { hwnd: h, title: String::new() }).collect(),
        }
    }

    #[test]
    fn mru_touch_moves_to_front_without_duplicates() {
        let mut m = Mru::default();
        for h in [1, 2, 3, 2] {
            m.touch(h);
        }
        assert_eq!(m.history(), &[2, 3, 1]);
    }

    #[test]
    fn mru_is_capped() {
        let mut m = Mru::default();
        for h in 0..200 {
            m.touch(h);
        }
        assert_eq!(m.history().len(), 64);
        assert_eq!(m.history()[0], 199);
    }

    #[test]
    fn mru_order_history_then_zorder_then_rest() {
        // 9 was focused but is gone; 5 never focused but in z-order; 7 unknown everywhere.
        let order = mru_order(&[4, 5, 6, 7], &[6, 9, 4], &[1, 5, 4, 6]);
        assert_eq!(order, vec![6, 4, 5, 7]);
    }

    #[test]
    fn group_apps_orders_by_most_recent_window() {
        let items = [item("code", &[10, 11]), item("idle", &[]), item("firefox", &[20]), item("term", &[30])];
        let apps = group_apps(&items, &[20, 11, 30, 10]);
        let ids: Vec<&str> = apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["firefox", "code", "term"], "apps without windows are skipped");
        let code: Vec<isize> = apps[1].windows.iter().map(|w| w.hwnd).collect();
        assert_eq!(code, vec![11, 10], "an app's windows are most recent first");
        assert_eq!(apps[0].name, "FIREFOX");
        assert_eq!(apps[0].icon_url, "icon://firefox");
    }

    #[test]
    fn group_apps_unknown_windows_keep_dock_order_last() {
        let items = [item("a", &[1]), item("b", &[2]), item("c", &[3])];
        let apps = group_apps(&items, &[3]);
        let ids: Vec<&str> = apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["c", "a", "b"]);
    }

    #[test]
    fn initial_index_is_previous_app() {
        assert_eq!(initial_index(3, false, true), 1);
        assert_eq!(initial_index(3, false, false), 0, "front is not an app (desktop): most recent app");
        assert_eq!(initial_index(1, false, true), 0);
        assert_eq!(initial_index(3, true, true), 2, "Shift starts from the end");
        assert_eq!(initial_index(0, false, true), 0);
    }

    #[test]
    fn model_steps_wrap_both_ways() {
        let mut m = SwitcherModel::new(vec![app("a", &[1]), app("b", &[2]), app("c", &[3])], 1);
        m.step_app(1);
        assert_eq!(m.index, 2);
        m.step_app(1);
        assert_eq!(m.index, 0, "wraps forward");
        m.step_app(-1);
        assert_eq!(m.index, 2, "wraps backward");
        m.step(-1);
        assert_eq!(m.index, 1);
    }

    #[test]
    fn model_clamps_initial_index_and_handles_empty() {
        assert_eq!(SwitcherModel::new(vec![app("a", &[1])], 5).index, 0);
        let mut e = SwitcherModel::new(Vec::new(), 3);
        e.step(1);
        assert_eq!(e.index, 0);
        assert!(!e.expand());
        assert_eq!(e.pick(), None);
    }

    #[test]
    fn model_pick_targets_most_recent_window() {
        let m = SwitcherModel::new(vec![app("a", &[1]), app("b", &[21, 22])], 1);
        assert_eq!(m.pick(), Some(("b".into(), 21)));
    }

    #[test]
    fn model_windows_row_moves_and_picks_a_specific_window() {
        let mut m = SwitcherModel::new(vec![app("a", &[1]), app("b", &[21, 22, 23])], 1);
        assert!(m.expand());
        assert!(!m.expand(), "already open");
        m.step(1);
        m.step(1);
        assert_eq!(m.window, Some(2));
        m.step(1);
        assert_eq!(m.window, Some(0), "wraps inside the row");
        m.step(-1);
        assert_eq!(m.pick(), Some(("b".into(), 23)));
        assert_eq!(m.index, 1, "app selection unchanged");
        assert!(m.collapse());
        assert!(!m.collapse());
        assert_eq!(m.pick(), Some(("b".into(), 21)));
    }

    #[test]
    fn model_hotkey_step_leaves_windows_row() {
        let mut m = SwitcherModel::new(vec![app("a", &[1]), app("b", &[21, 22])], 1);
        m.expand();
        m.step_app(1);
        assert_eq!((m.index, m.window), (0, None));
    }

    #[test]
    fn pick_target_prefers_the_given_window_of_that_app() {
        let apps = vec![app("a", &[1, 2]), app("b", &[3])];
        assert_eq!(pick_target(&apps, "a", Some(2)), Some(2));
        assert_eq!(pick_target(&apps, "a", None), Some(1));
        assert_eq!(pick_target(&apps, "a", Some(3)), Some(1), "window of another app is ignored");
        assert_eq!(pick_target(&apps, "zz", None), None);
    }

    #[test]
    fn key_edges_detect_presses_and_alt_release() {
        let idle = Keys { alt: true, ..Keys::default() };
        assert_eq!(key_edges(idle, idle), vec![]);
        let right = Keys { right: true, ..idle };
        assert_eq!(key_edges(idle, right), vec![KeyEvent::Right]);
        assert_eq!(key_edges(right, right), vec![], "held key fires once");
        assert_eq!(key_edges(right, Keys::default()), vec![KeyEvent::Release]);
        let all = Keys { alt: true, left: true, up: true, down: true, esc: true, enter: true, right: false };
        assert_eq!(
            key_edges(idle, all),
            vec![KeyEvent::Esc, KeyEvent::Left, KeyEvent::Up, KeyEvent::Down, KeyEvent::Enter]
        );
    }

    #[test]
    fn sticky_session_acts_on_keys_only_when_foreground() {
        assert!(accepts_key(false, false), "Alt-held session owns the keyboard");
        assert!(accepts_key(true, true));
        assert!(!accepts_key(true, false), "keys typed in another app are not stolen");
    }

    #[test]
    fn focus_lost_alt_held_exempts_previous_window() {
        let shown = Some(Duration::from_millis(500));
        assert!(!focus_lost(false, 5, 9, 5, shown), "window we opened over");
        assert!(!focus_lost(false, 9, 9, 5, shown));
        assert!(!focus_lost(false, 0, 9, 5, shown));
        assert!(focus_lost(false, 7, 9, 5, shown));
        assert!(!focus_lost(false, 7, 9, 5, None), "not shown yet");
    }

    #[test]
    fn focus_lost_sticky_closes_after_grace_even_on_previous_window() {
        assert!(!focus_lost(true, 5, 9, 5, Some(Duration::from_millis(100))), "grace");
        assert!(focus_lost(true, 5, 9, 5, Some(STICKY_FOCUS_GRACE)), "no prev_fg exemption");
        assert!(focus_lost(true, 7, 9, 5, Some(Duration::from_secs(1))));
        assert!(!focus_lost(true, 9, 9, 5, Some(Duration::from_secs(1))));
        assert!(!focus_lost(true, 0, 9, 5, Some(Duration::from_secs(1))));
        assert!(!focus_lost(true, 7, 9, 5, None));
    }

    #[test]
    fn idle_backstop_only_for_sticky() {
        assert!(!idle_expired(true, Duration::from_secs(9)));
        assert!(idle_expired(true, STICKY_IDLE));
        assert!(!idle_expired(false, Duration::from_secs(60)));
    }

    #[test]
    fn cell_shrinks_to_fit_the_monitor() {
        assert_eq!(cell_for(5, 1920.0), CELL);
        let c = cell_for(30, 1920.0);
        assert!(c < CELL && c >= MIN_CELL);
        let (w, _) = island_size(30, c);
        assert!(w <= 1920.0 * MAX_WIDTH_SHARE + 0.5, "{w}");
        assert_eq!(cell_for(500, 1920.0), MIN_CELL, "never below the minimum");
    }

    #[test]
    fn island_size_counts_padding_and_name_row() {
        let (w, h) = island_size(3, CELL);
        assert_eq!(w, 2.0 * PAD + 3.0 * CELL);
        assert_eq!(h, PAD + CELL + NAME_H + PAD / 2.0);
    }
}
