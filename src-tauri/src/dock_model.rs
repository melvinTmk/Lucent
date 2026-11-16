//! Dock item model: pure logic (ordering, click outcome, pin/unpin). No Win32 here.
use crate::icons::{cache_key, icon_url};
use crate::settings::{AppId, PinnedApp};
use crate::wintrack::TrackedWindow;
use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WindowRef {
    pub hwnd: isize,
    pub title: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DockItem {
    /// `cache_key(app)`.
    pub id: String,
    pub app: AppId,
    pub name: String,
    pub icon_url: String,
    pub pinned: bool,
    pub is_game: bool,
    pub windows: Vec<WindowRef>,
    pub badge: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClickOutcome {
    Launch(AppId),
    Focus(isize),
    /// Item id.
    ShowThumbs(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextAction {
    Pin,
    Unpin,
    Quit,
}

fn windows_of(app: &AppId, windows: &[TrackedWindow]) -> Vec<WindowRef> {
    windows
        .iter()
        .filter(|w| &w.app == app)
        .map(|w| WindowRef { hwnd: w.hwnd, title: w.title.clone() })
        .collect()
}

fn badge_of(windows: &[WindowRef]) -> Option<String> {
    windows.iter().find_map(|w| crate::wintrack::badge::extract_badge(&w.title))
}

/// Order: pinned non-games (pin order), running unpinned apps (first-seen order),
/// pinned games (pin order). `names` supplies the label of unpinned running apps.
pub fn build_items(
    pinned: &[PinnedApp],
    windows: &[TrackedWindow],
    names: &dyn Fn(&AppId) -> String,
) -> Vec<DockItem> {
    let from_pin = |p: &PinnedApp| {
        let wins = windows_of(&p.app, windows);
        let badge = badge_of(&wins);
        DockItem {
            id: cache_key(&p.app),
            app: p.app.clone(),
            name: p.name.clone(),
            icon_url: icon_url(&p.app),
            pinned: true,
            is_game: p.is_game,
            windows: wins,
            badge,
        }
    };
    let mut items: Vec<DockItem> = pinned.iter().filter(|p| !p.is_game).map(from_pin).collect();
    let mut seen: Vec<&AppId> = Vec::new();
    for w in windows {
        if pinned.iter().any(|p| p.app == w.app) || seen.contains(&&w.app) {
            continue;
        }
        seen.push(&w.app);
        let wins = windows_of(&w.app, windows);
        let badge = badge_of(&wins);
        items.push(DockItem {
            id: cache_key(&w.app),
            app: w.app.clone(),
            name: names(&w.app),
            icon_url: icon_url(&w.app),
            pinned: false,
            is_game: false,
            windows: wins,
            badge,
        });
    }
    items.extend(pinned.iter().filter(|p| p.is_game).map(from_pin));
    items
}

/// Spec 4.1: 0 windows launch, 1 focus, 2+ thumbnails.
pub fn click_outcome(item: &DockItem) -> ClickOutcome {
    match item.windows.as_slice() {
        [] => ClickOutcome::Launch(item.app.clone()),
        [w] => ClickOutcome::Focus(w.hwnd),
        _ => ClickOutcome::ShowThumbs(item.id.clone()),
    }
}

pub fn apply_context(mut pinned: Vec<PinnedApp>, item: &DockItem, action: ContextAction) -> Vec<PinnedApp> {
    match action {
        ContextAction::Pin => {
            if !pinned.iter().any(|p| p.app == item.app) {
                pinned.push(PinnedApp { app: item.app.clone(), name: item.name.clone(), is_game: item.is_game });
            }
        }
        ContextAction::Unpin => pinned.retain(|p| p.app != item.app),
        ContextAction::Quit => {}
    }
    pinned
}

/// Drag-reorder. `items` is the current visible order (`build_items`). Moves `moved_id`
/// before `before_id` (`None` = end of its section; sections: non-games / games). The moved
/// item becomes pinned. Crossing sections, unknown ids or a drop onto itself leave `pinned` as is.
pub fn reorder(pinned: Vec<PinnedApp>, items: &[DockItem], moved_id: &str, before_id: Option<&str>) -> Vec<PinnedApp> {
    let Some(moved) = items.iter().find(|i| i.id == moved_id) else { return pinned };
    if before_id == Some(moved_id) {
        return pinned;
    }
    if let Some(b) = before_id {
        let Some(target) = items.iter().find(|i| i.id == b) else { return pinned };
        if target.is_game != moved.is_game {
            return pinned;
        }
    }
    let mut out: Vec<PinnedApp> = Vec::with_capacity(pinned.len() + 1);
    for games in [false, true] {
        let mut order: Vec<&DockItem> = items.iter().filter(|i| i.is_game == games && i.id != moved_id).collect();
        if moved.is_game == games {
            let at = before_id
                .and_then(|b| order.iter().position(|i| i.id == b))
                .unwrap_or(order.len());
            order.insert(at, moved);
        }
        for it in &order {
            if let Some(p) = pinned.iter().find(|p| p.app == it.app) {
                out.push(p.clone());
            } else if it.id == moved_id {
                out.push(PinnedApp { app: it.app.clone(), name: it.name.clone(), is_game: it.is_game });
            }
        }
        // Pinned entries not in `items` (defensive): keep them after their section.
        for p in pinned.iter().filter(|p| p.is_game == games && !items.iter().any(|i| i.app == p.app)) {
            out.push(p.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe(s: &str) -> AppId { AppId::Exe(s.into()) }
    fn pin(s: &str, game: bool) -> PinnedApp { PinnedApp { app: exe(s), name: s.into(), is_game: game } }
    fn win(h: isize, s: &str) -> TrackedWindow { TrackedWindow { hwnd: h, title: format!("w{h}"), app: exe(s) } }
    fn names(a: &AppId) -> String { format!("{a:?}") }
    fn item(n: usize) -> DockItem {
        DockItem {
            id: "k".into(), app: exe("a"), name: "A".into(), icon_url: String::new(), pinned: true, is_game: false,
            windows: (0..n).map(|i| WindowRef { hwnd: 10 + i as isize, title: String::new() }).collect(),
            badge: None,
        }
    }

    #[test]
    fn item_extracts_badge_from_window_title() {
        let pins = [pin("discord", false)];
        let wins = [TrackedWindow { hwnd: 1, title: "(3) Discord | #general".into(), app: exe("discord") }];
        let items = build_items(&pins, &wins, &names);
        assert_eq!(items[0].badge.as_deref(), Some("3"));
    }

    #[test]
    fn pinned_first_then_running_then_games() {
        let pins = [pin("explorer", false), pin("valorant", true)];
        let wins = [win(1, "code"), win(2, "explorer")];
        let items = build_items(&pins, &wins, &names);
        let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
        assert_eq!(ids, [cache_key(&exe("explorer")), cache_key(&exe("code")), cache_key(&exe("valorant"))]);
        assert!(!items[1].pinned && items[2].is_game && items[2].windows.is_empty());
    }

    #[test]
    fn running_pinned_app_gets_windows() {
        let items = build_items(&[pin("explorer", false)], &[win(1, "explorer"), win(2, "explorer")], &names);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].windows.len(), 2);
        assert!(items[0].pinned);
    }

    #[test]
    fn unpinned_closed_app_disappears() {
        let items = build_items(&[pin("explorer", false)], &[], &names);
        assert!(items.iter().all(|i| i.app != exe("code")));
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn running_unpinned_apps_group_in_first_seen_order() {
        let items = build_items(&[], &[win(1, "b"), win(2, "a"), win(3, "b")], &names);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].app, exe("b"));
        assert_eq!(items[0].windows.len(), 2);
    }

    #[test] fn click_zero_windows_launches() { assert!(matches!(click_outcome(&item(0)), ClickOutcome::Launch(_))); }
    #[test] fn click_one_window_focuses() { assert_eq!(click_outcome(&item(1)), ClickOutcome::Focus(10)); }
    #[test] fn click_many_windows_shows_thumbs() { assert_eq!(click_outcome(&item(3)), ClickOutcome::ShowThumbs("k".into())); }

    #[test]
    fn pin_then_unpin_roundtrip() {
        let orig = vec![pin("explorer", false)];
        let it = item(0);
        let pinned = apply_context(orig.clone(), &it, ContextAction::Pin);
        assert_eq!(pinned.len(), 2);
        assert_eq!(apply_context(pinned, &it, ContextAction::Unpin), orig);
    }

    #[test]
    fn pin_is_idempotent() {
        let it = item(0);
        let once = apply_context(vec![], &it, ContextAction::Pin);
        assert_eq!(apply_context(once.clone(), &it, ContextAction::Pin), once);
    }

    #[test]
    fn quit_leaves_pins_unchanged() {
        let orig = vec![pin("a", false)];
        assert_eq!(apply_context(orig.clone(), &item(2), ContextAction::Quit), orig);
    }

    fn ids(p: &[PinnedApp]) -> Vec<String> { p.iter().map(|x| format!("{:?}", x.app)).collect() }
    fn k(s: &str) -> String { cache_key(&exe(s)) }
    fn ex(v: &[&str]) -> Vec<String> { v.iter().map(|s| format!("{:?}", exe(s))).collect() }

    #[test]
    fn reorder_moves_pinned_before_another() {
        let pins = vec![pin("a", false), pin("b", false), pin("c", false)];
        let items = build_items(&pins, &[], &names);
        let r = reorder(pins, &items, &k("c"), Some(&k("a")));
        assert_eq!(ids(&r), ex(&["c", "a", "b"]));
    }

    #[test]
    fn reorder_to_section_end() {
        let pins = vec![pin("a", false), pin("b", false), pin("c", false)];
        let items = build_items(&pins, &[], &names);
        let r = reorder(pins, &items, &k("a"), None);
        assert_eq!(ids(&r), ex(&["b", "c", "a"]));
    }

    #[test]
    fn reorder_pins_running_unpinned_app() {
        let pins = vec![pin("a", false), pin("b", false)];
        let items = build_items(&pins, &[win(1, "x")], &names);
        let r = reorder(pins, &items, &k("x"), Some(&k("b")));
        assert_eq!(ids(&r), ex(&["a", "x", "b"]));
        assert!(!r[1].is_game);
        assert_eq!(r[1].name, items.iter().find(|i| i.id == k("x")).map(|i| i.name.clone()).unwrap_or_default());
    }

    #[test]
    fn reorder_running_dropped_at_end_becomes_last_pin() {
        let pins = vec![pin("a", false)];
        let items = build_items(&pins, &[win(1, "x"), win(2, "y")], &names);
        let r = reorder(pins, &items, &k("y"), None);
        assert_eq!(ids(&r), ex(&["a", "y"]));
    }

    #[test]
    fn reorder_refuses_crossing_sections() {
        let pins = vec![pin("a", false), pin("g", true)];
        let items = build_items(&pins, &[], &names);
        assert_eq!(reorder(pins.clone(), &items, &k("a"), Some(&k("g"))), pins);
    }

    #[test]
    fn reorder_unknown_id_is_noop() {
        let pins = vec![pin("a", false), pin("b", false)];
        let items = build_items(&pins, &[], &names);
        assert_eq!(reorder(pins.clone(), &items, "nope", Some(&k("a"))), pins);
        assert_eq!(reorder(pins.clone(), &items, &k("a"), Some("nope")), pins);
    }

    #[test]
    fn reorder_onto_itself_is_noop() {
        let pins = vec![pin("a", false), pin("b", false)];
        let items = build_items(&pins, &[], &names);
        assert_eq!(reorder(pins.clone(), &items, &k("a"), Some(&k("a"))), pins);
    }
}
