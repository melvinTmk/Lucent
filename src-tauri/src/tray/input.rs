//! Translate a dock-side click into the callback messages a tray icon expects.

pub const WM_LBUTTONDOWN: u32 = 0x201;
pub const WM_LBUTTONUP: u32 = 0x202;
pub const WM_LBUTTONDBLCLK: u32 = 0x203;
pub const WM_RBUTTONDOWN: u32 = 0x204;
pub const WM_RBUTTONUP: u32 = 0x205;
pub const WM_CONTEXTMENU: u32 = 0x7B;
pub const NIN_SELECT: u32 = 0x400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayClick {
    Left,
    Right,
    Double,
}

/// Returns `(message, wParam, lParam)` triples to post to the icon's callback window.
/// `message` is the icon's `uCallbackMessage` (applied by the caller); here it is the event id
/// for legacy mode and in v4 mode the event goes in LOWORD(lParam).
/// The first tuple element is the event; the caller posts `(callback_message, wParam, lParam)`.
pub fn callback_messages(
    click: TrayClick,
    version: u32,
    uid: u32,
    anchor: (i16, i16),
) -> Vec<(u32, usize, isize)> {
    let modern = version >= 4;
    let events: &[u32] = match (click, modern) {
        (TrayClick::Left, true) => &[WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT],
        (TrayClick::Left, false) => &[WM_LBUTTONDOWN, WM_LBUTTONUP],
        (TrayClick::Right, true) => &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU],
        (TrayClick::Right, false) => &[WM_RBUTTONDOWN, WM_RBUTTONUP],
        (TrayClick::Double, _) => &[WM_LBUTTONDBLCLK, WM_LBUTTONUP],
    };
    events
        .iter()
        .map(|&ev| {
            if modern {
                let w = ((anchor.0 as u16 as u32) | ((anchor.1 as u16 as u32) << 16)) as usize;
                let l = ((ev & 0xFFFF) | ((uid & 0xFFFF) << 16)) as i32 as isize;
                (ev, w, l)
            } else {
                (ev, uid as usize, ev as isize)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4_left_click_messages() {
        assert_eq!(
            callback_messages(TrayClick::Left, 4, 7, (100, 200))
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            vec![0x201, 0x202, 0x400]
        );
    }

    #[test]
    fn v4_lparam_packs_event_and_uid() {
        let m = callback_messages(TrayClick::Right, 4, 7, (100, -2));
        let (ev, w, l) = m[0];
        assert_eq!(ev, 0x204);
        assert_eq!(l as u32 & 0xFFFF, 0x204);
        assert_eq!((l as u32) >> 16, 7);
        assert_eq!(w as u32 & 0xFFFF, 100);
        assert_eq!((w as u32) >> 16, 0xFFFE);
        assert_eq!(m[2].0, 0x7B);
    }

    #[test]
    fn legacy_right_click_uses_uid_wparam() {
        let m = callback_messages(TrayClick::Right, 0, 7, (1, 2));
        assert_eq!(m.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0x204, 0x205]);
        assert!(m.iter().all(|x| x.1 == 7));
        assert_eq!(m[0].2, 0x204);
    }
}
