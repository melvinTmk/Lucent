//! Apps source: every entry of the shell's AppsFolder (Store and desktop apps).
use super::{Entry, Kind, OpenMods, SearchIndex};
use std::time::{Duration, Instant};

/// The search window refreshes the list when it is older than this.
pub const MAX_AGE: Duration = Duration::from_secs(10 * 60);

/// Start-menu leftovers that are not apps (uninstallers, docs, web links).
fn is_noise(name: &str, parsing: &str) -> bool {
    let n = name.trim().to_lowercase();
    let p = parsing.trim().to_lowercase();
    n.is_empty()
        || n.starts_with("désinstaller")
        || n.starts_with("desinstaller")
        || n.starts_with("uninstall")
        || [".url", ".chm", ".txt", ".html"].iter().any(|ext| p.ends_with(ext))
}

/// `(display name, parsing name)` pairs → entries: noise dropped, same name keeps the first.
pub fn build_entries(items: Vec<(String, String)>) -> Vec<Entry> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (name, parsing) in items {
        if is_noise(&name, &parsing) || !seen.insert(name.trim().to_lowercase()) {
            continue;
        }
        let app = crate::settings::AppId::Aumid(parsing.clone());
        out.push(Entry {
            id: format!("app:{parsing}"),
            kind: Kind::App,
            title: name.trim().to_string(),
            subtitle: "Application".into(),
            icon: Some(crate::icons::icon_url(&app)),
            keywords: vec![],
            valo: None,
        });
    }
    out
}

fn is_stale(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.duration_since(t) >= MAX_AGE)
}

/// Opening an app launches it; modifiers are ignored.
pub fn open(entry: &Entry, _mods: OpenMods) {
    let Some(parsing) = entry.id.strip_prefix("app:") else { return };
    if let Err(e) = crate::actions::launch(&crate::settings::AppId::Aumid(parsing.to_string())) {
        eprintln!("lucent: cannot launch {}: {e}", entry.title);
    }
}

/// Registers the opener (call once at startup).
pub fn init() {
    super::register_opener(Kind::App, open);
}

static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
/// Parsing names (the AUMID for Store apps) seen at the last enumeration.
static PARSING_NAMES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn remember_parsing_names(items: &[(String, String)]) {
    *PARSING_NAMES.lock().unwrap_or_else(|e| e.into_inner()) = items.iter().map(|(_, p)| p.clone()).collect();
}

/// AUMID of an exe inside `WindowsApps` (which Windows refuses to launch by path), from
/// the AppsFolder. Uses the last enumeration, re-enumerating (tens of ms) on a miss.
pub fn aumid_for_packaged_exe(exe_path: &str) -> Option<String> {
    let family = crate::wintrack::packaged_family(exe_path)?;
    let cached = {
        let names = PARSING_NAMES.lock().unwrap_or_else(|e| e.into_inner());
        crate::wintrack::aumid_for_family(&family, names.iter().map(String::as_str))
    };
    if cached.is_some() {
        return cached;
    }
    #[cfg(windows)]
    match win::enumerate() {
        Ok(items) => {
            remember_parsing_names(&items);
            return crate::wintrack::aumid_for_family(&family, items.iter().map(|(_, p)| p.as_str()));
        }
        Err(e) => eprintln!("lucent: cannot enumerate apps: {e}"),
    }
    None
}
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Re-enumerates the AppsFolder and replaces the source. Blocking: run off the UI threads.
pub fn refresh(index: &SearchIndex) {
    #[cfg(windows)]
    match win::enumerate() {
        Ok(items) => {
            remember_parsing_names(&items);
            let entries = build_entries(items);
            index.set_source(Kind::App, entries);
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        }
        Err(e) => eprintln!("lucent: cannot enumerate apps: {e}"),
    }
    #[cfg(not(windows))]
    let _ = index;
}

/// Cheap: spawns a background refresh when the list is missing or stale, never blocks.
pub fn refresh_if_stale(index: &SearchIndex) {
    use std::sync::atomic::Ordering;
    let last = *LAST.lock().unwrap_or_else(|e| e.into_inner());
    if !is_stale(last, Instant::now()) || RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let index = index.clone();
    let spawned = std::thread::Builder::new().name("lucent-apps".into()).spawn(move || {
        refresh(&index);
        RUNNING.store(false, Ordering::SeqCst);
    });
    if let Err(e) = spawned {
        RUNNING.store(false, Ordering::SeqCst);
        eprintln!("lucent: cannot spawn apps refresh: {e}");
    }
}

#[cfg(windows)]
mod win {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, KF_FLAG_DEFAULT, SHGetKnownFolderItem,
        SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
    };
    use windows::core::Result;

    unsafe fn name(item: &IShellItem, kind: windows::Win32::UI::Shell::SIGDN) -> Result<String> {
        unsafe {
            let p = item.GetDisplayName(kind)?;
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            Ok(s)
        }
    }

    /// `(display name, parsing name)` of every AppsFolder item.
    pub fn enumerate() -> std::result::Result<Vec<(String, String)>, String> {
        unsafe {
            // Per-thread COM init; S_FALSE / RPC_E_CHANGED_MODE are fine to ignore.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let folder: IShellItem =
                SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None).map_err(|e| format!("AppsFolder: {e}"))?;
            let en: IEnumShellItems =
                folder.BindToHandler(None, &BHID_EnumItems).map_err(|e| format!("BindToHandler: {e}"))?;
            let mut out = Vec::new();
            loop {
                let mut slot: [Option<IShellItem>; 1] = [None];
                let mut got = 0u32;
                if en.Next(&mut slot, Some(&mut got)).is_err() || got == 0 {
                    break;
                }
                let Some(item) = slot[0].take() else { break };
                if let (Ok(n), Ok(p)) = (name(&item, SIGDN_NORMALDISPLAY), name(&item, SIGDN_PARENTRELATIVEPARSING)) {
                    out.push((n, p));
                }
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn dedups_same_name_keeps_first() {
        let es = build_entries(pairs(&[("Notepad", "A!App"), ("notepad", "B"), ("Paint", "C")]));
        assert_eq!(es.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["app:A!App", "app:C"]);
        assert_eq!(es[0].subtitle, "Application");
        assert!(es[0].icon.is_some());
    }

    #[test]
    fn skips_uninstallers_and_docs() {
        let es = build_entries(pairs(&[
            ("Désinstaller Foo", "C:\\x\\unins.exe"),
            ("Uninstall Bar", "bar"),
            ("Aide", "C:\\a\\help.chm"),
            ("Site", "C:\\a\\site.URL"),
            ("Lisez-moi", "C:\\a\\r.txt"),
            ("Doc", "C:\\a\\d.html"),
            ("", "empty"),
            ("Firefox", "Mozilla.Firefox"),
        ]));
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].title, "Firefox");
    }

    #[test]
    fn staleness() {
        let now = Instant::now();
        assert!(is_stale(None, now));
        assert!(!is_stale(Some(now), now));
        assert!(is_stale(Some(now), now + MAX_AGE));
    }

    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_apps_include_explorer_or_settings() {
        let idx = SearchIndex::new();
        refresh(&idx);
        let all = idx.snapshot();
        println!("apps found: {}", all.len());
        for e in all.iter().take(8) {
            println!("  {} -> {}", e.title, e.id);
        }
        assert!(all.iter().any(|e| {
            let t = e.title.to_lowercase();
            t.contains("explorer") || t.contains("paramètres") || t.contains("settings")
        }));
    }
}
