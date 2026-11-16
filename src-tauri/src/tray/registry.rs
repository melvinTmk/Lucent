//! Registry of tray icons, driven by parsed `NIM_*` messages.
use super::protocol::{
    NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_STATE, NIF_TIP, NIS_HIDDEN, NimMessage, NotifyIconData,
};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayKey {
    Guid([u8; 16]),
    HwndUid(u32, u32),
}

impl TrayKey {
    /// URL-safe (hex) identifier, used as `TrayIconView.key`.
    pub fn as_string(&self) -> String {
        match self {
            TrayKey::Guid(g) => {
                let mut s = String::from("g");
                for b in g {
                    s.push_str(&format!("{b:02x}"));
                }
                s
            }
            TrayKey::HwndUid(h, u) => format!("h{h:08x}u{u:08x}"),
        }
    }

    fn of(d: &NotifyIconData) -> TrayKey {
        match d.guid {
            Some(g) if d.flags & NIF_GUID != 0 && g != [0; 16] => TrayKey::Guid(g),
            _ => TrayKey::HwndUid(d.hwnd, d.uid),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrayIcon {
    pub key: TrayKey,
    pub data: NotifyIconData,
    pub icon_png: Option<Vec<u8>>,
    pub promoted: bool,
    pub owner_pid: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrayIconView {
    pub key: String,
    pub tip: String,
    pub icon_url: String,
    pub promoted: bool,
}

#[derive(Debug, Default)]
pub struct TrayRegistry {
    icons: Vec<TrayIcon>,
}

impl TrayRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn icons(&self) -> &[TrayIcon] {
        &self.icons
    }

    pub fn icons_mut(&mut self) -> &mut [TrayIcon] {
        &mut self.icons
    }

    /// Find by guid key first, then by (hwnd, uid) so guid icons stay reachable by legacy modifies.
    fn position(&self, d: &NotifyIconData) -> Option<usize> {
        let key = TrayKey::of(d);
        self.icons.iter().position(|i| i.key == key).or_else(|| {
            self.icons
                .iter()
                .position(|i| i.data.hwnd == d.hwnd && i.data.uid == d.uid)
        })
    }

    fn merge(icon: &mut TrayIcon, d: &NotifyIconData) {
        let f = d.flags;
        // Matched by GUID after the owner window was recreated: retarget to the live window.
        icon.data.hwnd = d.hwnd;
        icon.data.uid = d.uid;
        if f & NIF_MESSAGE != 0 {
            icon.data.callback_message = d.callback_message;
        }
        if f & NIF_ICON != 0 {
            if icon.data.hicon != d.hicon {
                icon.icon_png = None;
            }
            icon.data.hicon = d.hicon;
        }
        if f & NIF_TIP != 0 {
            icon.data.tip = d.tip.clone();
        }
        if f & NIF_STATE != 0 {
            icon.data.state = (icon.data.state & !d.state_mask) | (d.state & d.state_mask);
            icon.data.state_mask |= d.state_mask;
        }
        icon.data.flags |= f;
        // `version` is deliberately untouched here.
    }

    /// Returns the result `Shell_NotifyIcon` must report.
    pub fn apply(&mut self, msg: NimMessage, d: NotifyIconData) -> bool {
        let pos = self.position(&d);
        match msg {
            NimMessage::Add => {
                if let Some(i) = pos {
                    Self::merge(&mut self.icons[i], &d);
                } else {
                    let mut data = d.clone();
                    // The offset-800 union is uTimeout for Add/Modify; only SETVERSION sets version.
                    data.version = 0;
                    data.state = if d.flags & NIF_STATE != 0 { d.state & d.state_mask } else { 0 };
                    self.icons.push(TrayIcon {
                        key: TrayKey::of(&d),
                        data,
                        icon_png: None,
                        promoted: false,
                        owner_pid: 0,
                    });
                }
                true
            }
            NimMessage::Modify => match pos {
                Some(i) => {
                    Self::merge(&mut self.icons[i], &d);
                    true
                }
                None => false,
            },
            NimMessage::Delete => match pos {
                Some(i) => {
                    self.icons.remove(i);
                    true
                }
                None => false,
            },
            NimMessage::SetVersion => match pos {
                Some(i) => {
                    self.icons[i].data.version = d.version;
                    true
                }
                None => false,
            },
            NimMessage::SetFocus => pos.is_some(),
        }
    }

    /// Drop icons whose owner window is gone. Returns true if anything was removed.
    pub fn prune(&mut self, is_alive: impl Fn(u32) -> bool) -> bool {
        let before = self.icons.len();
        self.icons.retain(|i| is_alive(i.data.hwnd));
        self.icons.len() != before
    }

    /// Visible icons only, promoted first (stable otherwise).
    pub fn views(&self) -> Vec<TrayIconView> {
        let mut v: Vec<&TrayIcon> = self
            .icons
            .iter()
            .filter(|i| i.data.state & NIS_HIDDEN == 0)
            .collect();
        v.sort_by_key(|i| !i.promoted);
        v.into_iter()
            .map(|i| {
                let key = i.key.as_string();
                TrayIconView {
                    icon_url: format!("http://icon.localhost/tray-{key}.png"),
                    key,
                    tip: i.data.tip.clone(),
                    promoted: i.promoted,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nid(hwnd: u32, uid: u32, flags: u32) -> NotifyIconData {
        NotifyIconData { hwnd, uid, flags, ..Default::default() }
    }

    #[test]
    fn add_then_delete() {
        let mut r = TrayRegistry::new();
        assert!(r.apply(NimMessage::Add, nid(1, 1, 0)));
        assert_eq!(r.views().len(), 1);
        assert!(r.apply(NimMessage::Delete, nid(1, 1, 0)));
        assert!(r.views().is_empty());
        assert!(!r.apply(NimMessage::Delete, nid(1, 1, 0)));
    }

    #[test]
    fn add_existing_acts_as_modify() {
        let mut r = TrayRegistry::new();
        let mut a = nid(1, 1, NIF_TIP);
        a.tip = "a".into();
        r.apply(NimMessage::Add, a.clone());
        a.tip = "b".into();
        assert!(r.apply(NimMessage::Add, a));
        assert_eq!(r.icons().len(), 1);
        assert_eq!(r.views()[0].tip, "b");
    }

    #[test]
    fn modify_only_touches_flagged_fields() {
        let mut r = TrayRegistry::new();
        let mut a = nid(1, 1, NIF_TIP | NIF_ICON);
        a.tip = "old".into();
        a.hicon = 42;
        r.apply(NimMessage::Add, a);
        let mut m = nid(1, 1, NIF_TIP);
        m.tip = "new".into();
        m.hicon = 99;
        assert!(r.apply(NimMessage::Modify, m));
        assert_eq!(r.icons()[0].data.hicon, 42);
        assert_eq!(r.icons()[0].data.tip, "new");
        assert!(!r.apply(NimMessage::Modify, nid(5, 5, NIF_TIP)));
    }

    #[test]
    fn guid_key_wins_over_hwnd_uid() {
        let mut r = TrayRegistry::new();
        let mut a = nid(1, 1, NIF_GUID);
        a.guid = Some([7; 16]);
        r.apply(NimMessage::Add, a.clone());
        a.hwnd = 2; // owner window recreated, same guid
        r.apply(NimMessage::Add, a);
        assert_eq!(r.icons().len(), 1);
        assert_eq!(r.icons()[0].key, TrayKey::Guid([7; 16]));
        assert!(r.icons()[0].key.as_string().starts_with('g'));
    }

    #[test]
    fn guid_readd_updates_hwnd_and_survives_prune() {
        let mut r = TrayRegistry::new();
        let mut a = nid(1, 1, NIF_GUID);
        a.guid = Some([7; 16]);
        r.apply(NimMessage::Add, a.clone());
        a.hwnd = 2;
        a.uid = 3;
        r.apply(NimMessage::Add, a);
        assert_eq!((r.icons()[0].data.hwnd, r.icons()[0].data.uid), (2, 3));
        assert!(!r.prune(|h| h == 2));
        assert_eq!(r.icons().len(), 1);
    }

    #[test]
    fn utimeout_never_stored_as_version() {
        const NIF_INFO: u32 = 0x10;
        let mut r = TrayRegistry::new();
        let mut a = nid(1, 1, NIF_INFO);
        a.version = 10000;
        r.apply(NimMessage::Add, a.clone());
        assert_eq!(r.icons()[0].data.version, 0);
        let mut v = nid(1, 1, 0);
        v.version = 4;
        r.apply(NimMessage::SetVersion, v);
        assert_eq!(r.icons()[0].data.version, 4);
        r.apply(NimMessage::Modify, a);
        assert_eq!(r.icons()[0].data.version, 4);
    }

    #[test]
    fn hidden_state_excluded_from_views() {
        let mut r = TrayRegistry::new();
        let mut a = nid(1, 1, NIF_STATE);
        a.state = NIS_HIDDEN;
        a.state_mask = NIS_HIDDEN;
        r.apply(NimMessage::Add, a);
        assert!(r.views().is_empty());
        let mut m = nid(1, 1, NIF_STATE);
        m.state_mask = NIS_HIDDEN;
        r.apply(NimMessage::Modify, m);
        assert_eq!(r.views().len(), 1);
    }

    #[test]
    fn prune_drops_dead_owners() {
        let mut r = TrayRegistry::new();
        r.apply(NimMessage::Add, nid(1, 1, 0));
        r.apply(NimMessage::Add, nid(2, 1, 0));
        assert!(r.prune(|h| h == 2));
        assert_eq!(r.icons().len(), 1);
        assert!(!r.prune(|_| true));
    }

    #[test]
    fn views_promoted_first_and_url_safe() {
        let mut r = TrayRegistry::new();
        r.apply(NimMessage::Add, nid(1, 1, 0));
        r.apply(NimMessage::Add, nid(2, 1, 0));
        r.icons_mut()[1].promoted = true;
        let v = r.views();
        assert!(v[0].promoted);
        assert!(v[0].icon_url.starts_with("http://icon.localhost/tray-h"));
        assert!(v[0].icon_url.ends_with(".png"));
    }

    #[test]
    fn set_version_stored() {
        let mut r = TrayRegistry::new();
        r.apply(NimMessage::Add, nid(1, 1, 0));
        let mut v = nid(1, 1, 0);
        v.version = 4;
        assert!(r.apply(NimMessage::SetVersion, v));
        assert_eq!(r.icons()[0].data.version, 4);
    }
}
