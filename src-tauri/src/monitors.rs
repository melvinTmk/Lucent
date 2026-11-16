//! Monitor geometry: which monitors get a dock, where the dock sits, where the reveal
//! trigger strip is. Everything here is pure except the `win` submodule (enumeration).
//! All coordinates are physical pixels in virtual-screen space.
use crate::settings::{DockPosition, MonitorSelection};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }

    /// `self` covers all of `o` (a fullscreen or borderless window over a monitor).
    pub fn covers(&self, o: &Rect) -> bool {
        self.x <= o.x && self.y <= o.y && self.x + self.w >= o.x + o.w && self.y + self.h >= o.y + o.h
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct MonitorInfo {
    /// Device name, e.g. `\\.\DISPLAY1`.
    pub id: String,
    pub rect: Rect,
    pub work: Rect,
    pub scale: f32,
    pub primary: bool,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct MonitorDiff {
    pub create: Vec<MonitorInfo>,
    pub update: Vec<MonitorInfo>,
    pub destroy: Vec<String>,
}

/// Logical margin between the dock and the screen edge.
const EDGE_MARGIN: f32 = 8.0;
/// Physical height (or width) of the reveal trigger strip.
const TRIGGER_PX: i32 = 2;

pub fn selected(all: &[MonitorInfo], sel: &MonitorSelection) -> Vec<MonitorInfo> {
    match sel {
        MonitorSelection::All => all.to_vec(),
        MonitorSelection::Primary => all.iter().filter(|m| m.primary).cloned().collect(),
        MonitorSelection::List(ids) => all.iter().filter(|m| ids.contains(&m.id)).cloned().collect(),
    }
}

/// Monitors are matched by device id. A monitor whose geometry, scale or primary flag
/// changed is an update (its window is moved), not a destroy + create.
pub fn monitor_diff(old: &[MonitorInfo], new: &[MonitorInfo]) -> MonitorDiff {
    let mut d = MonitorDiff::default();
    for n in new {
        match old.iter().find(|o| o.id == n.id) {
            None => d.create.push(n.clone()),
            Some(o) if o != n => d.update.push(n.clone()),
            Some(_) => {}
        }
    }
    d.destroy = old.iter().filter(|o| !new.iter().any(|n| n.id == o.id)).map(|o| o.id.clone()).collect();
    d
}

fn margin(m: &MonitorInfo) -> i32 {
    (EDGE_MARGIN * m.scale).round() as i32
}

/// Dock window rect: centered along the edge, `EDGE_MARGIN` logical px away from it,
/// always fully inside the monitor (content larger than the monitor is clamped).
pub fn dock_rect(m: &MonitorInfo, pos: DockPosition, content_w: u32, content_h: u32) -> Rect {
    let r = m.rect;
    let w = (content_w.min(i32::MAX as u32) as i32).min(r.w);
    let h = (content_h.min(i32::MAX as u32) as i32).min(r.h);
    let mg = margin(m);
    let (x, y) = match pos {
        DockPosition::Bottom => (r.x + (r.w - w) / 2, r.y + r.h - h - mg),
        DockPosition::Left => (r.x + mg, r.y + (r.h - h) / 2),
        DockPosition::Right => (r.x + r.w - w - mg, r.y + (r.h - h) / 2),
    };
    Rect { x: x.clamp(r.x, r.x + r.w - w), y: y.clamp(r.y, r.y + r.h - h), w, h }
}

/// Where a hidden dock window is parked: same size, just past the edge.
pub fn hidden_rect(m: &MonitorInfo, pos: DockPosition, dock: Rect) -> Rect {
    let r = m.rect;
    match pos {
        DockPosition::Bottom => Rect { y: r.y + r.h, ..dock },
        DockPosition::Left => Rect { x: r.x - dock.w, ..dock },
        DockPosition::Right => Rect { x: r.x + r.w, ..dock },
    }
}

/// How a hidden dock is put away.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Park {
    /// Move the window to this rect, just past the edge (outside every monitor).
    OffEdge(Rect),
    /// The off-edge rect would land on another monitor: hide the window where it is.
    /// It must never cross onto a neighbour (mixed DPI would trigger WM_DPICHANGED).
    HideInPlace,
}

pub fn park_strategy(m: &MonitorInfo, all: &[MonitorInfo], shown: Rect, pos: DockPosition) -> Park {
    let r = hidden_rect(m, pos, shown);
    if all.iter().any(|o| o.id != m.id && o.rect.intersects(&r)) { Park::HideInPlace } else { Park::OffEdge(r) }
}

/// `TRIGGER_PX` physical strip along the whole dock edge.
pub fn trigger_zone(m: &MonitorInfo, pos: DockPosition) -> Rect {
    let r = m.rect;
    match pos {
        DockPosition::Bottom => Rect { x: r.x, y: r.y + r.h - TRIGGER_PX, w: r.w, h: TRIGGER_PX },
        DockPosition::Left => Rect { x: r.x, y: r.y, w: TRIGGER_PX, h: r.h },
        DockPosition::Right => Rect { x: r.x + r.w - TRIGGER_PX, y: r.y, w: TRIGGER_PX, h: r.h },
    }
}

/// Live enumeration of the attached monitors (physical px; the process is per-monitor
/// DPI aware v2 through Tauri, so `GetMonitorInfoW` reports physical coordinates).
#[cfg(windows)]
pub mod win {
    use super::{MonitorInfo, Rect};
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
    use windows::core::BOOL;

    fn rect(r: RECT) -> Rect {
        Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
    }

    unsafe extern "system" fn collect(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let out = unsafe { &mut *(data.0 as *mut Vec<MonitorInfo>) };
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(h, &mut mi.monitorInfo as *mut MONITORINFO) }.as_bool() {
            let (mut dx, mut dy) = (96u32, 96u32);
            if unsafe { GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.is_err() {
                dx = 96;
            }
            let len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
            out.push(MonitorInfo {
                id: String::from_utf16_lossy(&mi.szDevice[..len]),
                rect: rect(mi.monitorInfo.rcMonitor),
                work: rect(mi.monitorInfo.rcWork),
                scale: dx as f32 / 96.0,
                primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        BOOL(1)
    }

    pub fn enumerate() -> Vec<MonitorInfo> {
        let mut out: Vec<MonitorInfo> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut out as *mut _ as isize));
        }
        out
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn enumerate_finds_one_primary() {
            let all = super::enumerate();
            assert!(!all.is_empty());
            assert_eq!(all.iter().filter(|m| m.primary).count(), 1);
            assert!(all.iter().all(|m| m.id.starts_with(r"\\.\") && m.scale >= 1.0));
        }
    }
}

/// Rect of the foreground window when it may be a fullscreen app: frameless and not
/// maximized, not the desktop, not the shell, not Lucent. The caller checks it against each monitor.
#[cfg(windows)]
pub fn fullscreen_foreground_rect() -> Option<Rect> {
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId};
    unsafe {
        let h = GetForegroundWindow();
        if h.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == std::process::id() {
            return None;
        }
        let mut buf = [0u16; 64];
        let n = GetClassNameW(h, &mut buf) as usize;
        let class = String::from_utf16_lossy(&buf[..n]);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return None;
        }
        // With the taskbar hidden, a maximized window also spans the whole monitor: only
        // frameless, non-maximized windows (games, F11, video players) count.
        use windows::Win32::UI::WindowsAndMessaging::{GWL_STYLE, GetWindowLongW, IsZoomed, WS_CAPTION};
        let style = GetWindowLongW(h, GWL_STYLE) as u32;
        if IsZoomed(h).as_bool() || style & WS_CAPTION.0 == WS_CAPTION.0 {
            return None;
        }
        let mut r = windows::Win32::Foundation::RECT::default();
        GetWindowRect(h, &mut r).ok()?;
        Some(Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(id: &str, x: i32, w: i32, h: i32, scale: f32, primary: bool) -> MonitorInfo {
        let rect = Rect { x, y: 0, w, h };
        MonitorInfo { id: id.into(), rect, work: rect, scale, primary }
    }

    #[test]
    fn selected_all_keeps_every_monitor() {
        let all = [mon("A", 0, 1920, 1080, 1.0, true), mon("B", 1920, 1920, 1080, 1.0, false)];
        assert_eq!(selected(&all, &MonitorSelection::All), all.to_vec());
    }

    #[test]
    fn selected_primary_only() {
        let all = [mon("A", 0, 1920, 1080, 1.0, false), mon("B", 1920, 1920, 1080, 1.0, true)];
        let s = selected(&all, &MonitorSelection::Primary);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].id, "B");
    }

    #[test]
    fn selected_list_ignores_unknown_ids() {
        let all = [mon("A", 0, 1920, 1080, 1.0, true), mon("B", 1920, 1920, 1080, 1.0, false)];
        let sel = MonitorSelection::List(vec!["B".into(), "GONE".into()]);
        let s = selected(&all, &sel);
        assert_eq!(s.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["B"]);
    }

    #[test]
    fn monitor_diff_creates_moves_and_destroys() {
        let a = mon("A", 0, 1920, 1080, 1.0, true);
        let b = mon("B", 1920, 1920, 1080, 1.0, false);
        let a_moved = mon("A", 0, 2560, 1440, 1.25, true);
        let c = mon("C", 2560, 1920, 1080, 1.0, false);
        let d = monitor_diff(&[a, b], &[a_moved.clone(), c.clone()]);
        assert_eq!(d.update, vec![a_moved]);
        assert_eq!(d.create, vec![c]);
        assert_eq!(d.destroy, vec!["B".to_string()]);
    }

    #[test]
    fn monitor_diff_unchanged_is_empty() {
        let a = mon("A", 0, 1920, 1080, 1.0, true);
        assert_eq!(monitor_diff(&[a.clone()], &[a]), MonitorDiff::default());
    }

    #[test]
    fn dock_rect_bottom_centered_with_margin_scaled() {
        let m = mon("A", 0, 1920, 1080, 1.5, true);
        let r = dock_rect(&m, DockPosition::Bottom, 600, 80);
        assert_eq!(r, Rect { x: 660, y: 1080 - 80 - 12, w: 600, h: 80 });
    }

    #[test]
    fn dock_rect_offset_monitor_and_sides() {
        let m = mon("B", 1920, 1920, 1080, 1.0, false);
        assert_eq!(dock_rect(&m, DockPosition::Left, 80, 600), Rect { x: 1928, y: 240, w: 80, h: 600 });
        assert_eq!(
            dock_rect(&m, DockPosition::Right, 80, 600),
            Rect { x: 1920 + 1920 - 80 - 8, y: 240, w: 80, h: 600 }
        );
    }

    #[test]
    fn dock_rect_never_outside_monitor() {
        let m = mon("A", 100, 1280, 720, 1.0, true);
        let r = dock_rect(&m, DockPosition::Bottom, 5000, 2000);
        assert_eq!(r, m.rect);
    }

    #[test]
    fn hidden_rect_parks_just_past_the_edge() {
        let m = mon("A", 0, 1920, 1080, 1.0, true);
        let d = dock_rect(&m, DockPosition::Bottom, 600, 80);
        assert_eq!(hidden_rect(&m, DockPosition::Bottom, d), Rect { y: 1080, ..d });
        let d = dock_rect(&m, DockPosition::Left, 80, 600);
        assert_eq!(hidden_rect(&m, DockPosition::Left, d), Rect { x: -80, ..d });
        let d = dock_rect(&m, DockPosition::Right, 80, 600);
        assert_eq!(hidden_rect(&m, DockPosition::Right, d), Rect { x: 1920, ..d });
        assert!(!hidden_rect(&m, DockPosition::Bottom, d).intersects(&m.rect));
    }

    #[test]
    fn park_hides_in_place_with_neighbour_below() {
        let top = mon("A", 0, 1920, 1080, 1.0, true);
        let below = MonitorInfo { rect: Rect { x: 0, y: 1080, w: 2560, h: 1440 }, scale: 1.5, ..mon("B", 0, 0, 0, 1.5, false) };
        let shown = dock_rect(&top, DockPosition::Bottom, 600, 80);
        assert_eq!(park_strategy(&top, &[top.clone(), below], shown, DockPosition::Bottom), Park::HideInPlace);
    }

    #[test]
    fn park_off_edge_outside_every_monitor_without_neighbour() {
        let a = mon("A", 0, 1920, 1080, 1.0, true);
        let b = mon("B", 1920, 1920, 1080, 1.0, false); // side by side: nothing below
        let all = [a.clone(), b];
        for (pos, w, h) in [(DockPosition::Bottom, 600, 80), (DockPosition::Left, 80, 600)] {
            let shown = dock_rect(&a, pos, w, h);
            match park_strategy(&a, &all, shown, pos) {
                Park::OffEdge(r) => {
                    assert_eq!(r, hidden_rect(&a, pos, shown));
                    assert!(all.iter().all(|m| !m.rect.intersects(&r)));
                }
                Park::HideInPlace => panic!("expected OffEdge for {pos:?}"),
            }
        }
    }

    #[test]
    fn park_hides_in_place_with_diagonal_neighbour() {
        let a = mon("A", 0, 1920, 1080, 1.0, true);
        // Below-right, overlapping the parked rect's horizontal span only partly.
        let diag = MonitorInfo { rect: Rect { x: 1200, y: 1080, w: 1920, h: 1080 }, ..mon("C", 0, 0, 0, 1.25, false) };
        let shown = dock_rect(&a, DockPosition::Bottom, 600, 80); // x 660..1260
        assert_eq!(park_strategy(&a, &[a.clone(), diag], shown, DockPosition::Bottom), Park::HideInPlace);
    }

    #[test]
    fn covers_needs_the_whole_monitor() {
        let m = Rect { x: 1920, y: 0, w: 1920, h: 1080 };
        assert!(m.covers(&m));
        assert!(Rect { x: 1912, y: -8, w: 1936, h: 1096 }.covers(&m), "overhanging frameless window");
        assert!(!Rect { x: 1920, y: 0, w: 1920, h: 1040 }.covers(&m), "maximized above a taskbar");
        assert!(!Rect { x: 0, y: 0, w: 1920, h: 1080 }.covers(&m), "other monitor");
    }

    #[test]
    fn trigger_zone_is_two_px_strip_on_edge() {
        let m = mon("B", 1920, 1920, 1080, 1.5, false);
        assert_eq!(trigger_zone(&m, DockPosition::Bottom), Rect { x: 1920, y: 1078, w: 1920, h: 2 });
        assert_eq!(trigger_zone(&m, DockPosition::Left), Rect { x: 1920, y: 0, w: 2, h: 1080 });
        assert_eq!(trigger_zone(&m, DockPosition::Right), Rect { x: 3838, y: 0, w: 2, h: 1080 });
    }
}
