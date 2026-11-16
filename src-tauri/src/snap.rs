//! Rectangle-style window snapping (`Ctrl+Alt+arrows`, `Ctrl+Alt+D/F/G`, move to the
//! previous/next monitor). The geometry is pure (physical pixels, virtual-screen space);
//! the `win` submodule applies it to the foreground window on its own worker thread
//! (hotkey handlers only push to its channel).
use crate::hotkey::HotkeyId;
use crate::monitors::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapAction {
    Left,
    Right,
    Max,
    /// Centred, 2/3 of the work area.
    Center,
    Third1,
    Third2,
    Third3,
    PrevMonitor,
    NextMonitor,
}

impl SnapAction {
    pub const HOTKEYS: [HotkeyId; 9] = [
        HotkeyId::SnapLeft,
        HotkeyId::SnapRight,
        HotkeyId::SnapMax,
        HotkeyId::SnapCenter,
        HotkeyId::SnapThird1,
        HotkeyId::SnapThird2,
        HotkeyId::SnapThird3,
        HotkeyId::MovePrevMonitor,
        HotkeyId::MoveNextMonitor,
    ];

    pub fn from_hotkey(id: HotkeyId) -> Option<SnapAction> {
        Some(match id {
            HotkeyId::SnapLeft => SnapAction::Left,
            HotkeyId::SnapRight => SnapAction::Right,
            HotkeyId::SnapMax => SnapAction::Max,
            HotkeyId::SnapCenter => SnapAction::Center,
            HotkeyId::SnapThird1 => SnapAction::Third1,
            HotkeyId::SnapThird2 => SnapAction::Third2,
            HotkeyId::SnapThird3 => SnapAction::Third3,
            HotkeyId::MovePrevMonitor => SnapAction::PrevMonitor,
            HotkeyId::MoveNextMonitor => SnapAction::NextMonitor,
            _ => return None,
        })
    }
}

/// Where the window's visible frame goes. `work`: work area of the monitor the window is
/// on; `current`: its visible frame; `monitors`: every monitor's work area (for the moves).
pub fn target_rect(action: SnapAction, work: Rect, current: Rect, monitors: &[Rect]) -> Rect {
    let Rect { x, y, w, h } = work;
    // Column `i` of `n` equal columns; the pieces tile the work area exactly.
    let column = |i: i32, n: i32| {
        let (a, b) = (w * i / n, w * (i + 1) / n);
        Rect { x: x + a, y, w: b - a, h }
    };
    match action {
        SnapAction::Left => column(0, 2),
        SnapAction::Right => column(1, 2),
        SnapAction::Max => work,
        SnapAction::Center => {
            let (cw, ch) = (w * 2 / 3, h * 2 / 3);
            Rect { x: x + (w - cw) / 2, y: y + (h - ch) / 2, w: cw, h: ch }
        }
        SnapAction::Third1 => column(0, 3),
        SnapAction::Third2 => column(1, 3),
        SnapAction::Third3 => column(2, 3),
        SnapAction::PrevMonitor | SnapAction::NextMonitor => {
            if monitors.len() < 2 {
                return current;
            }
            let mut order: Vec<Rect> = monitors.to_vec();
            order.sort_by_key(|m| (m.x, m.y));
            let Some(i) = order.iter().position(|m| *m == work).or_else(|| monitor_for(work, &order)) else {
                return current;
            };
            let n = order.len();
            let j = if action == SnapAction::NextMonitor { (i + 1) % n } else { (i + n - 1) % n };
            move_into(current, work, order[j])
        }
    }
}

/// `current` (relative to `from`) mapped proportionally into `to`, then kept inside it.
fn move_into(current: Rect, from: Rect, to: Rect) -> Rect {
    let scale = |v: i32, num: i32, den: i32| -> i32 {
        if den <= 0 {
            return v;
        }
        (v as i64 * num as i64).div_euclid(den as i64) as i32
    };
    let w = scale(current.w, to.w, from.w).clamp(1, to.w.max(1));
    let h = scale(current.h, to.h, from.h).clamp(1, to.h.max(1));
    let x = to.x + scale(current.x - from.x, to.w, from.w);
    let y = to.y + scale(current.y - from.y, to.h, from.h);
    Rect { x: x.clamp(to.x, to.x + to.w - w), y: y.clamp(to.y, to.y + to.h - h), w, h }
}

fn overlap(a: &Rect, b: &Rect) -> i64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if w > 0 && h > 0 { w as i64 * h as i64 } else { 0 }
}

/// Index in `works` of the monitor the window belongs to: largest overlap, else the one
/// whose centre is nearest. None only when `works` is empty.
pub fn monitor_for(current: Rect, works: &[Rect]) -> Option<usize> {
    let best = works
        .iter()
        .enumerate()
        .map(|(i, m)| (i, overlap(&current, m)))
        .filter(|&(_, a)| a > 0)
        .max_by_key(|&(_, a)| a);
    if let Some((i, _)) = best {
        return Some(i);
    }
    let centre = |r: &Rect| (r.x as i64 + r.w as i64 / 2, r.y as i64 + r.h as i64 / 2);
    let (cx, cy) = centre(&current);
    works
        .iter()
        .enumerate()
        .min_by_key(|(_, m)| {
            let (mx, my) = centre(m);
            (mx - cx).pow(2) + (my - cy).pow(2)
        })
        .map(|(i, _)| i)
}

/// `SetWindowPos` rect that puts the visible frame on `target`: `window` (GetWindowRect)
/// includes the invisible resize borders that `frame` (DWMWA_EXTENDED_FRAME_BOUNDS) omits.
pub fn with_invisible_borders(target: Rect, window: Rect, frame: Rect) -> Rect {
    let left = (frame.x - window.x).max(0);
    let top = (frame.y - window.y).max(0);
    let right = ((window.x + window.w) - (frame.x + frame.w)).max(0);
    let bottom = ((window.y + window.h) - (frame.y + frame.h)).max(0);
    Rect { x: target.x - left, y: target.y - top, w: target.w + left + right, h: target.h + top + bottom }
}

pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_THICKFRAME: u32 = 0x0004_0000;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;

/// What `eligible` needs to know about the foreground window.
#[derive(Clone, Debug, Default)]
pub struct WindowFacts {
    /// Belongs to Lucent.
    pub own: bool,
    pub class: String,
    pub style: u32,
    pub ex_style: u32,
    pub visible: bool,
    pub minimized: bool,
    pub hung: bool,
    /// The window rect covers its whole monitor.
    pub covers_monitor: bool,
    /// A game is running and focused (Game Mode).
    pub game_focused: bool,
}

/// Never our own windows, the shell, tool windows, hung or minimized windows, and never a
/// frameless window covering its monitor (fullscreen / borderless games, F11).
pub fn eligible(w: &WindowFacts) -> bool {
    let shell = matches!(w.class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd");
    let caption = w.style & WS_CAPTION == WS_CAPTION;
    let framed = caption || w.style & WS_THICKFRAME != 0;
    !w.own
        && w.visible
        && !w.minimized
        && !w.hung
        && !w.game_focused
        && !shell
        && w.ex_style & WS_EX_TOOLWINDOW == 0
        && framed
        && !(w.covers_monitor && !caption)
}

#[cfg(windows)]
pub mod win {
    //! Applies snap actions to the foreground window, on the `lucent-snap` thread:
    //! `SetWindowPos` on another process's window waits for it, never on the hotkey thread.
    use super::{SnapAction, WindowFacts};
    use crate::hotkey::{self, HotkeyId};
    use crate::monitors::{self, Rect};
    use std::sync::mpsc;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect,
        GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindowVisible, IsZoomed, SW_RESTORE,
        SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowPos, ShowWindow,
    };

    /// Registers the snap hotkey handlers (they only push to the worker's channel).
    pub fn start() {
        let (tx, rx) = mpsc::channel::<SnapAction>();
        let spawned = std::thread::Builder::new().name("lucent-snap".into()).spawn(move || {
            while let Ok(action) = rx.recv() {
                apply(action);
            }
        });
        if let Err(e) = spawned {
            return eprintln!("lucent: cannot start snap worker: {e}");
        }
        for id in SnapAction::HOTKEYS {
            let tx = tx.clone();
            hotkey::set_handler(id, move |id: HotkeyId| {
                if let Some(a) = SnapAction::from_hotkey(id) {
                    let _ = tx.send(a);
                }
            });
        }
    }

    fn rect(r: RECT) -> Rect {
        Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
    }

    /// (GetWindowRect, visible frame); the frame falls back to the window rect.
    fn rects(h: HWND) -> Option<(Rect, Rect)> {
        let mut r = RECT::default();
        unsafe { GetWindowRect(h, &mut r) }.ok()?;
        let window = rect(r);
        let mut f = RECT::default();
        let frame = unsafe {
            DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut f as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32)
        }
        .ok()
        .map(|_| rect(f))
        .filter(|f| f.w > 0 && f.h > 0)
        .unwrap_or(window);
        Some((window, frame))
    }

    fn place(h: HWND, target: Rect) -> bool {
        let Some((window, frame)) = rects(h) else { return false };
        let r = super::with_invisible_borders(target, window, frame);
        unsafe { SetWindowPos(h, None, r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER) }.is_ok()
    }

    fn apply(action: SnapAction) {
        let h = unsafe { GetForegroundWindow() };
        if h.0.is_null() {
            return;
        }
        let all = monitors::win::enumerate();
        let Some((window, frame)) = rects(h) else { return };
        let full: Vec<Rect> = all.iter().map(|m| m.rect).collect();
        let works: Vec<Rect> = all.iter().map(|m| m.work).collect();
        let Some(i) = super::monitor_for(frame, &works) else { return };
        let facts = unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            let mut buf = [0u16; 64];
            let n = (GetClassNameW(h, &mut buf).max(0) as usize).min(buf.len());
            let g = crate::game::service::state();
            WindowFacts {
                own: pid == std::process::id(),
                class: String::from_utf16_lossy(&buf[..n]),
                style: GetWindowLongW(h, GWL_STYLE) as u32,
                ex_style: GetWindowLongW(h, GWL_EXSTYLE) as u32,
                visible: IsWindowVisible(h).as_bool(),
                minimized: IsIconic(h).as_bool(),
                hung: IsHungAppWindow(h).as_bool(),
                covers_monitor: window.covers(&full[i]),
                game_focused: g.active && g.focused,
            }
        };
        if !super::eligible(&facts) {
            return;
        }
        let work = works[i];
        let zoomed = unsafe { IsZoomed(h) }.as_bool();
        // A maximised window fills its work area: a move keeps it filling the target's.
        let current = if zoomed { work } else { frame };
        let target = super::target_rect(action, work, current, &works);
        if zoomed {
            unsafe {
                let _ = ShowWindow(h, SW_RESTORE);
            }
        }
        if !place(h, target) {
            return eprintln!("lucent: snap: SetWindowPos failed");
        }
        // Crossing to a monitor with another DPI: the app resizes itself on WM_DPICHANGED
        // and the invisible borders change with the scale. Place it again, measured anew.
        if matches!(action, SnapAction::PrevMonitor | SnapAction::NextMonitor) {
            let _ = place(h, target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    /// Primary 1920x1080 @100 % (work area minus a 40 px bottom strip) and, on its right, a
    /// 3840x2160 @150 % monitor, 200 px lower, full work area.
    const A: Rect = r(0, 0, 1920, 1040);
    const B: Rect = r(1920, 200, 3840, 2160);
    const WIN: Rect = r(100, 100, 800, 600);

    #[test]
    fn from_hotkey_maps_every_snap_id() {
        let got: Vec<_> = SnapAction::HOTKEYS.iter().map(|&id| SnapAction::from_hotkey(id)).collect();
        use SnapAction::*;
        let want = [Left, Right, Max, Center, Third1, Third2, Third3, PrevMonitor, NextMonitor];
        assert_eq!(got, want.map(Some).to_vec());
        assert_eq!(SnapAction::from_hotkey(HotkeyId::Search), None);
        assert_eq!(SnapAction::from_hotkey(HotkeyId::MicMute), None);
    }

    #[test]
    fn halves_max_centre_and_thirds_table() {
        use SnapAction::*;
        let table = [
            (Left, A, r(0, 0, 960, 1040)),
            (Right, A, r(960, 0, 960, 1040)),
            (Max, A, A),
            (Center, A, r(320, 173, 1280, 693)),
            (Third1, A, r(0, 0, 640, 1040)),
            (Third2, A, r(640, 0, 640, 1040)),
            (Third3, A, r(1280, 0, 640, 1040)),
            // Odd sizes: the pieces tile the work area exactly.
            (Left, r(1920, 200, 3841, 2160), r(1920, 200, 1920, 2160)),
            (Right, r(1920, 200, 3841, 2160), r(3840, 200, 1921, 2160)),
            (Third1, r(0, 0, 1000, 500), r(0, 0, 333, 500)),
            (Third2, r(0, 0, 1000, 500), r(333, 0, 333, 500)),
            (Third3, r(0, 0, 1000, 500), r(666, 0, 334, 500)),
            (Center, B, r(2560, 560, 2560, 1440)),
        ];
        for (action, work, want) in table {
            assert_eq!(target_rect(action, work, WIN, &[A, B]), want, "{action:?} on {work:?}");
        }
    }

    #[test]
    fn move_to_next_monitor_scales_position_and_size_to_the_target_work_area() {
        // 100/1920 -> 200/3840 ; 100/1040 of A's height -> 2160*100/1040.
        let got = target_rect(SnapAction::NextMonitor, A, WIN, &[A, B]);
        assert_eq!(got, r(1920 + 200, 200 + 207, 1600, 1246));
        // And back: the inverse mapping.
        let back = target_rect(SnapAction::PrevMonitor, B, got, &[A, B]);
        assert_eq!(back, r(100, 99, 800, 599));
    }

    #[test]
    fn monitor_moves_wrap_and_follow_left_to_right_order() {
        let c = r(-1280, 0, 1280, 1024);
        // Listed out of order on purpose: order is by x.
        let mons = [B, A, c];
        let on_a = r(0, 0, 1920, 1040);
        assert_eq!(target_rect(SnapAction::PrevMonitor, A, on_a, &mons), c);
        assert_eq!(target_rect(SnapAction::NextMonitor, A, on_a, &mons), B);
        // Wraps around.
        assert_eq!(target_rect(SnapAction::NextMonitor, B, B, &mons), c);
        assert_eq!(target_rect(SnapAction::PrevMonitor, c, c, &mons), B);
    }

    #[test]
    fn single_monitor_move_keeps_the_window() {
        assert_eq!(target_rect(SnapAction::NextMonitor, A, WIN, &[A]), WIN);
        assert_eq!(target_rect(SnapAction::PrevMonitor, A, WIN, &[]), WIN);
    }

    #[test]
    fn moved_window_stays_inside_the_target_work_area() {
        // Hanging off A's right edge, bigger than the target.
        let big = r(1500, -50, 1900, 1200);
        let small = r(1920, 0, 1280, 720);
        let got = target_rect(SnapAction::NextMonitor, A, big, &[A, small]);
        assert!(small.covers(&got), "{got:?} not inside {small:?}");
        assert_eq!((got.w, got.h), (1266, 720));
    }

    #[test]
    fn monitor_for_largest_overlap_then_nearest() {
        assert_eq!(monitor_for(WIN, &[A, B]), Some(0));
        // Straddles, mostly on B.
        assert_eq!(monitor_for(r(1800, 300, 800, 600), &[A, B]), Some(1));
        // Off every monitor: nearest centre.
        assert_eq!(monitor_for(r(9000, 300, 100, 100), &[A, B]), Some(1));
        assert_eq!(monitor_for(WIN, &[]), None);
    }

    #[test]
    fn invisible_borders_are_added_back() {
        // Windows 10/11: 7 px invisible borders left/right/bottom, none on top.
        let window = r(93, 100, 814, 607);
        let frame = r(100, 100, 800, 600);
        assert_eq!(with_invisible_borders(r(0, 0, 960, 1040), window, frame), r(-7, 0, 974, 1047));
        // No DWM frame info (frame == window): unchanged.
        assert_eq!(with_invisible_borders(r(0, 0, 960, 1040), frame, frame), r(0, 0, 960, 1040));
    }

    fn normal() -> WindowFacts {
        WindowFacts {
            class: "Chrome_WidgetWin_1".into(),
            style: WS_CAPTION | WS_THICKFRAME,
            visible: true,
            ..Default::default()
        }
    }

    #[test]
    fn eligibility_table() {
        assert!(eligible(&normal()));
        // Maximised windows cover their monitor but keep their caption: still eligible.
        assert!(eligible(&WindowFacts { covers_monitor: true, ..normal() }));
        // Custom-frame app (thick frame, no caption) not covering the monitor.
        assert!(eligible(&WindowFacts { style: WS_THICKFRAME, ..normal() }));
        let rejected = [
            WindowFacts { own: true, ..normal() },
            WindowFacts { visible: false, ..normal() },
            WindowFacts { minimized: true, ..normal() },
            WindowFacts { hung: true, ..normal() },
            WindowFacts { game_focused: true, ..normal() },
            WindowFacts { ex_style: WS_EX_TOOLWINDOW, ..normal() },
            WindowFacts { class: "Progman".into(), ..normal() },
            WindowFacts { class: "WorkerW".into(), ..normal() },
            WindowFacts { class: "Shell_TrayWnd".into(), ..normal() },
            WindowFacts { class: "Shell_SecondaryTrayWnd".into(), ..normal() },
            // Borderless fullscreen game.
            WindowFacts { style: 0, covers_monitor: true, ..normal() },
            WindowFacts { style: WS_THICKFRAME, covers_monitor: true, ..normal() },
            // Plain popup (no frame at all).
            WindowFacts { style: 0, ..normal() },
        ];
        for w in rejected {
            assert!(!eligible(&w), "{w:?}");
        }
    }
}
