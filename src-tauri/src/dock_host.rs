//! `DockHost`: one dock window per selected monitor, the cursor poller that reveals and
//! hides them, and the geometry/region updates from `report_dock_layout`.
//!
//! Everything runs on one worker thread fed by a channel, never on the main thread:
//! - building a webview window from the main thread (sync command, event handler)
//!   deadlocks on Windows;
//! - `SetWindowPos` on a window owned by the main thread sends it a message, so the main
//!   thread must never wait on this thread (it only ever pushes to the channel).
#![cfg(windows)]
use crate::game::session::GameStateDto;
use crate::monitors::{self, MonitorInfo, Park, Rect};
use crate::glass;
use crate::region;
use crate::thumbs::win::{ShowReq, ThumbsWindow};
use crate::reveal::{self, RevealAction, RevealInput, RevealState};
use crate::settings::{DockPosition, MonitorSelection, Settings};
use serde::Serialize;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetCursorPos, GetWindowLongPtrW, HWND_TOPMOST, MA_NOACTIVATE, SET_WINDOW_POS_FLAGS,
    SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowLongPtrW,
    WM_MOUSEACTIVATE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// Poll cadence while every dock is hidden (perf budget).
const POLL_HIDDEN: Duration = Duration::from_millis(50);
/// Poll cadence while a dock is shown, arming, disarming or sliding out.
const POLL_SHOWN: Duration = Duration::from_millis(16);
/// Monitor changes arrive in bursts and settle late: reconcile once things are quiet.
const RECONCILE_DEBOUNCE: Duration = Duration::from_millis(250);
/// Slide duration at speed 1.0 (Task 7 animates it in CSS); see `slide_ms`.
const SLIDE_MS: f32 = 240.0;
/// Content size used until the dock page reports its layout (logical px).
const DEFAULT_CONTENT: (f32, f32) = (640.0, 96.0);

pub enum Msg {
    Settings(Box<Settings>),
    /// Explorer restarted: its AppBar registrations are gone.
    ShellRestarted,
    /// `islands`: glass shapes; `hit`: extra areas the window region covers (magnified icons);
    /// `pill`: the page shows only the Game Mode system pill (and its panel).
    Layout { monitor_id: String, width: u32, height: u32, islands: Vec<Rect>, hit: Vec<Rect>, pill: bool },
    /// Monitors may have changed (WM_DISPLAYCHANGE, WM_DPICHANGED).
    Reconcile,
    /// Open the thumbnails popup for `item_id` above the dock of `monitor_id` (already
    /// open for that item: kept as is).
    ShowThumbs { item_id: String, monitor_id: String, windows: Vec<(isize, String)> },
    /// Same, from the pointer resting on the icon (the dock page's hover machine): ignored
    /// in Game Mode or while that dock is not fully shown (`hover_allowed`).
    HoverThumbs { item_id: String, monitor_id: String, windows: Vec<(isize, String)> },
    CloseThumbs,
    /// The thumbnails page has loaded (it listens to `thumbs://show`).
    ThumbsReady,
    /// Game Mode state changed (from the game session service).
    Game(GameStateDto),
    /// Force-show the dock on `monitor_id` for `duration_ms` (e.g. for post-match / ask card).
    Peek { monitor_id: String, duration_ms: u64 },
    /// An OS file drag entered / left the dock window `label`.
    DragOver { label: String, over: bool },
}

/// The dock of `monitor_id` must stay hidden: a fullscreen app covers that monitor, or
/// the focused game runs on it. A game in a window on another monitor leaves it alone.
pub fn suppressed(fullscreen_covers_monitor: bool, game: &GameStateDto, monitor_id: &str) -> bool {
    fullscreen_covers_monitor || game.owns(monitor_id)
}

/// While a game runs, the docks of the *other* monitors use static glass (no live blur)
/// and skip their animations.
pub fn static_glass(game: &GameStateDto, monitor_id: &str) -> bool {
    game.active && game.monitor_id.as_deref() != Some(monitor_id)
}

/// A hover preview may open over a dock in `state`: never in Game Mode (any monitor) nor
/// while the dock is hidden, sliding in or about to hide.
pub fn hover_allowed(game: &GameStateDto, state: RevealState) -> bool {
    !game.active && state == RevealState::Shown
}

/// Game Mode system pill on `monitor_id`: a game runs on another monitor, the setting is
/// on and this dock is not revealed (once revealed, the dock takes over).
pub fn show_pill(game: &GameStateDto, monitor_pill: bool, monitor_id: &str, revealed: bool) -> bool {
    monitor_pill && !revealed && static_glass(game, monitor_id)
}

/// A hidden dock window stays on-screen, clipped to the pill, instead of being parked past
/// the edge. Only once the page reports its pill layout: before that its region would be
/// the (empty, slid-away) dock islands.
fn pill_on_screen(game: &GameStateDto, monitor_pill: bool, monitor_id: &str, state: RevealState, layout_pill: bool) -> bool {
    let revealed = matches!(state, RevealState::Shown | RevealState::Disarming { .. });
    layout_pill && show_pill(game, monitor_pill, monitor_id, revealed)
}

#[derive(Serialize, Clone)]
struct RevealPayload {
    shown: bool,
}

static TX: OnceLock<Sender<Msg>> = OnceLock::new();

/// Accent tint under the CSS glass (0xAABBGGRR): barely there, the CSS fill sets the tone.
const ACCENT_TINT: u32 = 0x10_80_80_80;

pub fn send(msg: Msg) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(msg);
    }
}

pub fn peek(monitor_id: String, duration_ms: u64) {
    send(Msg::Peek { monitor_id, duration_ms });
}

/// `fn()` callback for the shell listener's `WM_DISPLAYCHANGE`.
pub fn request_reconcile() {
    send(Msg::Reconcile);
}

/// Starts the worker thread; the first reconcile creates the dock windows.
/// `on_ready(true)` fires once a dock page has reported its first layout (a dock is really
/// up), `on_ready(false)` when a reconcile ends with no dock window: the taskbar may only
/// be hidden in between.
pub fn start(app: &AppHandle, settings: Settings, on_ready: fn(bool)) {
    let (tx, rx) = mpsc::channel();
    if TX.set(tx).is_err() {
        return;
    }
    let app = app.clone();
    std::thread::Builder::new()
        .name("lucent-dock-host".into())
        .spawn(move || DockHost::new(app, settings, on_ready).run(rx))
        .expect("spawn dock host thread");
    request_reconcile();
}

#[derive(Clone, PartialEq, Default)]
struct Layout {
    w: u32,
    h: u32,
    islands: Vec<Rect>,
    hit: Vec<Rect>,
    /// The page shows only the system pill (see `Msg::Layout`).
    pill: bool,
}

/// Gap kept between the reserved strip and the dock's top (physical px).
const APPBAR_GAP: i32 = 6;

/// The strip of `m` a pinned dock reserves: from the screen edge to its row of islands
/// (open popups, which sit above the row, are ignored), plus a small gap.
fn reserved_strip(m: &MonitorInfo, pos: DockPosition, shown: Rect, islands: &[Rect]) -> Option<Rect> {
    let isl: Vec<&Rect> = islands.iter().filter(|r| r.w > 0 && r.h > 0).collect();
    if isl.is_empty() {
        return None;
    }
    let r = m.rect;
    Some(match pos {
        DockPosition::Bottom => {
            let base = isl.iter().map(|i| i.y + i.h).max()?;
            let top = shown.y + isl.iter().filter(|i| i.y + i.h >= base - 2).map(|i| i.y).min()? - APPBAR_GAP;
            let top = top.clamp(r.y, r.y + r.h);
            Rect { x: r.x, y: top, w: r.w, h: r.y + r.h - top }
        }
        DockPosition::Left => {
            let base = isl.iter().map(|i| i.x).min()?;
            let right = shown.x + isl.iter().filter(|i| i.x <= base + 2).map(|i| i.x + i.w).max()? + APPBAR_GAP;
            let right = right.clamp(r.x, r.x + r.w);
            Rect { x: r.x, y: r.y, w: right - r.x, h: r.h }
        }
        DockPosition::Right => {
            let base = isl.iter().map(|i| i.x + i.w).max()?;
            let left = shown.x + isl.iter().filter(|i| i.x + i.w >= base - 2).map(|i| i.x).min()? - APPBAR_GAP;
            let left = left.clamp(r.x, r.x + r.w);
            Rect { x: left, y: r.y, w: r.x + r.w - left, h: r.h }
        }
    })
}

/// Windows AppBar registration of a dock window (the taskbar's own mechanism to keep
/// maximized windows off a screen strip).
mod appbar {
    use super::{DockPosition, Rect};
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::Shell::{
        ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABM_NEW, ABM_REMOVE, ABM_SETPOS, APPBARDATA, SHAppBarMessage,
    };
    use windows::Win32::UI::WindowsAndMessaging::WM_APP;

    fn data(hwnd: isize) -> APPBARDATA {
        APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: HWND(hwnd as *mut _), ..Default::default() }
    }

    /// Applies `want` (None = free the strip). `current` tracks what is registered.
    pub fn set(hwnd: isize, current: &mut Option<Rect>, want: Option<Rect>, pos: DockPosition) {
        if *current == want {
            return;
        }
        let Some(r) = want else {
            let mut d = data(hwnd);
            unsafe { SHAppBarMessage(ABM_REMOVE, &mut d) };
            *current = None;
            return;
        };
        if current.is_none() {
            let mut d = data(hwnd);
            d.uCallbackMessage = WM_APP + 0x47;
            // 0 also means "already registered" (we forget ours on a TaskbarCreated
            // broadcast that was not a real Explorer restart): carry on with SETPOS.
            let _ = unsafe { SHAppBarMessage(ABM_NEW, &mut d) };
        }
        let mut d = data(hwnd);
        d.uEdge = match pos {
            DockPosition::Bottom => ABE_BOTTOM,
            DockPosition::Left => ABE_LEFT,
            DockPosition::Right => ABE_RIGHT,
        };
        d.rc = RECT { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h };
        // No ABM_QUERYPOS: it moves the strip off Explorer's (hidden, autohide) secondary
        // taskbars, which stacked their 48 px on top of ours. The dock sits at the edge.
        unsafe { SHAppBarMessage(ABM_SETPOS, &mut d) };
        *current = Some(r);
    }
}

/// Slack around the shapes for "cursor in dock" (physical px): gaps between islands and a
/// pointer grazing an edge must not start the hide timer.
const SHAPE_SLACK: i32 = 6;

/// The part of the dock window the cursor counts as "in the dock": the bounding box of the
/// islands and hit rects (open popups included, they are islands), in screen px. The window
/// itself is much taller (headroom kept so popups never resize it), and resting the
/// pointer in that transparent, click-through band must not hold the dock up.
fn dock_shape_box(shown: Rect, islands: &[Rect], hit: &[Rect]) -> Rect {
    let mut it = islands.iter().chain(hit).filter(|r| r.w > 0 && r.h > 0);
    let Some(first) = it.next() else { return shown };
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x + first.w, first.y + first.h);
    for r in it {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.x + r.w);
        y1 = y1.max(r.y + r.h);
    }
    Rect {
        x: shown.x + x0 - SHAPE_SLACK,
        y: shown.y + y0 - SHAPE_SLACK,
        w: x1 - x0 + 2 * SHAPE_SLACK,
        h: y1 - y0 + 2 * SHAPE_SLACK,
    }
}

struct Dock {
    label: String,
    window: WebviewWindow,
    hwnd: isize,
    monitor: MonitorInfo,
    layout: Layout,
    state: RevealState,
    /// Window currently at its shown rect (vs parked past the edge).
    on_screen: bool,
    /// When to park the window after a hide (the front slides out first).
    park_at: Option<Instant>,
    /// A slide (in or out) runs until then: the region also covers the path to the edge.
    slide_end: Option<Instant>,
    /// The page has reported its layout at least once (it listens to `dock://tone`).
    page_ready: bool,
    /// Shown rect last sent to the glass worker (tone recomputed when it moves).
    tone_rect: Option<Rect>,
    /// Static glass applied, with its tone (`Some(dark)`); `None`: live glass.
    static_glass: Option<bool>,
    /// Hidden but kept on-screen showing only the system pill (`pill_on_screen`).
    pill: bool,
    /// Force-shown for card peek until this instant.
    peek_until: Option<Instant>,
    /// An OS file drag hovers this dock window (reveal treats it as the cursor in the dock).
    drag_over: bool,
    /// Screen strip reserved through the AppBar API (« Toujours visible »).
    appbar: Option<Rect>,
}

impl Dock {
    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }
}

pub struct DockHost {
    app: AppHandle,
    settings: Settings,
    /// Every attached monitor (to avoid parking a dock onto a neighbour).
    all: Vec<MonitorInfo>,
    /// Monitors that currently have a dock.
    monitors: Vec<MonitorInfo>,
    docks: Vec<Dock>,
    next_n: u32,
    epoch: Instant,
    next_tick: Instant,
    reconcile_at: Option<Instant>,
    /// Built on the first thumbnails request (RAM budget), then kept.
    thumbs: Option<ThumbsWindow>,
    /// The thumbnails page is loaded; until then a request waits in `thumbs_pending`.
    thumbs_ready: bool,
    thumbs_pending: Option<(String, String, Vec<(isize, String)>)>,
    on_ready: fn(bool),
    /// Last value given to `on_ready`.
    ready: bool,
    /// Previous Escape / mouse-button state, for edge detection while the popup is open.
    prev_esc: bool,
    prev_click: bool,
    game: GameStateDto,
    /// A sysmon hold is taken (some dock shows its pill).
    sysmon_held: bool,
    /// Hot corners dwell state (M6), fed by this poll.
    corners: crate::corners::Dwell,
}

impl DockHost {
    fn new(app: AppHandle, settings: Settings, on_ready: fn(bool)) -> Self {
        let now = Instant::now();
        Self {
            app,
            settings,
            all: Vec::new(),
            monitors: Vec::new(),
            docks: Vec::new(),
            next_n: 1,
            epoch: now,
            next_tick: now,
            reconcile_at: None,
            thumbs: None,
            thumbs_ready: false,
            thumbs_pending: None,
            on_ready,
            ready: false,
            prev_esc: false,
            prev_click: false,
            // Read after `TX` is set: a later change arrives as `Msg::Game`.
            game: crate::game::service::state(),
            sysmon_held: false,
            corners: crate::corners::Dwell::default(),
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let now = Instant::now();
            let mut wake = self.next_tick;
            if let Some(t) = self.reconcile_at {
                wake = wake.min(t);
            }
            match rx.recv_timeout(wake.saturating_duration_since(now)) {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let now = Instant::now();
            if self.reconcile_at.is_some_and(|t| now >= t) {
                self.reconcile_at = None;
                self.reconcile();
            }
            if now >= self.next_tick {
                self.tick(now);
            }
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Game(g) => {
                self.game = g;
                for i in 0..self.docks.len() {
                    self.sync_static(i);
                    self.sync_pill(i);
                }
            }
            Msg::DragOver { label, over } => {
                if let Some(d) = self.docks.iter_mut().find(|d| d.label == label) {
                    d.drag_over = over;
                }
            }
            Msg::ShellRestarted => {
                for i in 0..self.docks.len() {
                    self.docks[i].appbar = None;
                    self.sync_appbar(i);
                }
            }
            Msg::Peek { monitor_id, duration_ms } => {
                let until = Instant::now() + Duration::from_millis(duration_ms);
                for d in self.docks.iter_mut() {
                    if monitor_id.is_empty() || d.monitor.id == monitor_id {
                        d.peek_until = Some(until);
                    }
                }
            }
            Msg::Reconcile => self.reconcile_at = Some(Instant::now() + RECONCILE_DEBOUNCE),
            Msg::CloseThumbs => {
                self.thumbs_pending = None;
                self.close_thumbs();
            }
            Msg::ThumbsReady => {
                self.thumbs_ready = true;
                if let Some((item_id, monitor_id, windows)) = self.thumbs_pending.take() {
                    self.show_thumbs(item_id, monitor_id, windows);
                }
            }
            Msg::ShowThumbs { item_id, monitor_id, windows } => self.show_thumbs(item_id, monitor_id, windows),
            Msg::HoverThumbs { item_id, monitor_id, windows } => {
                let state = self.docks.iter().find(|d| d.monitor.id == monitor_id).map(|d| d.state);
                if state.is_some_and(|st| hover_allowed(&self.game, st)) {
                    self.show_thumbs(item_id, monitor_id, windows);
                }
            }
            Msg::Settings(s) => {
                let old = std::mem::replace(&mut self.settings, *s);
                if old.dock.position != self.settings.dock.position {
                    self.close_thumbs();
                }
                if old.dock.monitors != self.settings.dock.monitors {
                    self.reconcile();
                } else if old.dock.position != self.settings.dock.position {
                    for i in 0..self.docks.len() {
                        self.place(i);
                    }
                }
                for i in 0..self.docks.len() {
                    self.sync_pill(i);
                    self.sync_appbar(i);
                }
            }
            Msg::Layout { monitor_id, width, height, islands, hit, pill } => {
                let layout = Layout { w: width, h: height, islands, hit, pill };
                let pos = self.docks.iter().position(|d| d.monitor.id == monitor_id);
                if let Some(i) = pos {
                    let first = !self.docks[i].page_ready;
                    self.docks[i].page_ready = true;
                    if first {
                        self.report_ready(true);
                        // « Toujours visible » shows a dock as soon as its window exists,
                        // before its page listens: that `dock://reveal` was lost and the
                        // page kept its content slid out under the glass. Repeat it.
                        if matches!(self.docks[i].state, RevealState::Shown | RevealState::Disarming { .. }) {
                            let label = self.docks[i].label.as_str();
                            if let Err(e) = self.app.emit_to(label, "dock://reveal", RevealPayload { shown: true }) {
                                eprintln!("lucent: cannot emit dock://reveal to {label}: {e}");
                            }
                        }
                    }
                    let d = &mut self.docks[i];
                    if !first && d.layout.w == layout.w && d.layout.h == layout.h {
                        // Same window size, new island shapes (magnification runs every
                        // frame): only the region changes, the window stays put.
                        let reshape = d.layout.islands != layout.islands || d.layout.hit != layout.hit;
                        d.layout = layout;
                        if !self.sync_pill(i) && reshape {
                            let d = &self.docks[i];
                            let extend = d.slide_end.map(|_| self.settings.dock.position);
                            apply_shape(&d.window, d.hwnd, &d.layout, d.monitor.scale, extend);
                        }
                    } else {
                        d.layout = layout;
                        if !self.sync_pill(i) {
                            self.place(i);
                        }
                    }
                    self.sync_appbar(i);
                }
            }
        }
    }

    /// Applies (or removes) static glass on dock `i` to match the game state and the
    /// dock's current tone. Only touches the compositor when something changed.
    fn sync_static(&mut self, i: usize) {
        let d = &self.docks[i];
        let want = static_glass(&self.game, &d.monitor.id).then(|| glass::win::last_tone(&d.label) == "dark");
        if want == d.static_glass {
            return;
        }
        let hwnd = d.hwnd;
        let _ = d.window.run_on_main_thread(move || {
            let r = match want {
                Some(dark) => crate::backdrop::set_tone(hwnd, dark).and_then(|_| crate::backdrop::set_static(hwnd, true)),
                None => crate::backdrop::set_static(hwnd, false),
            };
            if let Err(e) = r {
                eprintln!("lucent: cannot switch the dock glass for game mode: {e}");
            }
        });
        self.docks[i].static_glass = want;
    }

    /// Keeps dock `i`'s window on-screen showing only the system pill while it is hidden
    /// in Game Mode, parks it again when the pill goes (game over, setting off), and holds
    /// the system monitor while any pill shows. Returns true when it re-placed the window.
    fn sync_pill(&mut self, i: usize) -> bool {
        let d = &self.docks[i];
        let want = pill_on_screen(&self.game, self.settings.game_mode.monitor_pill, &d.monitor.id, d.state, d.layout.pill);
        if want == d.pill {
            return false;
        }
        let revealed = matches!(d.state, RevealState::Shown | RevealState::Disarming { .. });
        self.docks[i].pill = want;
        self.sync_sysmon();
        if revealed {
            // The dock took over the window (it is already on-screen).
            return false;
        }
        let d = &mut self.docks[i];
        if want {
            // Cancels a pending park (the dock has just slid away) or brings it back.
            d.park_at = None;
            d.on_screen = true;
            set_pos(d.hwnd(), Some(HWND_TOPMOST), Rect::default(), SWP_NOMOVE | SWP_NOSIZE);
        } else if d.park_at.is_none() {
            d.on_screen = false;
        } else {
            return false; // parked when its hide slide ends
        }
        self.place(i);
        true
    }

    fn sync_sysmon(&mut self) {
        let any = self.docks.iter().any(|d| d.pill);
        if any == self.sysmon_held {
            return;
        }
        self.sysmon_held = any;
        if any {
            crate::sysmon::acquire(&self.app);
        } else {
            crate::sysmon::release();
        }
    }

    /// Reserves (or frees) the dock's strip of its monitor for « Toujours visible », so
    /// maximized windows stop at the dock like at a taskbar. Follows the row's rest size.
    fn sync_appbar(&mut self, i: usize) {
        let pos = self.pos();
        let d = &self.docks[i];
        let want = (self.settings.dock.always_visible && d.page_ready)
            .then(|| reserved_strip(&d.monitor, pos, self.shown_rect(d), &d.layout.islands))
            .flatten();
        let d = &mut self.docks[i];
        appbar::set(d.hwnd, &mut d.appbar, want, pos);
    }

    fn pos(&self) -> DockPosition {
        self.settings.dock.position
    }

    fn slide_ms(&self) -> u64 {
        let a = &self.settings.animations;
        if a.reduce_motion { 0 } else { (SLIDE_MS / a.speed.max(0.1)).round() as u64 }
    }

    fn shown_rect(&self, d: &Dock) -> Rect {
        monitors::dock_rect(&d.monitor, self.pos(), d.layout.w, d.layout.h)
    }

    fn park(&self, d: &Dock) -> Park {
        monitors::park_strategy(&d.monitor, &self.all, self.shown_rect(d), self.pos())
    }

    /// Re-applies size, position, visibility and region of dock `i` for its current state.
    /// A dock never leaves its own monitor: crossing onto a neighbour with another DPI
    /// would fire WM_DPICHANGED and make the page re-report its layout at the wrong scale.
    fn place(&mut self, i: usize) {
        let d = &self.docks[i];
        let shown = self.shown_rect(d);
        if d.on_screen {
            set_pos(d.hwnd(), None, shown, SWP_NOZORDER | SWP_SHOWWINDOW);
        } else {
            match self.park(d) {
                Park::OffEdge(r) => set_pos(d.hwnd(), None, r, SWP_NOZORDER | SWP_SHOWWINDOW),
                Park::HideInPlace => {
                    // Hide first, then size/move it to its shown rect while hidden.
                    set_pos(d.hwnd(), None, shown, SWP_NOZORDER | SWP_NOMOVE | SWP_NOSIZE | SWP_HIDEWINDOW);
                    set_pos(d.hwnd(), None, shown, SWP_NOZORDER);
                }
            }
        }
        let extend = d.slide_end.map(|_| self.pos());
        apply_shape(&d.window, d.hwnd, &d.layout, d.monitor.scale, extend);
        if d.slide_end.is_none() {
            let to = if d.on_screen { (0.0, 0.0) } else { slide_offset(&d.layout, self.pos(), d.monitor.scale) };
            glass_slide(&d.window, d.hwnd, to, 0);
        }
        // Glass tone follows the area behind the dock. During magnification only the
        // region changes (the page keeps its window size), so this stays rare.
        if d.page_ready && d.tone_rect != Some(shown) {
            glass::win::send(glass::win::Msg::Dock { label: d.label.clone(), monitor: d.monitor.rect, area: shown });
            self.docks[i].tone_rect = Some(shown);
        }
    }

    fn reconcile(&mut self) {
        self.close_thumbs();
        let all = monitors::win::enumerate();
        let mut sel = monitors::selected(&all, &self.settings.dock.monitors);
        if sel.is_empty() {
            // The chosen monitors are all unplugged: keep a dock on the primary one.
            sel = monitors::selected(&all, &MonitorSelection::Primary);
        }
        let diff = monitors::monitor_diff(&self.monitors, &sel);
        self.all = all;
        // Create before destroying so the app never has zero windows (see `run` in lib.rs).
        for m in diff.create {
            match self.create(m) {
                Ok(d) => self.docks.push(d),
                Err(e) => eprintln!("lucent: cannot create dock window: {e}"),
            }
        }
        for m in diff.update {
            if let Some(d) = self.docks.iter_mut().find(|d| d.monitor.id == m.id) {
                d.monitor = m;
            }
        }
        for id in diff.destroy {
            if let Some(i) = self.docks.iter().position(|d| d.monitor.id == id) {
                let mut d = self.docks.remove(i);
                appbar::set(d.hwnd, &mut d.appbar, None, self.settings.dock.position);
                glass::win::send(glass::win::Msg::Forget(d.label.clone()));
                let hwnd = d.hwnd;
                let _ = d.window.run_on_main_thread(move || crate::backdrop::detach(hwnd));
                if let Err(e) = d.window.destroy() {
                    eprintln!("lucent: cannot destroy {}: {e}", d.label);
                }
            }
        }
        self.monitors = self.docks.iter().map(|d| d.monitor.clone()).collect();
        // Re-place every dock even when nothing changed: after WM_DPICHANGED (diff empty)
        // tao has moved/resized the window to its own suggested rect.
        for i in 0..self.docks.len() {
            self.place(i);
        }
        if self.docks.is_empty() {
            // No dock at all (every creation failed): the taskbar must come back.
            self.report_ready(false);
        }
        // A destroyed dock may have held the system monitor.
        self.sync_sysmon();
    }

    fn report_ready(&mut self, ready: bool) {
        if ready != self.ready {
            self.ready = ready;
            (self.on_ready)(ready);
        }
    }

    fn close_thumbs(&mut self) {
        if let Some(t) = &mut self.thumbs {
            t.hide();
        }
    }

    fn show_thumbs(&mut self, item_id: String, monitor_id: String, windows: Vec<(isize, String)>) {
        let Some(i) = self.docks.iter().position(|d| d.monitor.id == monitor_id) else { return };
        if self.thumbs.is_none() {
            match ThumbsWindow::create(&self.app) {
                Ok(t) => self.thumbs = Some(t),
                Err(e) => {
                    eprintln!("lucent: cannot create thumbnails window: {e}");
                    return;
                }
            }
        }
        if !self.thumbs_ready {
            // Fresh webview: shown once its page says it listens (`Msg::ThumbsReady`).
            self.thumbs_pending = Some((item_id, monitor_id, windows));
            return;
        }
        let Some(t) = &mut self.thumbs else { return };
        if t.open.as_ref().is_some_and(|o| o.item_id == item_id && o.monitor_id == monitor_id) {
            // Hover opened it, then a click: keep it (the hover machine closes it on leave).
            return;
        }
        let mut pt = POINT::default();
        let d = &self.docks[i];
        let dock = monitors::dock_rect(&d.monitor, self.settings.dock.position, d.layout.w, d.layout.h);
        // The clicked icon is under the cursor; fall back to the dock's centre.
        let anchor = if unsafe { GetCursorPos(&mut pt) }.is_ok() { (pt.x, pt.y) } else { (dock.x + dock.w / 2, dock.y + dock.h / 2) };
        // Start edge detection from the current state: the click that opened it is not an "outside" click.
        self.prev_esc = key_down(VK_ESCAPE.0 as i32);
        self.prev_click = key_down(VK_LBUTTON.0 as i32) || key_down(VK_RBUTTON.0 as i32);
        t.show(ShowReq {
            item_id,
            monitor: d.monitor.clone(),
            dock,
            pos: self.settings.dock.position,
            anchor,
            windows,
            tone: glass::win::last_tone(&d.label).to_string(),
            static_glass: static_glass(&self.game, &d.monitor.id),
        });
    }

    /// Escape or a click outside the popup (and outside its dock, whose own click handler
    /// decides) closes it. Polled, no low-level hooks.
    fn poll_thumbs_close(&mut self, cursor: Option<(i32, i32)>) {
        let Some(t) = &self.thumbs else { return };
        let Some(open) = &t.open else { return };
        let esc = key_down(VK_ESCAPE.0 as i32);
        let click = key_down(VK_LBUTTON.0 as i32) || key_down(VK_RBUTTON.0 as i32);
        let esc_edge = esc && !self.prev_esc;
        let click_edge = click && !self.prev_click;
        self.prev_esc = esc;
        self.prev_click = click;
        let dock_rect = self.docks.iter().find(|d| d.monitor.id == open.monitor_id).map(|d| dock_shape_box(self.shown_rect(d), &d.layout.islands, &d.layout.hit));
        let outside = cursor.is_some_and(|(x, y)| !open.rect.contains(x, y) && !dock_rect.is_some_and(|r| r.contains(x, y)));
        if esc_edge || (click_edge && outside) {
            self.close_thumbs();
        }
    }

    fn create(&mut self, m: MonitorInfo) -> tauri::Result<Dock> {
        let label = format!("dock-{}", self.next_n);
        self.next_n += 1;
        let url = format!("dock.html?monitor={}", percent_encode(&m.id));
        let window = WebviewWindowBuilder::new(&self.app, &label, WebviewUrl::App(url.into()))
            .title("Lucent")
            .transparent(true)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .focusable(false)
            .shadow(false)
            .resizable(false)
            .visible(false)
            .build()?;
        let hwnd = window.hwnd()?.0 as isize;
        let _ = window.run_on_main_thread(move || crate::backdrop::attach_or_accent(hwnd, ACCENT_TINT));
        make_non_activating(HWND(hwnd as *mut _));
        // Subclassing only works from the thread owning the window (the main thread).
        let _ = window.run_on_main_thread(move || unsafe {
            let _ = SetWindowSubclass(HWND(hwnd as *mut _), Some(no_activate_proc), 1, 0);
        });
        let drag_label = label.clone();
        window.on_window_event(move |e| match e {
            WindowEvent::ScaleFactorChanged { .. } => request_reconcile(),
            WindowEvent::DragDrop(ev) => {
                use tauri::DragDropEvent;
                let over = matches!(ev, DragDropEvent::Enter { .. } | DragDropEvent::Over { .. });
                send(Msg::DragOver { label: drag_label.clone(), over });
            }
            _ => {}
        });
        let layout = Layout {
            w: (DEFAULT_CONTENT.0 * m.scale).round() as u32,
            h: (DEFAULT_CONTENT.1 * m.scale).round() as u32,
            islands: Vec::new(),
            hit: Vec::new(),
            pill: false,
        };
        let d = Dock {
            label,
            window,
            hwnd,
            monitor: m,
            layout,
            state: RevealState::Hidden,
            on_screen: false,
            park_at: None,
            slide_end: None,
            page_ready: false,
            tone_rect: None,
            static_glass: None,
            pill: false,
            peek_until: None,
            drag_over: false,
            appbar: None,
        };
        // Topmost now; `reconcile` then places it (parked past the edge but shown, so the
        // webview loads and renders before the first reveal, or hidden in place).
        set_pos(d.hwnd(), Some(HWND_TOPMOST), Rect::default(), SWP_NOMOVE | SWP_NOSIZE);
        Ok(d)
    }

    fn tick(&mut self, now: Instant) {
        let now_ms = now.duration_since(self.epoch).as_millis() as u64;
        let mut pt = POINT::default();
        // Fails on the secure desktop (UAC, lock screen): treat as "cursor nowhere".
        let cursor = unsafe { GetCursorPos(&mut pt) }.ok().map(|_| (pt.x, pt.y));
        let reveal_delay = self.settings.dock.reveal_delay_ms as u64;
        let hide_delay = self.settings.dock.hide_delay_ms as u64;
        let pos = self.pos();
        let fullscreen = monitors::fullscreen_foreground_rect();
        for i in 0..self.docks.len() {
            let d = &self.docks[i];
            let shown = self.shown_rect(d);
            let shape = dock_shape_box(shown, &d.layout.islands, &d.layout.hit);
            let trigger = monitors::trigger_zone(&d.monitor, pos);
            // The popup is part of its dock: resting on it keeps the dock (and the popup) up.
            let on_popup = self.thumbs.as_ref().and_then(|t| t.open.as_ref()).is_some_and(|o| {
                o.monitor_id == d.monitor.id && cursor.is_some_and(|(x, y)| o.rect.contains(x, y))
            });
            let peek = d.peek_until.is_some_and(|until| now < until);
            let clear_peek = d.peek_until.is_some_and(|until| now >= until);
            let input = RevealInput {
                now_ms,
                cursor_in_trigger: cursor.is_some_and(|(x, y)| trigger.contains(x, y)),
                cursor_in_dock: d.on_screen && (on_popup || cursor.is_some_and(|(x, y)| shape.contains(x, y))),
                hotkey_toggle: false,
                peek,
                drag_over: d.drag_over,
                pinned: self.settings.dock.always_visible,
                // A fullscreen / borderless app or the focused game owns this monitor:
                // never pop over it.
                suppressed: suppressed(fullscreen.is_some_and(|r| r.covers(&d.monitor.rect)), &self.game, &d.monitor.id),
            };
            let (state, action) = reveal::step(d.state, &input, reveal_delay, hide_delay);
            self.docks[i].state = state;
            if clear_peek {
                self.docks[i].peek_until = None;
            }
            match action {
                Some(RevealAction::Show) => self.show(i),
                Some(RevealAction::Hide) => {
                    if self.thumbs.as_ref().and_then(|t| t.open.as_ref()).is_some_and(|o| o.monitor_id == self.docks[i].monitor.id) {
                        self.close_thumbs();
                    }
                    self.emit_reveal(i, false);
                    self.docks[i].park_at = Some(now + Duration::from_millis(self.slide_ms()));
                }
                None => {}
            }
            if action.is_some() {
                self.sync_pill(i);
            }
            if self.docks[i].slide_end.is_some_and(|t| now >= t) {
                self.docks[i].slide_end = None;
                let d = &self.docks[i];
                apply_shape(&d.window, d.hwnd, &d.layout, d.monitor.scale, None);
            }
            if self.docks[i].park_at.is_some_and(|t| now >= t) {
                self.docks[i].park_at = None;
                self.docks[i].on_screen = false;
                self.place(i);
            }
        }
        if self.game.active {
            // New docks (reconcile) and tone changes during the session.
            for i in 0..self.docks.len() {
                self.sync_static(i);
            }
        }
        self.poll_thumbs_close(cursor);
        self.poll_corners(cursor, now_ms, fullscreen);
        let thumbs_open = self.thumbs.as_ref().is_some_and(|t| t.open.is_some());
        // A pinned dock at rest needs no fast polling (it never hides on its own).
        let pinned = self.settings.dock.always_visible;
        let busy = thumbs_open
            || self.docks.iter().any(|d| {
                d.peek_until.is_some()
                    || (d.state != RevealState::Hidden && !(pinned && d.state == RevealState::Shown))
                    || d.park_at.is_some()
                    || d.slide_end.is_some()
            });
        self.next_tick = now + if busy { POLL_SHOWN } else { POLL_HIDDEN };
    }

    /// Hot corners on every monitor (not only those with a dock). Inert while a mouse
    /// button is down, a game is focused or a fullscreen app covers the monitor under the
    /// cursor; "none" corners are not even hit.
    fn poll_corners(&mut self, cursor: Option<(i32, i32)>, now_ms: u64, fullscreen: Option<Rect>) {
        use crate::corners;
        let hc = &self.settings.hot_corners;
        let rects: Vec<Rect> = self.all.iter().map(|m| m.rect).collect();
        let hit = cursor
            .and_then(|c| corners::hit(c, &rects))
            .filter(|h| corners::action_for(h.corner, hc).is_some());
        if hit.is_none() && self.corners.idle() {
            return;
        }
        let blocked = (self.game.active && self.game.focused)
            || cursor.is_some_and(|c| corners::fullscreen_under(c, &rects, fullscreen))
            || (hit.is_some() && (key_down(VK_LBUTTON.0 as i32) || key_down(VK_RBUTTON.0 as i32)));
        if let Some(action) = self.corners.step(hit, cursor, now_ms, blocked).and_then(|c| corners::action_for(c, hc)) {
            crate::run_action(&self.app, action);
        }
    }

    fn show(&mut self, i: usize) {
        let d = &mut self.docks[i];
        d.park_at = None;
        d.on_screen = true;
        let d = &self.docks[i];
        set_pos(d.hwnd(), Some(HWND_TOPMOST), self.shown_rect(d), SWP_SHOWWINDOW);
        self.emit_reveal(i, true);
    }

    /// Only called on transitions: the webview is never woken while nothing changes.
    fn emit_reveal(&mut self, i: usize, shown: bool) {
        // No slide on the other monitors while a game runs (the page drops it too).
        let ms = if static_glass(&self.game, &self.docks[i].monitor.id) { 0 } else { self.slide_ms() };
        let pos = self.pos();
        let d = &mut self.docks[i];
        d.slide_end = (ms > 0).then(|| Instant::now() + Duration::from_millis(ms));
        let extend = d.slide_end.map(|_| pos);
        apply_shape(&d.window, d.hwnd, &d.layout, d.monitor.scale, extend);
        let to = if shown { (0.0, 0.0) } else { slide_offset(&d.layout, pos, d.monitor.scale) };
        glass_slide(&d.window, d.hwnd, to, ms as u32);
        let label = self.docks[i].label.as_str();
        if let Err(e) = self.app.emit_to(label, "dock://reveal", RevealPayload { shown }) {
            eprintln!("lucent: cannot emit dock://reveal to {label}: {e}");
        }
    }
}

/// Window region + native glass for the islands. The region (GDI, aliased) is grown a
/// little past each island so it only ever cuts transparent pixels; the glass and the CSS
/// rim draw the real, anti-aliased edge inside it.
/// `extend`: a slide is running, so each island's region also reaches the screen edge
/// the dock slides from (the glass and the icons travel through that strip).
fn apply_shape(window: &WebviewWindow, hwnd: isize, layout: &Layout, scale: f32, extend: Option<DockPosition>) {
    let islands = &layout.islands;
    let pad = (region::REGION_PAD * scale).round() as i32;
    let (w, h) = (layout.w as i32, layout.h as i32);
    let grown: Vec<Rect> = islands
        .iter()
        .map(|&r| region::grown(r, pad))
        .map(|r| match extend {
            None => r,
            Some(DockPosition::Bottom) => Rect { h: h - r.y, ..r },
            Some(DockPosition::Left) => Rect { x: 0, w: r.x + r.w, ..r },
            Some(DockPosition::Right) => Rect { w: w - r.x, ..r },
        })
        .chain(layout.hit.iter().copied())
        .collect();
    let radius = (region::ISLAND_RADIUS * scale).round() as i32;
    region::apply(HWND(hwnd as *mut _), &grown, radius + pad);
    let islands = islands.to_vec();
    let glass_radius = region::ISLAND_RADIUS * scale;
    let _ = window.run_on_main_thread(move || {
        if let Err(e) = crate::backdrop::set_islands(hwnd, &islands, glass_radius) {
            eprintln!("lucent: cannot reshape the dock glass: {e}");
        }
    });
}

/// Where the glass rests while hidden, relative to its shown place: the CSS slide moves
/// #dock by its own size + 16 px (`translate3d(0, calc(100% + 16px), 0)`); #dock is as
/// thick as its thickest island.
fn slide_offset(layout: &Layout, pos: DockPosition, scale: f32) -> (f32, f32) {
    let extra = 16.0 * scale;
    let thick_h = layout.islands.iter().map(|r| r.h).max().unwrap_or(0) as f32 + extra;
    let thick_w = layout.islands.iter().map(|r| r.w).max().unwrap_or(0) as f32 + extra;
    match pos {
        DockPosition::Bottom => (0.0, thick_h),
        DockPosition::Left => (-thick_w, 0.0),
        DockPosition::Right => (thick_w, 0.0),
    }
}

fn glass_slide(window: &WebviewWindow, hwnd: isize, to: (f32, f32), ms: u32) {
    let _ = window.run_on_main_thread(move || {
        if let Err(e) = crate::backdrop::slide(hwnd, to, ms) {
            eprintln!("lucent: cannot slide the dock glass: {e}");
        }
    });
}

fn key_down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
}

pub(crate) fn set_pos(hwnd: HWND, after: Option<HWND>, r: Rect, flags: SET_WINDOW_POS_FLAGS) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
            hwnd,
            after,
            r.x,
            r.y,
            r.w,
            r.h,
            flags | SWP_NOACTIVATE,
        );
    }
}

/// Belt and braces on top of Tauri's `focusable(false)`: the dock must never take the
/// foreground (and stays out of Alt+Tab).
pub(crate) fn make_non_activating(hwnd: HWND) {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let want = ex | (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize;
        if want != ex {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
        }
    }
}

/// Clicks on the webview (a child window) bubble `WM_MOUSEACTIVATE` up to the dock
/// window: answer "don't activate", the click itself is still delivered.
pub(crate) unsafe extern "system" fn no_activate_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

/// Query-string encoding for monitor ids such as `\\.\DISPLAY1`.
fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn percent_encode_device_name() {
        assert_eq!(super::percent_encode(r"\\.\DISPLAY1"), "%5C%5C.%5CDISPLAY1");
    }
}

#[cfg(test)]
mod slide_tests {
    use super::*;

    #[test]
    fn hidden_glass_sits_past_the_edge_by_the_css_distance() {
        let layout = Layout {
            w: 900,
            h: 120,
            islands: vec![Rect { x: 10, y: 50, w: 500, h: 58 }, Rect { x: 520, y: 60, w: 300, h: 48 }],
            hit: Vec::new(),
            pill: false,
        };
        assert_eq!(slide_offset(&layout, DockPosition::Bottom, 1.0), (0.0, 74.0));
        assert_eq!(slide_offset(&layout, DockPosition::Bottom, 1.5), (0.0, 82.0));
        assert_eq!(slide_offset(&layout, DockPosition::Left, 1.0), (-516.0, 0.0));
        assert_eq!(slide_offset(&layout, DockPosition::Right, 1.0), (516.0, 0.0));
    }
}

#[cfg(test)]
mod game_tests {
    use super::*;
    use crate::game::session::GameStateDto;

    fn game(active: bool, focused: bool, monitor: &str) -> GameStateDto {
        GameStateDto { active, name: active.then(|| "Valorant".into()), monitor_id: active.then(|| monitor.into()), focused }
    }

    #[test]
    fn suppression_table() {
        let a = r"\\.\DISPLAY1";
        let b = r"\\.\DISPLAY2";
        // (fullscreen covers this monitor, game, this monitor) -> suppressed
        let rows = [
            (false, game(false, false, a), a, false),
            (true, game(false, false, a), a, true),
            (false, game(true, true, a), a, true),
            (false, game(true, true, a), b, false),
            (false, game(true, false, a), a, false),
            (true, game(true, false, a), a, true),
            (true, game(true, true, a), b, true),
            (false, game(false, true, a), a, false),
        ];
        for (i, (fs, g, m, want)) in rows.iter().enumerate() {
            assert_eq!(suppressed(*fs, g, m), *want, "row {i}");
        }
    }

    #[test]
    fn pill_on_other_monitors_while_hidden_with_the_setting() {
        let a = r"\\.\DISPLAY1";
        let b = r"\\.\DISPLAY2";
        // (game, setting on, this monitor, dock revealed) -> pill shown
        let rows = [
            (game(true, true, a), true, b, false, true),
            (game(true, false, a), true, b, false, true),
            (game(true, true, a), true, a, false, false),
            (game(true, true, a), false, b, false, false),
            (game(true, true, a), true, b, true, false),
            (game(false, false, a), true, b, false, false),
        ];
        for (i, (g, on, m, revealed, want)) in rows.iter().enumerate() {
            assert_eq!(show_pill(g, *on, m, *revealed), *want, "row {i}");
        }
    }

    #[test]
    fn pill_window_stays_on_screen_only_once_the_page_shows_it() {
        let a = r"\\.\DISPLAY1";
        let b = r"\\.\DISPLAY2";
        let g = game(true, true, a);
        assert!(pill_on_screen(&g, true, b, RevealState::Hidden, true));
        // The page still shows the full dock (its pill layout not reported yet).
        assert!(!pill_on_screen(&g, true, b, RevealState::Hidden, false));
        // Arming is still hidden: the pill stays until the dock really shows.
        assert!(pill_on_screen(&g, true, b, RevealState::Arming { since_ms: 0 }, true));
        assert!(!pill_on_screen(&g, true, b, RevealState::Shown, true));
        assert!(!pill_on_screen(&g, true, b, RevealState::Disarming { since_ms: 0 }, true));
        assert!(!pill_on_screen(&g, true, a, RevealState::Hidden, true));
    }

    #[test]
    fn hover_previews_only_on_a_shown_dock_outside_game_mode() {
        let a = r"\\.\DISPLAY1";
        assert!(hover_allowed(&game(false, false, a), RevealState::Shown));
        assert!(!hover_allowed(&game(false, false, a), RevealState::Hidden));
        assert!(!hover_allowed(&game(false, false, a), RevealState::Arming { since_ms: 0 }));
        assert!(!hover_allowed(&game(false, false, a), RevealState::Disarming { since_ms: 0 }));
        assert!(!hover_allowed(&game(true, true, a), RevealState::Shown));
        assert!(!hover_allowed(&game(true, false, a), RevealState::Shown));
    }

    #[test]
    fn static_glass_on_every_other_monitor_while_active() {
        let a = r"\\.\DISPLAY1";
        let b = r"\\.\DISPLAY2";
        assert!(!static_glass(&game(false, false, a), b));
        assert!(!static_glass(&game(true, true, a), a));
        assert!(static_glass(&game(true, true, a), b));
        assert!(static_glass(&game(true, false, a), b));
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    #[test]
    fn cursor_box_follows_the_islands_not_the_window() {
        let shown = Rect { x: 100, y: 500, w: 800, h: 520 };
        let islands = [Rect { x: 10, y: 450, w: 300, h: 54 }, Rect { x: 330, y: 450, w: 200, h: 54 }];
        let b = dock_shape_box(shown, &islands, &[]);
        assert_eq!(b, Rect { x: 104, y: 944, w: 532, h: 66 });
        assert!(!b.contains(300, 700)); // transparent headroom above the row
        assert!(b.contains(420, 970)); // gap between two islands
        // An open popup (an island) extends it upwards.
        let open = [islands[0], islands[1], Rect { x: 330, y: 200, w: 200, h: 240 }];
        assert!(dock_shape_box(shown, &open, &[]).contains(400, 750));
        // Nothing reported yet: the whole window.
        assert_eq!(dock_shape_box(shown, &[], &[]), shown);
    }
}

#[cfg(test)]
mod appbar_tests {
    use super::*;

    fn mon() -> MonitorInfo {
        let r = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        MonitorInfo { id: "M".into(), rect: r, work: r, scale: 1.0, primary: true }
    }

    #[test]
    fn strip_covers_the_row_not_the_popups() {
        // Window 520 tall at the bottom; row islands 54 tall at y=458, a popup above.
        let shown = Rect { x: 400, y: 552, w: 1100, h: 520 };
        let row = [Rect { x: 10, y: 458, w: 500, h: 54 }, Rect { x: 530, y: 458, w: 200, h: 54 }];
        let s = reserved_strip(&mon(), DockPosition::Bottom, shown, &row).unwrap();
        assert_eq!(s, Rect { x: 0, y: 552 + 458 - APPBAR_GAP, w: 1920, h: 1080 - (552 + 458 - APPBAR_GAP) });
        let with_popup = [row[0], row[1], Rect { x: 530, y: 100, w: 200, h: 340 }];
        assert_eq!(reserved_strip(&mon(), DockPosition::Bottom, shown, &with_popup), Some(s));
        assert_eq!(reserved_strip(&mon(), DockPosition::Bottom, shown, &[]), None);
    }

    #[test]
    fn strip_on_the_sides() {
        let shown = Rect { x: 8, y: 200, w: 600, h: 700 };
        let col = [Rect { x: 0, y: 10, w: 60, h: 400 }];
        assert_eq!(reserved_strip(&mon(), DockPosition::Left, shown, &col).unwrap().w, 8 + 60 + APPBAR_GAP);
        let shown = Rect { x: 1312, y: 200, w: 600, h: 700 };
        let col = [Rect { x: 540, y: 10, w: 60, h: 400 }];
        let s = reserved_strip(&mon(), DockPosition::Right, shown, &col).unwrap();
        assert_eq!((s.x, s.x + s.w), (1312 + 540 - APPBAR_GAP, 1920));
    }
}
