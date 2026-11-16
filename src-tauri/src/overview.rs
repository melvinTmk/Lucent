//! Window overview (macOS Mission Control): one overlay per monitor, a dimmed backdrop
//! (CSS, nothing is captured) and every visible, non-minimised window of that monitor as a
//! live DWM thumbnail laid out in a balanced grid. Thumbnails glide from the windows' real
//! places to the grid on open and back on close, so the overlay appears and disappears
//! seamlessly over the real windows. Click a tile: that window comes to the front.
//! Échap, a click on the backdrop or the same hotkey: close.
//!
//! Pure layout/animation math here; the overlays live in `win` (one worker thread).
use crate::monitors::Rect;
use serde::Serialize;
use std::time::Duration;

/// Overlay window labels: `overview-0`, `overview-1`… (`capabilities/default.json`: `overview-*`).
pub const LABEL_PREFIX: &str = "overview-";
/// Logical px: margins of the tile area inside the monitor, gap between tiles (the title
/// strip under each tile lives in the gap) and the title strip height.
pub const MARGIN: f32 = 56.0;
pub const GAP: f32 = 44.0;
pub const LABEL_H: f32 = 30.0;
/// Thumbnails glide this long (Rust moves them in 16 ms steps).
pub const OPEN_MS: u64 = 150;
pub const CLOSE_MS: u64 = 120;
pub const FRAME: Duration = Duration::from_millis(16);
/// Esc / focus poll while open.
pub const POLL: Duration = Duration::from_millis(16);
/// A fresh overlay page has this long to load before the overview opens anyway.
pub const LOAD_TIMEOUT: Duration = Duration::from_millis(1500);
/// The pages have this long to confirm they rendered the tiles.
pub const ACK_TIMEOUT: Duration = Duration::from_millis(150);
/// Hidden this long → the overlays are destroyed (RAM budget).
pub const IDLE_DESTROY: Duration = Duration::from_secs(60);

/// Unknown source size (minimised, gone): assume a 16:9 window of this size.
const FALLBACK: (i32, i32) = (1600, 900);

/// Lays out one tile per window inside `area`: aspect ratios kept, never scaled above 1,
/// rows balanced (sizes differ by at most one, in input order), the row count chosen to
/// make the tiles as large as possible; rows and the whole block are centred.
pub fn layout_grid(windows: &[Rect], area: Rect, gap: i32) -> Vec<Rect> {
    let n = windows.len();
    if n == 0 || area.w <= 0 || area.h <= 0 {
        return vec![Rect { x: area.x, y: area.y, w: 0, h: 0 }; n];
    }
    let sizes: Vec<(f64, f64)> = windows
        .iter()
        .map(|r| if r.w > 0 && r.h > 0 { (r.w as f64, r.h as f64) } else { (FALLBACK.0 as f64, FALLBACK.1 as f64) })
        .collect();
    let gap = gap.max(0) as f64;
    let (aw, ah) = (area.w as f64, area.h as f64);
    let rows_of = |r: usize| -> Vec<std::ops::Range<usize>> {
        let (base, extra) = (n / r, n % r);
        let mut start = 0;
        (0..r)
            .map(|i| {
                let len = base + usize::from(i < extra);
                let range = start..start + len;
                start += len;
                range
            })
            .collect()
    };
    // Tile height of the whole grid for `r` rows, before the scale-1 cap.
    let height_for = |rows: &[std::ops::Range<usize>]| -> f64 {
        let by_h = (ah - gap * (rows.len() as f64 - 1.0)) / rows.len() as f64;
        rows.iter()
            .map(|row| {
                let aspect: f64 = sizes[row.clone()].iter().map(|(w, h)| w / h).sum();
                (aw - gap * (row.len() as f64 - 1.0)) / aspect
            })
            .fold(by_h, f64::min)
            .max(1.0)
    };
    let dims = |hgt: f64, i: usize| -> (f64, f64) {
        let (w, h) = sizes[i];
        let th = hgt.min(h);
        (th * w / h, th)
    };
    let mut best: Option<(f64, Vec<std::ops::Range<usize>>, f64)> = None;
    for r in 1..=n {
        let rows = rows_of(r);
        let hgt = height_for(&rows);
        let score: f64 = (0..n).map(|i| dims(hgt, i)).map(|(w, h)| w * h).sum();
        if best.as_ref().is_none_or(|b| score > b.0 + 0.5) {
            best = Some((score, rows, hgt));
        }
    }
    let Some((_, rows, hgt)) = best else { return Vec::new() };
    let row_h: Vec<f64> = rows.iter().map(|row| row.clone().map(|i| dims(hgt, i).1).fold(0.0, f64::max)).collect();
    let block_h = row_h.iter().sum::<f64>() + gap * (rows.len() as f64 - 1.0);
    let mut out = vec![Rect::default(); n];
    let mut y = area.y as f64 + ((ah - block_h) / 2.0).max(0.0);
    for (row, rh) in rows.iter().zip(&row_h) {
        let row_w = row.clone().map(|i| dims(hgt, i).0).sum::<f64>() + gap * (row.len() as f64 - 1.0);
        let mut x = area.x as f64 + ((aw - row_w) / 2.0).max(0.0);
        for i in row.clone() {
            let (w, h) = dims(hgt, i);
            let (w, h) = ((w.floor() as i32).clamp(1, area.w), (h.floor() as i32).clamp(1, area.h));
            let ty = y + (rh - h as f64) / 2.0;
            let rx = (x.round() as i32).min(area.x + area.w - w);
            let ry = (ty.round() as i32).min(area.y + area.h - h);
            out[i] = Rect { x: rx, y: ry, w, h };
            x += w as f64 + gap;
        }
        y += rh + gap;
    }
    out
}

/// Spatial reading order (top band first, then left to right): tiles keep roughly the
/// arrangement of the real windows. `band`: height of a band, px.
pub fn spatial_order(rects: &[Rect], band: i32) -> Vec<usize> {
    let band = band.max(1);
    let mut idx: Vec<usize> = (0..rects.len()).collect();
    idx.sort_by_key(|&i| {
        let r = rects[i];
        ((r.y + r.h / 2).div_euclid(band), r.x + r.w / 2)
    });
    idx
}

/// The window belongs to the monitor holding its centre.
pub fn on_monitor(window: Rect, monitor: Rect) -> bool {
    monitor.contains(window.x + window.w / 2, window.y + window.h / 2)
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    let l = |x: i32, y: i32| (x as f32 + (y - x) as f32 * t).round() as i32;
    Rect { x: l(a.x, b.x), y: l(a.y, b.y), w: l(a.w, b.w), h: l(a.h, b.h) }
}

/// One tile of `overview://show`.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OverviewTile {
    pub hwnd: isize,
    pub title: String,
    pub app_name: String,
    pub icon_url: String,
    /// Physical px relative to the overlay window.
    pub rect: Rect,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OverviewShow {
    pub monitor_id: String,
    pub tiles: Vec<OverviewTile>,
    /// Monitor scale (physical px per CSS px).
    pub scale: f32,
    /// No monitor has any window: the page says so.
    pub empty_hint: bool,
}

/// Opens the overview, or closes it when open. Never blocks.
pub fn toggle(app: &tauri::AppHandle) {
    #[cfg(windows)]
    win::send(app, win::Msg::Toggle);
    #[cfg(not(windows))]
    let _ = app;
}

/// Installs the hotkey handler. From `run()` setup.
pub fn init(app: &tauri::AppHandle) {
    let a = app.clone();
    crate::hotkey::set_handler(crate::hotkey::HotkeyId::Overview, move |_| toggle(&a));
}

/// A tile was clicked: that window comes to the front, the overview closes.
#[tauri::command]
pub fn overview_pick(app: tauri::AppHandle, hwnd: isize) {
    #[cfg(windows)]
    win::send(&app, win::Msg::Pick(hwnd));
    #[cfg(not(windows))]
    let _ = (app, hwnd);
}

/// Échap or a click on the backdrop.
#[tauri::command]
pub fn overview_close(app: tauri::AppHandle) {
    #[cfg(windows)]
    win::send(&app, win::Msg::Close);
    #[cfg(not(windows))]
    let _ = app;
}

/// An overlay page is loaded and listening, or has rendered the last `overview://show`.
#[tauri::command]
pub fn overview_ready(app: tauri::AppHandle, window: tauri::WebviewWindow) {
    #[cfg(windows)]
    win::send(&app, win::Msg::Ready(window.label().to_string()));
    #[cfg(not(windows))]
    let _ = (app, window);
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::dock_host::set_pos;
    use crate::monitors::{self, MonitorInfo};
    use crate::switcher::win::{activate_own, cursor, esc_down, focus_window, foreground_root, make_tool_window};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::OnceLock;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::time::Instant;
    use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{
        DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE, DWM_TNP_VISIBLE,
        DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail,
        DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, HWND_TOPMOST, IsIconic, IsWindowVisible, SWP_HIDEWINDOW, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SWP_SHOWWINDOW,
    };

    pub enum Msg {
        Toggle,
        Pick(isize),
        Close,
        Ready(String),
    }

    static TX: OnceLock<Sender<Msg>> = OnceLock::new();

    /// Starts the worker on first use. Only pushes to a channel: never blocks.
    pub fn send(app: &AppHandle, msg: Msg) {
        let tx = TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            let app = app.clone();
            if let Err(e) = std::thread::Builder::new().name("lucent-overview".into()).spawn(move || Host::new(app).run(rx)) {
                eprintln!("lucent: cannot spawn overview thread: {e}");
            }
            tx
        });
        let _ = tx.send(msg);
    }

    /// A window shown in the overview.
    struct Source {
        hwnd: isize,
        title: String,
        app_name: String,
        icon_url: String,
        /// Visible frame (no invisible resize borders), screen px.
        frame: Rect,
        /// Whole window rect, screen px.
        window: Rect,
    }

    struct Thumb {
        id: isize,
        hwnd: isize,
        /// Overlay-relative: the real window place, and the grid place.
        from: Rect,
        to: Rect,
        /// Part of the source window to show (its visible frame), source coordinates.
        source: Option<RECT>,
    }

    struct Overlay {
        monitor: MonitorInfo,
        label: String,
        window: WebviewWindow,
        hwnd: isize,
        loaded: bool,
        acked: bool,
        thumbs: Vec<Thumb>,
    }

    impl Overlay {
        fn hwnd(&self) -> HWND {
            HWND(self.hwnd as *mut _)
        }
    }

    #[derive(PartialEq)]
    enum Phase {
        /// Fresh pages loading (since).
        Loading(Instant),
        /// Payload sent, waiting for the render acks (since).
        Acking(Instant),
        Open,
    }

    struct Session {
        phase: Phase,
        prev_fg: isize,
        took_focus: bool,
        esc: bool,
    }

    struct Host {
        app: AppHandle,
        overlays: Vec<Overlay>,
        session: Option<Session>,
        hidden_since: Option<Instant>,
        next_label: usize,
    }

    impl Host {
        fn new(app: AppHandle) -> Self {
            Self { app, overlays: Vec::new(), session: None, hidden_since: None, next_label: 0 }
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
                self.tick();
            }
        }

        fn handle(&mut self, m: Msg) {
            match m {
                Msg::Toggle if self.session.is_some() => self.close(None, true),
                Msg::Toggle => self.open(),
                Msg::Close => self.close(None, true),
                Msg::Pick(h) => {
                    let ours = self.overlays.iter().any(|o| o.thumbs.iter().any(|t| t.hwnd == h));
                    if ours {
                        self.close(Some(h), false);
                    }
                }
                Msg::Ready(label) => {
                    let acking = matches!(self.session.as_ref().map(|s| &s.phase), Some(Phase::Acking(_)));
                    if let Some(o) = self.overlays.iter_mut().find(|o| o.label == label) {
                        if !o.loaded {
                            o.loaded = true;
                        } else if acking {
                            o.acked = true;
                        }
                    }
                }
            }
        }

        fn tick(&mut self) {
            let Some(s) = &mut self.session else {
                if self.hidden_since.is_some_and(|t| t.elapsed() >= IDLE_DESTROY) {
                    self.destroy_all();
                }
                return;
            };
            match s.phase {
                Phase::Loading(t) => {
                    if self.overlays.iter().all(|o| o.loaded) || t.elapsed() >= LOAD_TIMEOUT {
                        self.prepare();
                    }
                }
                Phase::Acking(t) => {
                    if self.overlays.iter().all(|o| o.acked) || t.elapsed() >= ACK_TIMEOUT {
                        self.reveal();
                    }
                }
                Phase::Open => {
                    let esc = esc_down();
                    let pressed = esc && !s.esc;
                    s.esc = esc;
                    if pressed {
                        return self.close(None, true);
                    }
                    // Something else took the foreground (Alt+Tab, Win key, a notification
                    // clicked): get out of the way.
                    let fg = foreground_root();
                    let prev = s.prev_fg;
                    if fg != 0 && fg != prev && !self.overlays.iter().any(|o| o.hwnd == fg) {
                        self.close(None, false);
                    }
                }
            }
        }

        fn open(&mut self) {
            let g = crate::game::service::state();
            if g.active && g.focused {
                return;
            }
            let mons = monitors::win::enumerate();
            if mons.is_empty() {
                return;
            }
            self.reconcile(&mons);
            if self.overlays.is_empty() {
                return;
            }
            self.hidden_since = None;
            self.session = Some(Session {
                phase: Phase::Loading(Instant::now()),
                prev_fg: foreground_root(),
                took_focus: false,
                esc: esc_down(),
            });
            self.tick();
        }

        /// One overlay per monitor: kept by monitor id, created or destroyed as needed.
        fn reconcile(&mut self, mons: &[MonitorInfo]) {
            let mut keep: Vec<Overlay> = Vec::new();
            for o in std::mem::take(&mut self.overlays) {
                match mons.iter().find(|m| m.id == o.monitor.id) {
                    Some(m) => keep.push(Overlay { monitor: m.clone(), ..o }),
                    None => destroy(o),
                }
            }
            for m in mons {
                if keep.iter().any(|o| o.monitor.id == m.id) {
                    continue;
                }
                let label = format!("{LABEL_PREFIX}{}", self.next_label);
                self.next_label += 1;
                match create(&self.app, &label) {
                    Ok((window, hwnd)) => keep.push(Overlay {
                        monitor: m.clone(),
                        label,
                        window,
                        hwnd,
                        loaded: false,
                        acked: false,
                        thumbs: Vec::new(),
                    }),
                    Err(e) => eprintln!("lucent: cannot create an overview overlay: {e}"),
                }
            }
            self.overlays = keep;
        }

        /// Visible, non-minimised dock windows with their app name and icon.
        fn sources(&self) -> Vec<Source> {
            let Some(tracker) = self.app.try_state::<Arc<crate::wintrack::WindowTracker>>() else { return Vec::new() };
            let items = crate::current_items(&self.app);
            let mut meta: HashMap<isize, (String, String)> = HashMap::new();
            for i in &items {
                for w in &i.windows {
                    meta.insert(w.hwnd, (i.name.clone(), i.icon_url.clone()));
                }
            }
            tracker
                .snapshot()
                .into_iter()
                .filter_map(|w| {
                    let h = HWND(w.hwnd as *mut _);
                    if unsafe { IsIconic(h).as_bool() || !IsWindowVisible(h).as_bool() } {
                        return None;
                    }
                    let mut wr = RECT::default();
                    unsafe { GetWindowRect(h, &mut wr) }.ok()?;
                    let window = to_rect(wr);
                    let mut fr = RECT::default();
                    let frame = match unsafe {
                        DwmGetWindowAttribute(
                            h,
                            DWMWA_EXTENDED_FRAME_BOUNDS,
                            &mut fr as *mut _ as _,
                            std::mem::size_of::<RECT>() as u32,
                        )
                    } {
                        Ok(()) => to_rect(fr),
                        Err(_) => window,
                    };
                    if frame.w <= 0 || frame.h <= 0 {
                        return None;
                    }
                    let (app_name, icon_url) = meta.get(&w.hwnd).cloned().unwrap_or_default();
                    Some(Source { hwnd: w.hwnd, title: w.title, app_name, icon_url, frame, window })
                })
                .collect()
        }

        /// Sizes the (hidden) overlays, registers and lays out the thumbnails, sends the tiles.
        fn prepare(&mut self) {
            let sources = self.sources();
            let mut payloads: Vec<(String, OverviewShow)> = Vec::new();
            for o in &mut self.overlays {
                let m = o.monitor.clone();
                for _ in 0..2 {
                    set_pos(o.hwnd(), Some(HWND_TOPMOST), m.rect, SWP_HIDEWINDOW);
                }
                let mine: Vec<&Source> = sources.iter().filter(|s| on_monitor(s.frame, m.rect)).collect();
                let frames: Vec<Rect> = mine.iter().map(|s| s.frame).collect();
                let order = spatial_order(&frames, (m.rect.h / 3).max(1));
                let px = |v: f32| (v * m.scale).round() as i32;
                let area = Rect {
                    x: px(MARGIN),
                    y: px(MARGIN),
                    w: m.rect.w - 2 * px(MARGIN),
                    h: m.rect.h - 2 * px(MARGIN) - px(LABEL_H),
                };
                let ordered: Vec<&Source> = order.iter().map(|&i| mine[i]).collect();
                let grid = layout_grid(&ordered.iter().map(|s| s.frame).collect::<Vec<_>>(), area, px(GAP));
                let mut tiles = Vec::new();
                for (src, to) in ordered.iter().zip(grid) {
                    let Ok(id) = (unsafe { DwmRegisterThumbnail(o.hwnd(), HWND(src.hwnd as *mut _)) }) else { continue };
                    // Crop the invisible resize borders, unless DWM sees another size
                    // (DPI-virtualised window): then show it whole.
                    let size = unsafe { DwmQueryThumbnailSourceSize(id) }.map_or((0, 0), |s| (s.cx, s.cy));
                    let source = (size == (src.window.w, src.window.h)).then(|| RECT {
                        left: src.frame.x - src.window.x,
                        top: src.frame.y - src.window.y,
                        right: src.frame.x - src.window.x + src.frame.w,
                        bottom: src.frame.y - src.window.y + src.frame.h,
                    });
                    let from = Rect { x: src.frame.x - m.rect.x, y: src.frame.y - m.rect.y, ..src.frame };
                    let thumb = Thumb { id, hwnd: src.hwnd, from, to, source };
                    update(&thumb, from, false);
                    o.thumbs.push(thumb);
                    tiles.push(OverviewTile {
                        hwnd: src.hwnd,
                        title: src.title.clone(),
                        app_name: src.app_name.clone(),
                        icon_url: src.icon_url.clone(),
                        rect: to,
                    });
                }
                o.acked = false;
                payloads.push((o.label.clone(), OverviewShow { monitor_id: m.id.clone(), tiles, scale: m.scale, empty_hint: false }));
            }
            let empty = payloads.iter().all(|(_, p)| p.tiles.is_empty());
            for (label, mut payload) in payloads {
                payload.empty_hint = empty;
                if let Err(e) = self.app.emit_to(label.as_str(), "overview://show", payload) {
                    eprintln!("lucent: cannot emit overview://show: {e}");
                }
            }
            if let Some(s) = &mut self.session {
                s.phase = Phase::Acking(Instant::now());
            }
        }

        /// Shows the overlays, takes the foreground, glides the thumbnails into the grid.
        fn reveal(&mut self) {
            for o in &self.overlays {
                for _ in 0..2 {
                    set_pos(o.hwnd(), Some(HWND_TOPMOST), o.monitor.rect, SWP_SHOWWINDOW);
                }
                for t in &o.thumbs {
                    update(t, t.from, true);
                }
            }
            // Échap and clicks go to us rather than to the app below.
            let pt = cursor();
            let target = self
                .overlays
                .iter()
                .find(|o| pt.is_some_and(|(x, y)| o.monitor.rect.contains(x, y)))
                .or_else(|| self.overlays.first());
            if let Some(o) = target {
                activate_own(&o.window, o.hwnd);
            }
            self.animate(false, OPEN_MS);
            if let Some(s) = &mut self.session {
                s.phase = Phase::Open;
                s.took_focus = target.is_some();
            }
        }

        /// Glides every thumbnail grid → real place (`back`) or real place → grid
        /// (jumps when « Réduire les animations » is on).
        fn animate(&self, back: bool, ms: u64) {
            let reduce = self
                .app
                .try_state::<crate::settings::SettingsStore>()
                .is_some_and(|s| s.get().animations.reduce_motion);
            let ms = if reduce { 0 } else { ms };
            let start = Instant::now();
            let total = Duration::from_millis(ms);
            loop {
                let t = if ms == 0 { 1.0 } else { (start.elapsed().as_secs_f32() / total.as_secs_f32()).min(1.0) };
                let k = ease_out_cubic(t);
                for o in &self.overlays {
                    for th in &o.thumbs {
                        let (a, b) = if back { (th.to, th.from) } else { (th.from, th.to) };
                        update(th, lerp_rect(a, b, k), true);
                    }
                }
                if t >= 1.0 {
                    return;
                }
                std::thread::sleep(FRAME.min(total.saturating_sub(start.elapsed())).max(Duration::from_millis(1)));
            }
        }

        /// Ends the overview: `target` comes to the front (else, with `restore`, the window
        /// that was in front gets the focus back), thumbnails glide back, overlays hide.
        fn close(&mut self, target: Option<isize>, restore: bool) {
            let Some(s) = self.session.take() else { return };
            let open = s.phase == Phase::Open;
            if open || matches!(s.phase, Phase::Acking(_)) {
                for o in &self.overlays {
                    if let Err(e) = self.app.emit_to(o.label.as_str(), "overview://hide", ()) {
                        eprintln!("lucent: cannot emit overview://hide: {e}");
                    }
                }
            }
            match target {
                Some(h) => focus_window(h),
                None if restore && s.took_focus && s.prev_fg != 0 => focus_window(s.prev_fg),
                None => {}
            }
            if open {
                self.animate(true, CLOSE_MS);
            }
            for o in &mut self.overlays {
                set_pos(o.hwnd(), None, Rect::default(), SWP_NOZORDER | SWP_NOMOVE | SWP_NOSIZE | SWP_HIDEWINDOW);
                for t in o.thumbs.drain(..) {
                    unsafe {
                        let _ = DwmUnregisterThumbnail(t.id);
                    }
                }
            }
            self.hidden_since = Some(Instant::now());
        }

        fn destroy_all(&mut self) {
            self.hidden_since = None;
            for o in std::mem::take(&mut self.overlays) {
                destroy(o);
            }
        }
    }

    fn to_rect(r: RECT) -> Rect {
        Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
    }

    fn update(t: &Thumb, dest: Rect, visible: bool) {
        let mut props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY,
            rcDestination: RECT { left: dest.x, top: dest.y, right: dest.x + dest.w, bottom: dest.y + dest.h },
            opacity: 255,
            fVisible: visible.into(),
            ..Default::default()
        };
        if let Some(src) = t.source {
            props.dwFlags |= DWM_TNP_RECTSOURCE;
            props.rcSource = src;
        }
        unsafe {
            let _ = DwmUpdateThumbnailProperties(t.id, &props);
        }
    }

    fn create(app: &AppHandle, label: &str) -> tauri::Result<(WebviewWindow, isize)> {
        let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("overview.html".into()))
            .title("Lucent overview")
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
        Ok((window, hwnd))
    }

    fn destroy(o: Overlay) {
        for t in &o.thumbs {
            unsafe {
                let _ = DwmUnregisterThumbnail(t.id);
            }
        }
        if let Err(e) = o.window.destroy() {
            eprintln!("lucent: cannot destroy an overview overlay: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 50, y: 40, w: 1800, h: 960 };

    fn r(w: i32, h: i32) -> Rect {
        Rect { x: 0, y: 0, w, h }
    }

    fn inside(t: &Rect, a: &Rect) -> bool {
        t.x >= a.x && t.y >= a.y && t.x + t.w <= a.x + a.w && t.y + t.h <= a.y + a.h
    }

    fn aspect_kept(t: &Rect, src: &Rect) -> bool {
        // Within one pixel of rounding on either side.
        let want = src.w as f64 / src.h as f64;
        let lo = (t.w as f64 - 1.0) / (t.h as f64 + 1.0);
        let hi = (t.w as f64 + 1.0) / (t.h as f64 - 1.0).max(1.0);
        lo <= want && want <= hi
    }

    fn overlap(a: &Rect, b: &Rect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    fn check(srcs: &[Rect], gap: i32) -> Vec<Rect> {
        let out = layout_grid(srcs, AREA, gap);
        assert_eq!(out.len(), srcs.len());
        for (t, s) in out.iter().zip(srcs) {
            assert!(inside(t, &AREA), "{t:?} outside {AREA:?}");
            assert!(t.w > 0 && t.h > 0);
            assert!(aspect_kept(t, s), "{t:?} vs {s:?}");
            assert!(t.w <= s.w && t.h <= s.h, "never upscaled: {t:?} vs {s:?}");
        }
        for i in 0..out.len() {
            for j in i + 1..out.len() {
                assert!(!overlap(&out[i], &out[j]), "{:?} overlaps {:?}", out[i], out[j]);
            }
        }
        out
    }

    fn row_sizes(out: &[Rect]) -> Vec<usize> {
        let mut rows: Vec<(i32, usize)> = Vec::new();
        for t in out {
            let cy = t.y + t.h / 2;
            match rows.iter_mut().find(|(y, _)| (y - cy).abs() < 20) {
                Some(r) => r.1 += 1,
                None => rows.push((cy, 1)),
            }
        }
        rows.into_iter().map(|r| r.1).collect()
    }

    #[test]
    fn one_small_window_keeps_its_size_and_is_centred() {
        let out = check(&[r(800, 600)], 40);
        assert_eq!((out[0].w, out[0].h), (800, 600), "max scale 1");
        assert_eq!(out[0].x, AREA.x + (AREA.w - 800) / 2);
        assert_eq!(out[0].y, AREA.y + (AREA.h - 600) / 2);
    }

    #[test]
    fn one_large_window_shrinks_to_fit() {
        let out = check(&[r(3840, 2160)], 40);
        assert!(out[0].w <= AREA.w && out[0].h <= AREA.h);
        assert!(out[0].w >= AREA.w - 2 || out[0].h >= AREA.h - 2, "fills the area: {:?}", out[0]);
    }

    #[test]
    fn two_windows_side_by_side() {
        let out = check(&[r(1920, 1080), r(1920, 1080)], 40);
        assert_eq!(row_sizes(&out), vec![2]);
        assert!(out[0].x < out[1].x, "input order kept");
        assert!(out[1].x - (out[0].x + out[0].w) >= 40, "gap kept");
    }

    #[test]
    fn five_windows_in_balanced_rows() {
        let srcs = [r(1920, 1080), r(1280, 1024), r(800, 1200), r(1600, 900), r(1000, 700)];
        let out = check(&srcs, 40);
        let rows = row_sizes(&out);
        assert!(rows.len() >= 2, "{rows:?}");
        let (lo, hi) = (rows.iter().min().unwrap(), rows.iter().max().unwrap());
        assert!(hi - lo <= 1, "balanced: {rows:?}");
    }

    #[test]
    fn nine_windows_three_rows_of_three() {
        let out = check(&[r(1920, 1080); 9], 40);
        assert_eq!(row_sizes(&out), vec![3, 3, 3]);
    }

    #[test]
    fn mixed_and_degenerate_sizes_do_not_panic() {
        check(&[r(0, 0), r(300, 0), r(10000, 50), r(50, 4000)].map(|s| if s.w > 0 && s.h > 0 { s } else { r(1600, 900) }), 40);
        let out = layout_grid(&[r(0, 0)], AREA, 40);
        assert!(out[0].w > 0 && inside(&out[0], &AREA));
        assert!(layout_grid(&[], AREA, 40).is_empty());
        let tiny = layout_grid(&[r(100, 100)], Rect { x: 0, y: 0, w: 0, h: 0 }, 40);
        assert_eq!(tiny.len(), 1);
    }

    #[test]
    fn spatial_order_reads_bands_left_to_right() {
        let rects = [
            Rect { x: 1000, y: 600, w: 400, h: 300 }, // bottom right
            Rect { x: 900, y: 50, w: 400, h: 300 },   // top right
            Rect { x: 10, y: 600, w: 400, h: 300 },   // bottom left
            Rect { x: 0, y: 80, w: 400, h: 300 },     // top left
        ];
        assert_eq!(spatial_order(&rects, 360), vec![3, 1, 2, 0]);
    }

    #[test]
    fn window_belongs_to_monitor_of_its_centre() {
        let left = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let right = Rect { x: 1920, y: 0, w: 2560, h: 1440 };
        let w = Rect { x: 1500, y: 100, w: 1000, h: 600 };
        assert!(!on_monitor(w, left));
        assert!(on_monitor(w, right));
    }

    #[test]
    fn easing_and_lerp() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(2.0), 1.0);
        assert!(ease_out_cubic(0.5) > 0.5, "decelerates");
        let a = Rect { x: 0, y: 0, w: 100, h: 100 };
        let b = Rect { x: 100, y: 200, w: 300, h: 50 };
        assert_eq!(lerp_rect(a, b, 0.0), a);
        assert_eq!(lerp_rect(a, b, 1.0), b);
        assert_eq!(lerp_rect(a, b, 0.5), Rect { x: 50, y: 100, w: 200, h: 75 });
    }
}
