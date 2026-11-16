//! Hot corners: pure hit-testing and the dwell state machine. `dock_host` feeds it from
//! its existing cursor poll (no extra polling, no mouse hook) and runs the action.
use crate::monitors::Rect;
use crate::settings::HotCorners;
use crate::Action;

/// Side of the square hit zone in each corner (physical px).
pub const HIT_SIZE: i32 = 2;
/// The cursor must rest in the corner this long.
pub const DWELL_MS: u64 = 250;
/// After firing, the corner re-arms once the cursor is this far from it.
pub const REARM_PX: i32 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// A corner of one monitor, identified by its corner pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub corner: Corner,
    pub point: (i32, i32),
}

/// Which corner of `monitor` the cursor is in (a `size`x`size` square), if any.
pub fn corner_hit(cursor: (i32, i32), monitor: Rect, size: i32) -> Option<Corner> {
    let (x, y) = cursor;
    if !monitor.contains(x, y) {
        return None;
    }
    let left = x < monitor.x + size;
    let right = x >= monitor.x + monitor.w - size;
    let top = y < monitor.y + size;
    let bottom = y >= monitor.y + monitor.h - size;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(Corner::TopLeft),
        (_, true, true, _) => Some(Corner::TopRight),
        (true, _, _, true) => Some(Corner::BottomLeft),
        (_, true, _, true) => Some(Corner::BottomRight),
        _ => None,
    }
}

/// The corner pixel of `monitor`.
pub fn corner_point(c: Corner, m: Rect) -> (i32, i32) {
    let (l, t, r, b) = (m.x, m.y, m.x + m.w - 1, m.y + m.h - 1);
    match c {
        Corner::TopLeft => (l, t),
        Corner::TopRight => (r, t),
        Corner::BottomLeft => (l, b),
        Corner::BottomRight => (r, b),
    }
}

/// The cursor is stopped by the screen edge in this corner: neither of the two pixels just
/// past it (horizontally, vertically) belongs to another monitor.
pub fn is_outer_corner(c: Corner, m: Rect, all: &[Rect]) -> bool {
    let (x, y) = corner_point(c, m);
    let dx = if matches!(c, Corner::TopLeft | Corner::BottomLeft) { -1 } else { 1 };
    let dy = if matches!(c, Corner::TopLeft | Corner::TopRight) { -1 } else { 1 };
    let on_screen = |px: i32, py: i32| all.iter().any(|o| o.contains(px, py));
    !on_screen(x + dx, y) && !on_screen(x, y + dy)
}

/// Outer corner under the cursor across every monitor.
pub fn hit(cursor: (i32, i32), monitors: &[Rect]) -> Option<Hit> {
    let m = *monitors.iter().find(|m| m.contains(cursor.0, cursor.1))?;
    let corner = corner_hit(cursor, m, HIT_SIZE)?;
    is_outer_corner(corner, m, monitors).then(|| Hit { corner, point: corner_point(corner, m) })
}

/// A fullscreen / borderless foreground window covers the monitor under the cursor: its
/// corners belong to that app (games, videos), never fire there.
pub fn fullscreen_under(cursor: (i32, i32), monitors: &[Rect], fullscreen: Option<Rect>) -> bool {
    let Some(fs) = fullscreen else { return false };
    monitors.iter().find(|m| m.contains(cursor.0, cursor.1)).is_some_and(|m| fs.covers(m))
}

/// The action configured for `c` (None for "none" / unknown).
pub fn action_for(c: Corner, hc: &HotCorners) -> Option<Action> {
    Action::from_corner(match c {
        Corner::TopLeft => &hc.top_left,
        Corner::TopRight => &hc.top_right,
        Corner::BottomLeft => &hc.bottom_left,
        Corner::BottomRight => &hc.bottom_right,
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Idle,
    Arming { hit: Hit, since_ms: u64 },
    /// Fired; waits for the cursor to leave by `REARM_PX`.
    Fired { point: (i32, i32) },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Dwell {
    state: State,
}

impl Dwell {
    /// One poll. `hit`: configured corner under the cursor; `blocked`: a mouse button is
    /// down or a game is focused. Returns the corner to fire, once per visit.
    pub fn step(&mut self, hit: Option<Hit>, cursor: Option<(i32, i32)>, now_ms: u64, blocked: bool) -> Option<Corner> {
        if let State::Fired { point } = self.state {
            let left = cursor.is_none_or(|(x, y)| (x - point.0).abs().max((y - point.1).abs()) > REARM_PX);
            if !left {
                return None;
            }
            self.state = State::Idle;
        }
        if blocked {
            self.state = State::Idle;
            return None;
        }
        match (self.state, hit) {
            (_, None) => self.state = State::Idle,
            (State::Arming { hit: armed, since_ms }, Some(h)) if armed == h => {
                if now_ms.saturating_sub(since_ms) >= DWELL_MS {
                    self.state = State::Fired { point: h.point };
                    return Some(h.corner);
                }
            }
            (_, Some(h)) => self.state = State::Arming { hit: h, since_ms: now_ms },
        }
        None
    }

    /// Nothing pending: the poller may slow down.
    pub fn idle(&self) -> bool {
        self.state == State::Idle
    }
}

/// The `desktop` and `lock` actions.
#[cfg(windows)]
pub mod win {
    /// Shows the desktop / brings the windows back (`IShellDispatch4::ToggleDesktop`).
    /// On a short-lived STA thread: never blocks the caller (cursor poll, hotkey thread).
    pub fn toggle_desktop() {
        let spawned = std::thread::Builder::new().name("lucent-desktop".into()).spawn(|| {
            use windows::Win32::System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
            };
            use windows::Win32::UI::Shell::{IShellDispatch4, Shell};
            unsafe {
                let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
                match CoCreateInstance::<_, IShellDispatch4>(&Shell, None, CLSCTX_INPROC_SERVER) {
                    Ok(shell) => {
                        if let Err(e) = shell.ToggleDesktop() {
                            eprintln!("lucent: ToggleDesktop failed: {e}");
                        }
                    }
                    Err(e) => eprintln!("lucent: no shell dispatch: {e}"),
                }
                if com {
                    CoUninitialize();
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("lucent: cannot spawn desktop thread: {e}");
        }
    }

    /// Locks the session (returns at once; the lock screen comes up asynchronously).
    pub fn lock() {
        if let Err(e) = unsafe { windows::Win32::System::Shutdown::LockWorkStation() } {
            eprintln!("lucent: LockWorkStation failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fullscreen_blocks_only_the_covered_monitor() {
        let a = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let b = Rect { x: 1920, y: 0, w: 1920, h: 1080 };
        let mons = [a, b];
        assert!(!fullscreen_under((0, 0), &mons, None));
        assert!(fullscreen_under((0, 0), &mons, Some(a)));
        assert!(!fullscreen_under((3839, 0), &mons, Some(a)), "other monitor stays live");
        assert!(fullscreen_under((3839, 0), &mons, Some(Rect { x: 1912, y: -8, w: 1936, h: 1096 })));
        assert!(!fullscreen_under((0, 0), &mons, Some(Rect { x: 0, y: 0, w: 1920, h: 1040 })), "maximized");
    }

    use super::*;

    const fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }
    const A: Rect = r(0, 0, 1920, 1080);
    /// Right of A, 200 px lower (its top-left corner is free, its bottom-left is not).
    const B: Rect = r(1920, 200, 2560, 1440);

    #[test]
    fn corner_hit_table() {
        use Corner::*;
        let table = [
            ((0, 0), Some(TopLeft)),
            ((1, 1), Some(TopLeft)),
            ((2, 0), None),
            ((0, 2), None),
            ((1919, 0), Some(TopRight)),
            ((1918, 1), Some(TopRight)),
            ((1917, 0), None),
            ((0, 1079), Some(BottomLeft)),
            ((1919, 1079), Some(BottomRight)),
            ((960, 540), None),
            // Outside the monitor.
            ((-1, 0), None),
            ((1920, 0), None),
        ];
        for (cursor, want) in table {
            assert_eq!(corner_hit(cursor, A, HIT_SIZE), want, "{cursor:?}");
        }
    }

    #[test]
    fn corner_points() {
        assert_eq!(corner_point(Corner::TopLeft, B), (1920, 200));
        assert_eq!(corner_point(Corner::TopRight, B), (4479, 200));
        assert_eq!(corner_point(Corner::BottomLeft, B), (1920, 1639));
        assert_eq!(corner_point(Corner::BottomRight, B), (4479, 1639));
    }

    #[test]
    fn corners_shared_with_a_neighbour_are_not_outer() {
        let all = [A, B];
        // A's right edge touches B from y=200: its top-right corner is still stopped (B
        // starts lower), its bottom-right is not (the cursor slides into B).
        assert!(is_outer_corner(Corner::TopRight, A, &all));
        assert!(!is_outer_corner(Corner::BottomRight, A, &all));
        assert!(is_outer_corner(Corner::TopLeft, A, &all));
        assert!(is_outer_corner(Corner::BottomLeft, B, &all));
        // B's top-left: left of it is A (y=200 < 1080) -> not outer.
        assert!(!is_outer_corner(Corner::TopLeft, B, &all));
        assert!(is_outer_corner(Corner::TopRight, B, &all));
        // Alone: every corner is outer.
        for c in [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight] {
            assert!(is_outer_corner(c, A, &[A]));
        }
    }

    #[test]
    fn hit_finds_the_monitor_and_skips_inner_corners() {
        let all = [A, B];
        assert_eq!(hit((0, 0), &all), Some(Hit { corner: Corner::TopLeft, point: (0, 0) }));
        assert_eq!(hit((4479, 201), &all), Some(Hit { corner: Corner::TopRight, point: (4479, 200) }));
        assert_eq!(hit((1919, 1079), &all), None);
        assert_eq!(hit((500, 500), &all), None);
    }

    #[test]
    fn action_for_reads_the_settings() {
        let hc = HotCorners {
            top_left: "overview".into(),
            top_right: "desktop".into(),
            bottom_left: "none".into(),
            bottom_right: "bogus".into(),
        };
        assert_eq!(action_for(Corner::TopLeft, &hc), Some(Action::Overview));
        assert_eq!(action_for(Corner::TopRight, &hc), Some(Action::Desktop));
        assert_eq!(action_for(Corner::BottomLeft, &hc), None);
        assert_eq!(action_for(Corner::BottomRight, &hc), None);
    }

    const TL: Hit = Hit { corner: Corner::TopLeft, point: (0, 0) };
    const TR: Hit = Hit { corner: Corner::TopRight, point: (1919, 0) };

    #[test]
    fn fires_once_after_the_dwell() {
        let mut d = Dwell::default();
        assert!(d.idle());
        assert_eq!(d.step(Some(TL), Some((0, 0)), 1000, false), None);
        assert!(!d.idle());
        assert_eq!(d.step(Some(TL), Some((1, 0)), 1200, false), None);
        assert_eq!(d.step(Some(TL), Some((0, 0)), 1250, false), Some(Corner::TopLeft));
        // Staying in the corner never fires again.
        assert_eq!(d.step(Some(TL), Some((0, 0)), 2000, false), None);
        assert_eq!(d.step(Some(TL), Some((0, 0)), 9000, false), None);
    }

    #[test]
    fn leaving_before_the_dwell_cancels() {
        let mut d = Dwell::default();
        d.step(Some(TL), Some((0, 0)), 0, false);
        assert_eq!(d.step(None, Some((5, 5)), 100, false), None);
        assert!(d.idle());
        // Back in: a fresh 250 ms.
        assert_eq!(d.step(Some(TL), Some((0, 0)), 200, false), None);
        assert_eq!(d.step(Some(TL), Some((0, 0)), 400, false), None);
        assert_eq!(d.step(Some(TL), Some((0, 0)), 450, false), Some(Corner::TopLeft));
    }

    #[test]
    fn re_arms_only_after_leaving_by_40_px() {
        let mut d = Dwell::default();
        d.step(Some(TL), Some((0, 0)), 0, false);
        assert_eq!(d.step(Some(TL), Some((0, 0)), 250, false), Some(Corner::TopLeft));
        // Wiggles out of the 2 px zone but within 40 px, then back: nothing.
        assert_eq!(d.step(None, Some((30, 40)), 300, false), None);
        assert_eq!(d.step(Some(TL), Some((0, 0)), 400, false), None);
        assert_eq!(d.step(Some(TL), Some((0, 0)), 900, false), None);
        // Leaves far enough: re-armed.
        assert_eq!(d.step(None, Some((41, 0)), 1000, false), None);
        assert!(d.idle());
        d.step(Some(TL), Some((0, 0)), 1100, false);
        assert_eq!(d.step(Some(TL), Some((0, 0)), 1350, false), Some(Corner::TopLeft));
    }

    #[test]
    fn mouse_button_or_game_blocks_and_restarts_the_dwell() {
        let mut d = Dwell::default();
        d.step(Some(TL), Some((0, 0)), 0, false);
        // Dragging into the corner: never fires while blocked.
        assert_eq!(d.step(Some(TL), Some((0, 0)), 300, true), None);
        assert_eq!(d.step(Some(TL), Some((0, 0)), 600, true), None);
        // Released: the dwell starts over.
        assert_eq!(d.step(Some(TL), Some((0, 0)), 700, false), None);
        assert_eq!(d.step(Some(TL), Some((0, 0)), 900, false), None);
        assert_eq!(d.step(Some(TL), Some((0, 0)), 950, false), Some(Corner::TopLeft));
    }

    #[test]
    fn switching_corner_restarts_the_dwell_and_lost_cursor_resets() {
        let mut d = Dwell::default();
        d.step(Some(TL), Some((0, 0)), 0, false);
        assert_eq!(d.step(Some(TR), Some((1919, 0)), 200, false), None);
        assert_eq!(d.step(Some(TR), Some((1919, 0)), 300, false), None);
        assert_eq!(d.step(Some(TR), Some((1919, 0)), 450, false), Some(Corner::TopRight));
        // Secure desktop (no cursor): re-armed.
        assert_eq!(d.step(None, None, 500, false), None);
        assert!(d.idle());
    }
}
