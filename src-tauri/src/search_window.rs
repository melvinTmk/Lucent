//! Spotlight-style search window: one focusable webview (`search.html`) with native rounded
//! glass, centred horizontally on the monitor under the cursor, its top at 22 % of the
//! monitor height. Toggled by the global hotkey; hidden on Échap (page), focus loss or a
//! second hotkey press; destroyed after 5 min hidden (RAM budget), rebuilt on next show.
//!
//! Everything runs on one worker thread fed by a channel (as `dock_host`): building a webview
//! from the main thread deadlocks, and the hotkey thread must only hand work off. Glass
//! calls (`backdrop`) go to the main thread through `run_on_main_thread`.
use crate::monitors::{MonitorInfo, Rect};
use std::time::{Duration, Instant};

pub const LABEL: &str = "search";
/// Fixed window width, logical px.
pub const WIDTH: f32 = 680.0;
/// Window height bounds, logical px: at least the input row, at most 520.
pub const MIN_HEIGHT: f32 = 64.0;
pub const MAX_HEIGHT: f32 = 520.0;
/// Corner radius of the glass (and the CSS `.island`), logical px.
pub const RADIUS: f32 = 22.0;
/// Top of the window, as a share of the monitor height.
const TOP_SHARE: f32 = 0.22;

/// Window rect (physical px) for a `logical_w` × `logical_h` window on `m`: horizontally
/// centred, top at 22 % of the monitor height.
pub fn centred_rect(m: &MonitorInfo, logical_w: f32, logical_h: f32) -> Rect {
    let w = (logical_w * m.scale).round() as i32;
    let h = (logical_h * m.scale).round() as i32;
    Rect {
        x: m.rect.x + (m.rect.w - w) / 2,
        y: m.rect.y + (m.rect.h as f32 * TOP_SHARE).round() as i32,
        w,
        h,
    }
}

/// Page content height (CSS px) → window height (logical px).
pub fn clamp_height(css_h: u32) -> f32 {
    (css_h as f32).clamp(MIN_HEIGHT, MAX_HEIGHT)
}

/// The monitor containing `cursor`, else the primary one, else the first.
pub fn monitor_at(all: &[MonitorInfo], cursor: Option<(i32, i32)>) -> Option<&MonitorInfo> {
    cursor
        .and_then(|(x, y)| all.iter().find(|m| m.rect.contains(x, y)))
        .or_else(|| all.iter().find(|m| m.primary))
        .or_else(|| all.first())
}

/// Hidden this long → destroyed (next show rebuilds it).
pub const IDLE_DESTROY: Duration = Duration::from_secs(5 * 60);
/// A fresh webview is shown once its page reports its height, or after this at worst.
pub const READY_TIMEOUT: Duration = Duration::from_millis(1500);

/// What reaches the window worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Toggle,
    Show,
    Hide,
    /// `WindowEvent::Focused(false)`; `ours`: the search window is still the foreground one
    /// (a late event from an earlier hide).
    FocusLost { ours: bool },
    /// Page content height, CSS px.
    Resize(u32),
    /// A deadline (`Shell::wake`) may have passed.
    Tick,
}

/// What the worker must do to the real window, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Pick the monitor under the cursor; build the webview if it does not exist.
    Prepare,
    /// Place + shape the window, take the foreground, emit `search://shown`.
    Reveal,
    /// Re-place + reshape (height changed while visible).
    Place,
    HideWindow,
    /// `backdrop::detach`, then destroy the webview.
    Destroy,
}

/// Pure show/hide/idle state of the search window: every decision lives here, the
/// worker only performs the effects.
#[derive(Debug, Clone, PartialEq)]
pub struct Shell {
    /// A webview exists (built by `Prepare`, gone after `Destroy`).
    pub exists: bool,
    /// Its page has reported its height once (it listens to `search://shown`).
    pub ready: bool,
    pub visible: bool,
    /// Show requested on a fresh webview: revealed on its first `Resize` or at this deadline.
    pub pending_show: Option<Instant>,
    /// The webview exists but is not shown since then (idle destroy after `IDLE_DESTROY`).
    pub hidden_since: Option<Instant>,
    /// Window height, logical px.
    pub height: f32,
}

impl Default for Shell {
    fn default() -> Self {
        Self { exists: false, ready: false, visible: false, pending_show: None, hidden_since: None, height: MIN_HEIGHT }
    }
}

impl Shell {
    /// Next deadline the worker must wake up for (`Input::Tick`).
    pub fn wake(&self) -> Option<Instant> {
        [self.pending_show, self.hidden_since.map(|t| t + IDLE_DESTROY)].into_iter().flatten().min()
    }

    pub fn step(&mut self, input: Input, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        match input {
            Input::Toggle if self.visible || self.pending_show.is_some() => self.hide(now, &mut fx),
            Input::Toggle | Input::Show => self.show(now, &mut fx),
            Input::Hide => self.hide(now, &mut fx),
            Input::FocusLost { ours } => {
                if self.visible && !ours {
                    self.hide(now, &mut fx);
                }
            }
            Input::Resize(css_h) => {
                self.height = clamp_height(css_h);
                if self.exists {
                    let first = !std::mem::replace(&mut self.ready, true);
                    if first && self.pending_show.is_some() {
                        self.reveal(&mut fx);
                    } else if self.visible {
                        fx.push(Effect::Place);
                    }
                }
            }
            Input::Tick => {
                if self.pending_show.is_some_and(|t| now >= t) {
                    self.reveal(&mut fx);
                }
                if self.hidden_since.is_some_and(|t| now >= t + IDLE_DESTROY) {
                    *self = Shell { height: self.height, ..Shell::default() };
                    fx.push(Effect::Destroy);
                }
            }
        }
        fx
    }

    /// `Prepare` failed (the webview could not be built): back to "no window".
    pub fn create_failed(&mut self) {
        *self = Shell { height: self.height, ..Shell::default() };
    }

    fn show(&mut self, now: Instant, fx: &mut Vec<Effect>) {
        if self.visible || self.pending_show.is_some() {
            return;
        }
        self.hidden_since = None;
        self.exists = true;
        fx.push(Effect::Prepare);
        if self.ready {
            self.reveal(fx);
        } else {
            self.pending_show = Some(now + READY_TIMEOUT);
        }
    }

    fn reveal(&mut self, fx: &mut Vec<Effect>) {
        self.pending_show = None;
        self.visible = true;
        fx.push(Effect::Reveal);
    }

    /// Also cancels a pending show; a webview that exists but is not shown always gets an
    /// idle deadline (even one hidden before its first reveal).
    fn hide(&mut self, now: Instant, fx: &mut Vec<Effect>) {
        self.pending_show = None;
        if self.visible {
            self.visible = false;
            fx.push(Effect::HideWindow);
        }
        if self.exists && self.hidden_since.is_none() {
            self.hidden_since = Some(now);
        }
    }
}

/// Shows the window if hidden, hides it otherwise. Never blocks (safe from the hotkey thread).
pub fn toggle(app: &tauri::AppHandle) {
    #[cfg(windows)]
    win::send(app, win::Msg::Toggle);
    #[cfg(not(windows))]
    let _ = app;
}

pub fn show(app: &tauri::AppHandle) {
    #[cfg(windows)]
    win::send(app, win::Msg::Show);
    #[cfg(not(windows))]
    let _ = app;
}

pub fn hide(app: &tauri::AppHandle) {
    #[cfg(windows)]
    win::send(app, win::Msg::Hide);
    #[cfg(not(windows))]
    let _ = app;
}

/// The page reports its content height (CSS px); the window follows (clamped).
pub fn resize(app: &tauri::AppHandle, css_h: u32) {
    #[cfg(windows)]
    win::send(app, win::Msg::Resize(css_h));
    #[cfg(not(windows))]
    let _ = (app, css_h);
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::dock_host::set_pos;
    use crate::{monitors, region};
    use std::sync::OnceLock;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use tauri::{AppHandle, Emitter, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetForegroundWindow, HWND_TOPMOST, SWP_HIDEWINDOW, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SWP_SHOWWINDOW, SetForegroundWindow,
    };

    /// Accent tint used only when native glass is unavailable (same as the dock).
    const ACCENT_TINT: u32 = 0x10_80_80_80;

    pub enum Msg {
        Toggle,
        Show,
        Hide,
        Resize(u32),
        /// `WindowEvent::Focused(false)`.
        FocusLost,
    }

    static TX: OnceLock<Sender<Msg>> = OnceLock::new();

    /// Starts the worker on first use. Only pushes to a channel: never blocks.
    pub fn send(app: &AppHandle, msg: Msg) {
        let tx = TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            let app = app.clone();
            // Without the worker the search window stays unavailable; the app keeps running.
            if let Err(e) = std::thread::Builder::new().name("lucent-search".into()).spawn(move || Host::new(app).run(rx)) {
                eprintln!("lucent: cannot spawn search window thread: {e}");
            }
            tx
        });
        let _ = tx.send(msg);
    }

    struct Win {
        window: WebviewWindow,
        hwnd: isize,
    }

    impl Win {
        fn hwnd(&self) -> HWND {
            HWND(self.hwnd as *mut _)
        }
    }

    /// Performs the effects decided by the pure `Shell`.
    struct Host {
        app: AppHandle,
        shell: Shell,
        win: Option<Win>,
        /// Monitor the window is placed on (chosen at each show).
        monitor: Option<MonitorInfo>,
    }

    impl Host {
        fn new(app: AppHandle) -> Self {
            Self { app, shell: Shell::default(), win: None, monitor: None }
        }

        fn run(mut self, rx: Receiver<Msg>) {
            loop {
                let msg = match self.shell.wake() {
                    Some(t) => rx.recv_timeout(t.saturating_duration_since(Instant::now())),
                    None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                let input = match msg {
                    Ok(m) => self.input(m),
                    Err(RecvTimeoutError::Timeout) => Input::Tick,
                    Err(RecvTimeoutError::Disconnected) => return,
                };
                let was_pending = self.shell.pending_show;
                let fx = self.shell.step(input, Instant::now());
                if input == Input::Tick && was_pending.is_some() && fx.contains(&Effect::Reveal) {
                    eprintln!("lucent: search page did not report in time, showing anyway");
                }
                for e in fx {
                    if !self.perform(e) {
                        break;
                    }
                }
            }
        }

        fn input(&self, m: Msg) -> Input {
            match m {
                Msg::Toggle => Input::Toggle,
                Msg::Show => Input::Show,
                Msg::Hide => Input::Hide,
                Msg::Resize(h) => Input::Resize(h),
                Msg::FocusLost => Input::FocusLost {
                    ours: self.win.as_ref().is_some_and(|w| unsafe { GetForegroundWindow() } == w.hwnd()),
                },
            }
        }

        /// `false`: stop performing this step's remaining effects.
        fn perform(&mut self, e: Effect) -> bool {
            match e {
                Effect::Prepare => {
                    let mut pt = POINT::default();
                    let cursor = unsafe { GetCursorPos(&mut pt) }.ok().map(|_| (pt.x, pt.y));
                    let all = monitors::win::enumerate();
                    self.monitor = monitor_at(&all, cursor).cloned();
                    if self.win.is_none() {
                        match self.create() {
                            Ok(w) => self.win = Some(w),
                            Err(e) => {
                                eprintln!("lucent: cannot create search window: {e}");
                                self.shell.create_failed();
                                return false;
                            }
                        }
                    }
                }
                Effect::Reveal => self.reveal(),
                Effect::Place => self.place(),
                Effect::HideWindow => {
                    // A hidden search has no preview: its qlook URL stops working.
                    crate::search::quicklook::quicklook_close();
                    if let Some(w) = &self.win {
                        set_pos(w.hwnd(), None, Rect::default(), SWP_NOZORDER | SWP_NOMOVE | SWP_NOSIZE | SWP_HIDEWINDOW);
                    }
                }
                Effect::Destroy => self.destroy(),
            }
            true
        }

        /// Places, shows and focuses the window, then tells the page.
        fn reveal(&self) {
            let Some(w) = &self.win else { return };
            // Background thread when the app list is older than 10 min; never blocks the show.
            if let Some(idx) = tauri::Manager::try_state::<crate::search::SearchIndex>(&self.app) {
                crate::search::apps::refresh_if_stale(&idx);
                if let Some(store) = tauri::Manager::try_state::<crate::settings::SettingsStore>(&self.app) {
                    crate::search::projects::refresh_if_stale(&idx, store.get());
                }
            }
            self.place();
            let hwnd = w.hwnd;
            // SetForegroundWindow from the thread owning the window. Allowed: we are handling
            // our own hotkey (the last input event went to this process). No workaround if
            // Windows refuses it: just log.
            let _ = w.window.run_on_main_thread(move || {
                if !unsafe { SetForegroundWindow(HWND(hwnd as *mut _)) }.as_bool() {
                    eprintln!("lucent: SetForegroundWindow refused for the search window");
                }
            });
            if let Err(e) = self.app.emit_to(LABEL, "search://shown", ()) {
                eprintln!("lucent: cannot emit search://shown: {e}");
            }
        }

        /// Size, position, region and glass for the current monitor and height.
        fn place(&self) {
            let (Some(w), Some(m)) = (&self.win, &self.monitor) else { return };
            let rect = centred_rect(m, WIDTH, self.shell.height);
            // Moving onto a monitor with another DPI makes tao resize the window
            // (WM_DPICHANGED, handled during the first call): the second call wins.
            for _ in 0..2 {
                set_pos(w.hwnd(), Some(HWND_TOPMOST), rect, SWP_SHOWWINDOW);
            }
            apply_shape(&w.window, w.hwnd, rect.w, rect.h, m.scale);
        }

        fn destroy(&mut self) {
            let Some(w) = self.win.take() else { return };
            let hwnd = w.hwnd;
            // Queued before `destroy` on the same event loop: the glass goes first.
            let _ = w.window.run_on_main_thread(move || crate::backdrop::detach(hwnd));
            if let Err(e) = w.window.destroy() {
                eprintln!("lucent: cannot destroy the search window: {e}");
            }
        }

        fn create(&self) -> tauri::Result<Win> {
            let window = WebviewWindowBuilder::new(&self.app, LABEL, WebviewUrl::App("search.html".into()))
                .title("Lucent")
                .inner_size(WIDTH as f64, MIN_HEIGHT as f64)
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
            let _ = window.run_on_main_thread(move || crate::backdrop::attach_or_accent(hwnd, ACCENT_TINT));
            let app = self.app.clone();
            window.on_window_event(move |e| {
                if let WindowEvent::Focused(false) = e {
                    send(&app, Msg::FocusLost);
                }
            });
            Ok(Win { window, hwnd })
        }
    }

    /// One glass island over the whole client rect; the region is the same rounded rect
    /// grown by `REGION_PAD` (as for the dock), so it only cuts outside the glass.
    fn apply_shape(window: &WebviewWindow, hwnd: isize, w: i32, h: i32, scale: f32) {
        let island = Rect { x: 0, y: 0, w, h };
        let pad = (region::REGION_PAD * scale).round() as i32;
        let radius = RADIUS * scale;
        region::apply(HWND(hwnd as *mut _), &[region::grown(island, pad)], radius.round() as i32 + pad);
        let _ = window.run_on_main_thread(move || {
            if let Err(e) = crate::backdrop::set_islands(hwnd, &[island], radius) {
                eprintln!("lucent: cannot reshape the search glass: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(x: i32, y: i32, w: i32, h: i32, scale: f32) -> MonitorInfo {
        let rect = Rect { x, y, w, h };
        MonitorInfo { id: "M".into(), rect, work: rect, scale, primary: false }
    }

    #[test]
    fn centred_rect_uses_monitor_scale() {
        let m = mon(1920, 0, 2560, 1440, 1.5);
        let r = centred_rect(&m, 680.0, 120.0);
        let y = (1440.0f32 * 0.22).round() as i32;
        assert_eq!(r, Rect { x: 1920 + (2560 - 1020) / 2, y, w: 1020, h: 180 });
    }

    #[test]
    fn centred_rect_offsets_by_monitor_origin() {
        let m = mon(-1920, -200, 1920, 1080, 1.0);
        assert_eq!(centred_rect(&m, 680.0, 64.0), Rect { x: -1920 + 620, y: -200 + 238, w: 680, h: 64 });
    }

    #[test]
    fn height_is_clamped_between_input_row_and_max() {
        assert_eq!(clamp_height(0), MIN_HEIGHT);
        assert_eq!(clamp_height(300), 300.0);
        assert_eq!(clamp_height(2000), MAX_HEIGHT);
    }

    #[test]
    fn monitor_at_prefers_cursor_then_primary() {
        let a = mon(0, 0, 1920, 1080, 1.0);
        let b = MonitorInfo { id: "B".into(), primary: true, ..mon(1920, 0, 2560, 1440, 1.5) };
        let all = vec![a.clone(), b.clone()];
        assert_eq!(monitor_at(&all, Some((100, 100))), Some(&a));
        assert_eq!(monitor_at(&all, Some((2000, 100))), Some(&b));
        assert_eq!(monitor_at(&all, Some((-50, 100))), Some(&b), "off-screen cursor: primary");
        assert_eq!(monitor_at(&all, None), Some(&b));
        assert_eq!(monitor_at(&[a.clone()], None), Some(&a), "no primary: first");
        assert_eq!(monitor_at(&[], None), None);
    }

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn double_press_during_first_creation_arms_idle_destroy() {
        let now = t0();
        let mut s = Shell::default();
        assert_eq!(s.step(Input::Toggle, now), vec![Effect::Prepare]);
        assert!(s.pending_show.is_some());
        assert_eq!(s.step(Input::Toggle, now), vec![]);
        assert!(s.pending_show.is_none() && !s.visible);
        assert_eq!(s.hidden_since, Some(now), "the hidden webview must get an idle deadline");
        // The page reports in later: nothing is shown.
        assert_eq!(s.step(Input::Resize(64), now), vec![]);
        assert_eq!(s.step(Input::Tick, now + IDLE_DESTROY), vec![Effect::Destroy]);
        assert!(!s.exists && s.wake().is_none());
    }

    #[test]
    fn fresh_window_reveals_on_first_resize_then_places() {
        let now = t0();
        let mut s = Shell::default();
        s.step(Input::Show, now);
        assert_eq!(s.step(Input::Resize(64), now), vec![Effect::Reveal]);
        assert!(s.visible);
        assert_eq!(s.step(Input::Resize(300), now), vec![Effect::Place]);
        assert_eq!(s.height, 300.0);
    }

    #[test]
    fn fresh_window_reveals_at_ready_timeout() {
        let now = t0();
        let mut s = Shell::default();
        s.step(Input::Show, now);
        assert_eq!(s.wake(), Some(now + READY_TIMEOUT));
        assert_eq!(s.step(Input::Tick, now + READY_TIMEOUT), vec![Effect::Reveal]);
        // The late first report only re-places.
        assert_eq!(s.step(Input::Resize(64), now), vec![Effect::Place]);
    }

    #[test]
    fn hide_then_idle_destroy_then_show_rebuilds() {
        let now = t0();
        let mut s = Shell::default();
        s.step(Input::Show, now);
        s.step(Input::Resize(120), now);
        assert_eq!(s.step(Input::Hide, now), vec![Effect::HideWindow]);
        assert_eq!(s.step(Input::Tick, now + IDLE_DESTROY - Duration::from_secs(1)), vec![]);
        assert_eq!(s.step(Input::Tick, now + IDLE_DESTROY), vec![Effect::Destroy]);
        let later = now + IDLE_DESTROY * 2;
        assert_eq!(s.step(Input::Toggle, later), vec![Effect::Prepare], "rebuilt, waits for its page");
        assert_eq!(s.pending_show, Some(later + READY_TIMEOUT));
        assert!(s.hidden_since.is_none());
        assert_eq!(s.step(Input::Resize(64), later), vec![Effect::Reveal]);
    }

    #[test]
    fn ready_window_toggles_and_show_clears_idle_deadline() {
        let now = t0();
        let mut s = Shell::default();
        s.step(Input::Show, now);
        s.step(Input::Resize(64), now);
        assert_eq!(s.step(Input::Toggle, now), vec![Effect::HideWindow]);
        assert_eq!(s.step(Input::Toggle, now), vec![Effect::Prepare, Effect::Reveal]);
        assert!(s.hidden_since.is_none() && s.wake().is_none());
        assert_eq!(s.step(Input::Show, now), vec![], "already visible");
    }

    #[test]
    fn focus_loss_hides_only_when_something_else_is_foreground() {
        let now = t0();
        let mut s = Shell::default();
        s.step(Input::Show, now);
        s.step(Input::Resize(64), now);
        assert_eq!(s.step(Input::FocusLost { ours: true }, now), vec![]);
        assert_eq!(s.step(Input::FocusLost { ours: false }, now), vec![Effect::HideWindow]);
        assert_eq!(s.step(Input::FocusLost { ours: false }, now), vec![], "already hidden");
    }

    #[test]
    fn failed_creation_returns_to_no_window() {
        let now = t0();
        let mut s = Shell::default();
        s.step(Input::Show, now);
        s.create_failed();
        assert!(!s.exists && s.wake().is_none());
        assert_eq!(s.step(Input::Show, now), vec![Effect::Prepare]);
    }
}
