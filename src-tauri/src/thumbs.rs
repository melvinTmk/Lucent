//! Window thumbnails popup: pure layout/placement here, the Win32/webview part in `win`.
use crate::monitors::Rect;
use crate::settings::DockPosition;

/// Lays out one card per source window (aspect ratio kept, `thumb_h` tall), wrapping to a
/// new row when the row would exceed `max_total_w` (popup width, margins included).
/// Returns the popup size (at 0,0) and the thumbnail rects relative to the popup. `gap`
/// is also the popup's outer margin; each row reserves `title_h` below the thumbnails.
pub fn layout_thumbs(
    sources: &[(i32, i32)],
    max_total_w: i32,
    thumb_h: i32,
    gap: i32,
    title_h: i32,
) -> (Rect, Vec<Rect>) {
    let inner_max = (max_total_w - 2 * gap).max(1);
    let cell_h = thumb_h + title_h;
    let (mut x, mut row, mut widest) = (0, 0, 0);
    let mut rects = Vec::with_capacity(sources.len());
    for &(sw, sh) in sources {
        // Unknown size (minimised, gone): assume 16:9.
        let (sw, sh) = if sw > 0 && sh > 0 { (sw as i64, sh as i64) } else { (16, 9) };
        let mut w = ((thumb_h as i64 * sw + sh / 2) / sh) as i32;
        let mut h = thumb_h;
        if w > inner_max {
            // Wider than a whole row: shrink to the row, keeping the ratio.
            h = (thumb_h as i64 * inner_max as i64 / w as i64) as i32;
            w = inner_max;
        }
        let (w, h) = (w.max(1), h.max(1));
        if x > 0 && x + w > inner_max {
            row += 1;
            x = 0;
        }
        rects.push(Rect { x: gap + x, y: gap + row * (cell_h + gap), w, h });
        widest = widest.max(x + w);
        x += w + gap;
    }
    if rects.is_empty() {
        return (Rect { x: 0, y: 0, w: 2 * gap, h: 2 * gap }, rects);
    }
    let rows = row + 1;
    (Rect { x: 0, y: 0, w: widest + 2 * gap, h: rows * cell_h + (rows + 1) * gap }, rects)
}

/// Popup rect near the clicked icon: centred on `anchor` along the dock's edge, `gap` away
/// from the dock, clamped inside the monitor.
pub fn place_popup(anchor: (i32, i32), size: (i32, i32), dock: Rect, pos: DockPosition, mon: Rect, gap: i32) -> Rect {
    let (w, h) = (size.0.min(mon.w), size.1.min(mon.h));
    let (x, y) = match pos {
        DockPosition::Bottom => (anchor.0 - w / 2, dock.y - gap - h),
        DockPosition::Left => (dock.x + dock.w + gap, anchor.1 - h / 2),
        DockPosition::Right => (dock.x - gap - w, anchor.1 - h / 2),
    };
    Rect { x: x.clamp(mon.x, mon.x + mon.w - w), y: y.clamp(mon.y, mon.y + mon.h - h), w, h }
}

/// Logical px (scaled by the monitor's factor at use).
pub const THUMB_H: f32 = 180.0;
pub const GAP: f32 = 16.0;
pub const TITLE_H: f32 = 24.0;
/// Distance between the dock and the popup.
pub const POPUP_GAP: f32 = 12.0;
/// The popup never gets wider than this share of the monitor.
pub const MAX_WIDTH_SHARE: f32 = 0.9;
/// Label of the single thumbnails window.
pub const LABEL: &str = "thumbs";

#[cfg(windows)]
pub mod win {
    //! One `thumbs` webview window (non-activating, topmost, rounded region, native
    //! composition glass from `backdrop`, like the dock) plus the DWM thumbnails drawn over
    //! it. Everything here runs on the dock-host worker thread (see `dock_host`): never
    //! build the window or call `SetWindowPos` from the main thread; `backdrop` calls go to
    //! the main thread through `run_on_main_thread`. The page only draws the glass cards
    //! and titles under the rects.
    //!
    //! No Tauri `Effect::Acrylic` here: the DWM system backdrop ignores the region and turns
    //! flat grey on a window that never takes focus (see `backdrop` and docs/decisions.md).
    use super::*;
    use crate::dock_host::{make_non_activating, no_activate_proc, set_pos};
    use crate::monitors::MonitorInfo;
    use crate::region;
    use serde::Serialize;
    use tauri::{AppHandle, Emitter, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{
        DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_VISIBLE, DwmQueryThumbnailSourceSize,
        DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
    };
    use windows::Win32::UI::Shell::SetWindowSubclass;
    use windows::Win32::UI::WindowsAndMessaging::{HWND_TOPMOST, SWP_NOZORDER, SWP_SHOWWINDOW};

    /// Where the window waits while closed: shown (so the webview keeps rendering and the
    /// first open has no load delay) but far outside every monitor.
    const PARKED: Rect = Rect { x: -32000, y: -32000, w: 200, h: 100 };

    /// Accent tint used only when native glass is unavailable (0xAABBGGRR), as for the dock.
    const ACCENT_TINT: u32 = 0x10_80_80_80;

    #[derive(Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    struct ShowPayload {
        item_id: String,
        hwnds: Vec<isize>,
        titles: Vec<String>,
        /// Physical px, relative to the window.
        rects: Vec<Rect>,
        width: i32,
        height: i32,
        /// Monitor scale: the page divides physical px by it (its own devicePixelRatio may
        /// lag behind right after the window moved to another monitor).
        scale: f32,
        title_h: i32,
        /// Same tone as the dock the popup belongs to.
        tone: String,
    }

    pub struct Open {
        pub item_id: String,
        pub monitor_id: String,
        /// Screen rect of the popup.
        pub rect: Rect,
    }

    pub struct ShowReq {
        pub item_id: String,
        pub monitor: MonitorInfo,
        pub dock: Rect,
        pub pos: crate::settings::DockPosition,
        /// Cursor at click time, screen px: the clicked icon is under it.
        pub anchor: (i32, i32),
        pub windows: Vec<(isize, String)>,
        pub tone: String,
        /// Game Mode static glass, as for the dock of that monitor (`dock_host::static_glass`).
        pub static_glass: bool,
    }

    pub struct ThumbsWindow {
        app: AppHandle,
        window: WebviewWindow,
        hwnd: isize,
        thumbs: Vec<isize>,
        pub open: Option<Open>,
    }

    impl ThumbsWindow {
        pub fn create(app: &AppHandle) -> tauri::Result<Self> {
            let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("thumbs.html".into()))
                .title("Lucent thumbnails")
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
            let _ = window.run_on_main_thread(move || unsafe {
                let _ = SetWindowSubclass(HWND(hwnd as *mut _), Some(no_activate_proc), 1, 0);
            });
            set_pos(HWND(hwnd as *mut _), Some(HWND_TOPMOST), PARKED, SWP_SHOWWINDOW);
            Ok(Self { app: app.clone(), window, hwnd, thumbs: Vec::new(), open: None })
        }

        fn hwnd(&self) -> HWND {
            HWND(self.hwnd as *mut _)
        }

        fn unregister_all(&mut self) {
            for t in self.thumbs.drain(..) {
                unsafe {
                    let _ = DwmUnregisterThumbnail(t);
                }
            }
        }

        pub fn show(&mut self, req: ShowReq) {
            self.unregister_all();
            // Register first: the source size (needed for the layout) comes from DWM.
            let mut regs: Vec<(isize, isize, String, (i32, i32))> = Vec::new();
            for (src, title) in &req.windows {
                let Ok(t) = (unsafe { DwmRegisterThumbnail(self.hwnd(), HWND(*src as *mut _)) }) else { continue };
                let size = unsafe { DwmQueryThumbnailSourceSize(t) }.map_or((0, 0), |s| (s.cx, s.cy));
                regs.push((t, *src, title.clone(), size));
            }
            self.thumbs = regs.iter().map(|r| r.0).collect();
            if regs.is_empty() {
                self.hide();
                return;
            }
            let sc = req.monitor.scale;
            let px = |v: f32| (v * sc).round() as i32;
            let sizes: Vec<(i32, i32)> = regs.iter().map(|r| r.3).collect();
            let max_w = (req.monitor.rect.w as f32 * MAX_WIDTH_SHARE) as i32;
            let (size, rects) = layout_thumbs(&sizes, max_w, px(THUMB_H), px(GAP), px(TITLE_H));
            let rect = place_popup(req.anchor, (size.w, size.h), req.dock, req.pos, req.monitor.rect, px(POPUP_GAP));
            // Moving to a monitor with another DPI makes tao resize the window (WM_DPICHANGED
            // is handled synchronously during the first call): the second call wins.
            for _ in 0..2 {
                set_pos(self.hwnd(), Some(HWND_TOPMOST), rect, SWP_SHOWWINDOW);
            }
            self.apply_glass(rect.w, rect.h, sc, req.static_glass.then_some(req.tone == "dark"));
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
            self.open = Some(Open { item_id: req.item_id.clone(), monitor_id: req.monitor.id.clone(), rect });
            let payload = ShowPayload {
                item_id: req.item_id,
                hwnds: regs.iter().map(|r| r.1).collect(),
                titles: regs.into_iter().map(|r| r.2).collect(),
                rects,
                width: rect.w,
                height: rect.h,
                scale: sc,
                title_h: px(TITLE_H),
                tone: req.tone,
            };
            if let Err(e) = self.app.emit_to(LABEL, "thumbs://show", payload) {
                eprintln!("lucent: cannot emit thumbs://show: {e}");
            }
        }

        /// One glass island over the whole client rect; the region is the same rounded rect
        /// grown by `REGION_PAD` (as for the dock and the search window), so it only cuts
        /// outside the glass. `static_dark`: Game Mode static glass with that tone.
        fn apply_glass(&self, w: i32, h: i32, scale: f32, static_dark: Option<bool>) {
            let island = Rect { x: 0, y: 0, w, h };
            let pad = (region::REGION_PAD * scale).round() as i32;
            let radius = region::ISLAND_RADIUS * scale;
            region::apply(self.hwnd(), &[region::grown(island, pad)], radius.round() as i32 + pad);
            let hwnd = self.hwnd;
            let _ = self.window.run_on_main_thread(move || {
                let r = match static_dark {
                    Some(dark) => crate::backdrop::set_tone(hwnd, dark).and_then(|_| crate::backdrop::set_static(hwnd, true)),
                    None => crate::backdrop::set_static(hwnd, false),
                };
                if let Err(e) = r.and_then(|_| crate::backdrop::set_islands(hwnd, &[island], radius)) {
                    eprintln!("lucent: cannot shape the thumbnails glass: {e}");
                }
            });
        }

        pub fn hide(&mut self) {
            self.unregister_all();
            if self.open.take().is_some() {
                if let Err(e) = self.app.emit_to(LABEL, "thumbs://hide", ()) {
                    eprintln!("lucent: cannot emit thumbs://hide: {e}");
                }
                // The docks' hover state machines (Escape, outside click, pick, dock hidden).
                if let Err(e) = self.app.emit("thumbs://closed", ()) {
                    eprintln!("lucent: cannot emit thumbs://closed: {e}");
                }
            }
            set_pos(self.hwnd(), None, PARKED, SWP_NOZORDER | SWP_SHOWWINDOW);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAP: i32 = 16;
    const TITLE: i32 = 24;

    #[test]
    fn single_row_keeps_aspect_ratio() {
        let (_, r) = layout_thumbs(&[(1600, 900), (900, 1600)], 5000, 180, GAP, TITLE);
        assert_eq!((r[0].w, r[0].h), (320, 180));
        assert_eq!((r[1].w, r[1].h), (101, 180));
        assert_eq!(r[0].y, r[1].y, "same row");
        assert!(r[1].x >= r[0].x + r[0].w + GAP);
    }

    #[test]
    fn wraps_when_wider_than_max() {
        let (popup, r) = layout_thumbs(&[(1920, 1080); 8], 1200, 180, GAP, TITLE);
        assert_eq!(r.len(), 8);
        let rows: std::collections::BTreeSet<i32> = r.iter().map(|x| x.y).collect();
        assert!(rows.len() >= 2);
        assert!(r.iter().all(|x| x.x + x.w <= 1200), "{r:?}");
        assert!(popup.w <= 1200);
    }

    #[test]
    fn popup_size_contains_all_rects_plus_titles() {
        let (popup, r) = layout_thumbs(&[(1920, 1080); 8], 1200, 180, GAP, TITLE);
        for x in &r {
            assert!(x.x >= GAP && x.y >= GAP);
            assert!(x.x + x.w + GAP <= popup.w, "{x:?} in {popup:?}");
            assert!(x.y + x.h + TITLE + GAP <= popup.h, "{x:?} in {popup:?}");
        }
    }

    #[test]
    fn zero_sized_source_does_not_panic() {
        let (_, r) = layout_thumbs(&[(0, 0), (0, 500), (500, 0)], 1200, 180, GAP, TITLE);
        assert_eq!((r[0].w, r[0].h), (320, 180));
        assert!(r.iter().all(|x| x.w > 0 && x.h > 0));
    }

    #[test]
    fn very_wide_source_is_capped_to_the_row() {
        let (popup, r) = layout_thumbs(&[(10000, 100)], 600, 180, GAP, TITLE);
        assert!(r[0].x + r[0].w + GAP <= 600 && popup.w <= 600);
        assert!(r[0].h > 0 && r[0].h <= 180);
    }

    #[test]
    fn empty_sources_give_a_margin_only_popup() {
        let (popup, r) = layout_thumbs(&[], 1200, 180, GAP, TITLE);
        assert!(r.is_empty());
        assert_eq!((popup.w, popup.h), (2 * GAP, 2 * GAP));
    }

    const MON: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };

    #[test]
    fn popup_sits_above_a_bottom_dock_centred_on_the_icon() {
        let dock = Rect { x: 500, y: 980, w: 900, h: 80 };
        let r = place_popup((900, 1000), (400, 250), dock, DockPosition::Bottom, MON, 12);
        assert_eq!(r, Rect { x: 700, y: 980 - 12 - 250, w: 400, h: 250 });
    }

    #[test]
    fn popup_is_clamped_to_the_monitor() {
        let dock = Rect { x: 0, y: 980, w: 900, h: 80 };
        let r = place_popup((20, 1000), (400, 250), dock, DockPosition::Bottom, MON, 12);
        assert_eq!(r.x, 0);
        let r = place_popup((1900, 1000), (400, 250), dock, DockPosition::Bottom, MON, 12);
        assert_eq!(r.x, 1920 - 400);
    }

    #[test]
    fn popup_sits_beside_side_docks() {
        let left = Rect { x: 10, y: 300, w: 80, h: 500 };
        let r = place_popup((50, 400), (400, 250), left, DockPosition::Left, MON, 12);
        assert_eq!((r.x, r.y), (10 + 80 + 12, 275));
        let right = Rect { x: 1830, y: 300, w: 80, h: 500 };
        let r = place_popup((1870, 400), (400, 250), right, DockPosition::Right, MON, 12);
        assert_eq!((r.x, r.y), (1830 - 12 - 400, 275));
    }

    #[test]
    fn oversized_popup_does_not_panic() {
        let dock = Rect { x: 0, y: 0, w: 10, h: 10 };
        let r = place_popup((0, 0), (5000, 5000), dock, DockPosition::Bottom, MON, 12);
        assert_eq!((r.x, r.y), (0, 0));
    }
}
