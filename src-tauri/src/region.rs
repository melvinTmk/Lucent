//! Window region = union of the dock islands with rounded corners, so the native Acrylic
//! backdrop (and hit-testing) only exists inside the islands. See `docs/decisions.md`.
#![cfg(windows)]
use crate::monitors::Rect;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, HRGN, RGN_OR, SetWindowRgn,
};

/// Corner radius of the islands in logical px (validated mockup).
pub const ISLAND_RADIUS: f32 = 18.0;

/// Logical px the window region extends past each glass island: the region (GDI, aliased)
/// then only ever cuts transparent pixels; the glass and the CSS rim draw the real edge.
pub const REGION_PAD: f32 = 2.0;

/// `r` grown by `pad` px on every side.
pub fn grown(r: Rect, pad: i32) -> Rect {
    Rect { x: r.x - pad, y: r.y - pad, w: r.w + 2 * pad, h: r.h + 2 * pad }
}

/// Union of `islands` (client coordinates, physical px), each with corners of `radius`.
/// The caller owns the returned region (hand it to `SetWindowRgn` or `DeleteObject` it).
/// No islands → an empty region.
pub fn rounded_union(islands: &[Rect], radius: i32) -> HRGN {
    unsafe {
        let acc = CreateRectRgn(0, 0, 0, 0);
        for r in islands.iter().filter(|r| r.w > 0 && r.h > 0) {
            // A radius larger than half the side would bulge; clamp to a pill.
            let rad = radius.clamp(0, r.w.min(r.h) / 2);
            // CreateRoundRectRgn excludes the right/bottom edge: +1 to cover w×h pixels.
            let one = CreateRoundRectRgn(r.x, r.y, r.x + r.w + 1, r.y + r.h + 1, rad * 2, rad * 2);
            CombineRgn(Some(acc), Some(acc), Some(one), RGN_OR);
            let _ = DeleteObject(one.into());
        }
        acc
    }
}

/// Clips `hwnd` to the rounded union of `islands`. No islands → no clipping at all
/// (an empty region would make the window invisible and unclickable).
pub fn apply(hwnd: HWND, islands: &[Rect], radius: i32) {
    unsafe {
        if islands.is_empty() {
            SetWindowRgn(hwnd, None, true);
            return;
        }
        let rgn = rounded_union(islands, radius);
        // On success the system owns the region; on failure we must free it.
        if SetWindowRgn(hwnd, Some(rgn), true) == 0 {
            let _ = DeleteObject(rgn.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::PtInRegion;

    fn inside(rgn: HRGN, x: i32, y: i32) -> bool {
        unsafe { PtInRegion(rgn, x, y).as_bool() }
    }

    #[test]
    fn union_has_rounded_corners_and_gaps() {
        let islands = [Rect { x: 0, y: 0, w: 200, h: 80 }, Rect { x: 220, y: 0, w: 100, h: 80 }];
        let rgn = rounded_union(&islands, 26);
        assert!(inside(rgn, 100, 40), "island centre");
        assert!(inside(rgn, 270, 40), "second island centre");
        assert!(inside(rgn, 199, 40) && inside(rgn, 100, 79), "edges covered");
        assert!(!inside(rgn, 0, 0) && !inside(rgn, 199, 79), "corners cut");
        assert!(!inside(rgn, 210, 40), "gap between islands");
        unsafe {
            let _ = DeleteObject(rgn.into());
        }
    }

    #[test]
    fn no_islands_is_empty() {
        let rgn = rounded_union(&[], 26);
        assert!(!inside(rgn, 0, 0));
        unsafe {
            let _ = DeleteObject(rgn.into());
        }
    }
}

/// Blur-behind through the (undocumented, Windows 10 1803+) `SetWindowCompositionAttribute`
/// accent. Unlike the DWM system backdrop (`DWMWA_SYSTEMBACKDROP_TYPE`, what Tauri's
/// `Effect::Acrylic` uses on Windows 11), the accent is clipped by the window region and
/// does not fall back to a flat colour when the window is inactive: the dock never takes
/// focus, so the backdrop would otherwise always render as its inactive grey.
/// `tint` is 0xAABBGGRR.
pub fn set_acrylic_accent(hwnd: HWND, tint: u32) -> bool {
    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        gradient: u32,
        animation: u32,
    }
    #[repr(C)]
    struct CompositionData {
        attrib: u32,
        data: *mut core::ffi::c_void,
        size: usize,
    }
    // Not in user32.lib: resolve it at run time.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut core::ffi::c_void;
        fn GetProcAddress(module: *mut core::ffi::c_void, name: *const u8) -> *mut core::ffi::c_void;
    }
    type SetWca = unsafe extern "system" fn(HWND, *mut CompositionData) -> i32;
    let user32: Vec<u16> = "user32.dll\0".encode_utf16().collect();
    let proc = unsafe { GetProcAddress(GetModuleHandleW(user32.as_ptr()), b"SetWindowCompositionAttribute\0".as_ptr()) };
    if proc.is_null() {
        return false;
    }
    let set_wca: SetWca = unsafe { std::mem::transmute(proc) };
    const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
    const WCA_ACCENT_POLICY: u32 = 19;
    let mut policy = AccentPolicy { state: ACCENT_ENABLE_ACRYLICBLURBEHIND, flags: 0, gradient: tint, animation: 0 };
    let mut data = CompositionData {
        attrib: WCA_ACCENT_POLICY,
        data: &mut policy as *mut _ as *mut _,
        size: std::mem::size_of::<AccentPolicy>(),
    };
    unsafe { set_wca(hwnd, &mut data) != 0 }
}
