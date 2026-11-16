//! Spotlight-style search: entries from several sources, ranked per kind.
pub mod apps;
pub mod clipboard;
pub mod commands;
pub mod emoji;
pub mod files;
pub mod history;
pub mod projects;
pub mod quicklook;
pub mod rank;
pub mod valo;
pub mod wsl;

use history::History;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, RwLock};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    App,
    Project,
    File,
    Command,
    Clipboard,
    Emoji,
    Valorant,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub subtitle: String,
    pub icon: Option<String>,
    pub keywords: Vec<String>,
    /// `valo` rows only: what the Valorant tiles draw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valo: Option<crate::valorant::ValoRow>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub entry: Entry,
    pub score: f32,
    /// Char positions in the ORIGINAL title of the matched characters (empty when the
    /// match came from a keyword or a dynamic entry).
    pub indices: Vec<u32>,
}

/// `key`: "recent" | "valorant" | "app" | "project" | "command" | "clipboard" | "emoji" | "file".
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub key: String,
    pub items: Vec<Hit>,
}

pub const LIMIT_PER_KIND: usize = 8;

/// Modifier keys held when the user validated a result.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenMods {
    pub shift: bool,
    pub ctrl: bool,
}

pub type Opener = fn(&Entry, OpenMods);
/// Returns entries computed from the query itself (calculator, web search, ...).
pub type DynamicProvider = fn(&str) -> Vec<Entry>;

static OPENERS: LazyLock<RwLock<HashMap<Kind, Opener>>> = LazyLock::new(Default::default);

/// Later tasks register how their kind is opened (launch app, open folder, ...).
pub fn register_opener(kind: Kind, f: Opener) {
    OPENERS.write().unwrap_or_else(|e| e.into_inner()).insert(kind, f);
}

/// Entries per source, shared between the sources (writers) and `search_query` (reader).
#[derive(Clone, Default)]
pub struct SearchIndex {
    sources: Arc<RwLock<HashMap<Kind, Vec<Entry>>>>,
    provider: Arc<RwLock<Option<DynamicProvider>>>,
}

impl SearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces everything a source knows (its previous entries are dropped).
    pub fn set_source(&self, kind: Kind, entries: Vec<Entry>) {
        self.sources.write().unwrap_or_else(|e| e.into_inner()).insert(kind, entries);
    }

    /// Flat copy of every entry (copies: meant for tests and diagnostics, not the query path).
    pub fn snapshot(&self) -> Vec<Entry> {
        let g = self.sources.read().unwrap_or_else(|e| e.into_inner());
        g.values().flatten().cloned().collect()
    }

    /// Hook for Task 8 (dynamic command results). One provider; none by default.
    pub fn set_dynamic_provider(&self, f: Option<DynamicProvider>) {
        *self.provider.write().unwrap_or_else(|e| e.into_inner()) = f;
    }

    /// Entries the provider computes for `q` (none for a blank query).
    fn dynamic(&self, q: &str) -> Vec<Entry> {
        let provider = *self.provider.read().unwrap_or_else(|e| e.into_inner());
        match provider {
            Some(f) if !q.trim().is_empty() => f(q),
            _ => Vec::new(),
        }
    }

    pub fn query(&self, q: &str, history: &History, limit: usize) -> Vec<Group> {
        let mut groups = {
            let g = self.sources.read().unwrap_or_else(|e| e.into_inner());
            let slices: Vec<&[Entry]> = g.values().map(Vec::as_slice).collect();
            rank::rank_slices(q, &slices, history, limit)
        };
        let dynamic = self.dynamic(q);
        if !dynamic.is_empty() {
            rank::merge_dynamic(&mut groups, dynamic, limit);
        }
        groups
    }

    /// The entry `id` as the results of query `q` showed it: source entries, else the
    /// provider's entries recomputed for `q` (dynamic ids such as `cmd:calc` only mean
    /// something together with their query, so nothing is remembered between queries).
    pub fn find(&self, id: &str, q: &str) -> Option<Entry> {
        let found = {
            let g = self.sources.read().unwrap_or_else(|e| e.into_inner());
            g.values().flatten().find(|e| e.id == id).cloned()
        };
        found.or_else(|| self.dynamic(q).into_iter().find(|e| e.id == id))
    }
}

pub type SharedHistory = Arc<Mutex<History>>;

pub fn load_history() -> SharedHistory {
    Arc::new(Mutex::new(History::load(&history::history_path())))
}

/// Bumps the history, then hands the entry to its kind's opener. `q` is the query whose
/// results held `id`. No lock is held while the opener runs. An unregistered kind is logged
/// and ignored.
pub fn open(index: &SearchIndex, history: &SharedHistory, id: &str, q: &str, mods: OpenMods) {
    let Some(entry) = index.find(id, q) else {
        eprintln!("lucent: search_open: unknown entry {id}");
        return;
    };
    // Clipboard ids are per-launch counters: bumping them would boost unrelated items later.
    if entry.kind != Kind::Clipboard {
        let mut h = history.lock().unwrap_or_else(|e| e.into_inner());
        h.bump(id);
        if let Err(e) = h.save() {
            eprintln!("lucent: cannot save search history: {e}");
        }
    }
    let opener = OPENERS.read().unwrap_or_else(|e| e.into_inner()).get(&entry.kind).copied();
    match opener {
        Some(f) => f(&entry, mods),
        None => eprintln!("lucent: no opener registered for {:?}", entry.kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static OPENED: AtomicU32 = AtomicU32::new(0);

    fn e(id: &str, kind: Kind, title: &str) -> Entry {
        Entry { id: id.into(), kind, title: title.into(), subtitle: String::new(), icon: None, keywords: vec![], valo: None }
    }

    #[test]
    fn open_bumps_history_and_dispatches_by_kind() {
        let idx = SearchIndex::new();
        idx.set_source(Kind::Emoji, vec![e("emoji:1", Kind::Emoji, "sourire")]);
        idx.set_source(Kind::Clipboard, vec![e("clip:1", Kind::Clipboard, "texte")]);
        let hist: SharedHistory = Arc::new(Mutex::new(History::empty()));
        register_opener(Kind::Emoji, |_, m| {
            assert!(m.shift && !m.ctrl);
            OPENED.fetch_add(1, Ordering::SeqCst);
        });
        open(&idx, &hist, "emoji:1", "sou", OpenMods { shift: true, ctrl: false });
        assert_eq!(OPENED.load(Ordering::SeqCst), 1);
        assert_eq!(hist.lock().unwrap().count("emoji:1"), 1);
        // No opener for Clipboard, unknown id: both are ignored without panicking.
        open(&idx, &hist, "clip:1", "", OpenMods::default());
        open(&idx, &hist, "nope", "nope", OpenMods::default());
        assert_eq!(OPENED.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn set_source_replaces_and_provider_results_are_merged() {
        let idx = SearchIndex::new();
        idx.set_source(Kind::App, vec![e("a", Kind::App, "Alpha")]);
        idx.set_source(Kind::App, vec![e("b", Kind::App, "Beta")]);
        assert_eq!(idx.snapshot().len(), 1);
        idx.set_dynamic_provider(Some(|q| vec![e("calc", Kind::Command, &format!("= {q}"))]));
        let gs = idx.query("bet", &History::empty(), 8);
        assert_eq!(gs.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(), ["app", "command"]);
        assert_eq!(idx.find("calc", "bet").unwrap().title, "= bet");
        assert_eq!(idx.find("b", "").unwrap().title, "Beta");
    }

    #[test]
    fn dynamic_entries_resolve_from_the_query_they_were_shown_for() {
        let idx = SearchIndex::new();
        idx.set_dynamic_provider(Some(|q| vec![e("cmd:calc", Kind::Command, &format!("= {q}"))]));
        let h = History::empty();
        // Queries answered out of order: the last one to finish no longer decides what opens.
        idx.query("2+23", &h, 8);
        idx.query("2+2", &h, 8);
        assert_eq!(idx.find("cmd:calc", "2+23").unwrap().title, "= 2+23");
        assert_eq!(idx.find("cmd:calc", "2+2").unwrap().title, "= 2+2");
        // Nothing is remembered: without a query there is no dynamic entry.
        assert!(idx.find("cmd:calc", "").is_none());
        assert!(idx.find("cmd:calc", "   ").is_none());
    }

    #[test]
    fn prefix_lists_are_uncapped_and_clipboard_is_not_bumped() {
        let idx = SearchIndex::new();
        idx.set_dynamic_provider(Some(|q| {
            let (kind, n) = if q == "cb" { (Kind::Clipboard, 50) } else { (Kind::Emoji, 40) };
            (0..n).map(|i| e(&format!("{q}:{i}"), kind, &format!("item {i}"))).collect()
        }));
        let h = History::empty();
        let gs = idx.query("cb", &h, LIMIT_PER_KIND);
        assert_eq!(gs.iter().find(|g| g.key == "clipboard").unwrap().items.len(), 50);
        let gs = idx.query(":e", &h, LIMIT_PER_KIND);
        assert_eq!(gs.iter().find(|g| g.key == "emoji").unwrap().items.len(), 40);

        assert!(idx.find("cb:3", "cb").is_some());
        assert!(idx.find("cb:3", ":e").is_none());

        let hist: SharedHistory = Arc::new(Mutex::new(History::empty()));
        open(&idx, &hist, "cb:3", "cb", OpenMods::default());
        assert_eq!(hist.lock().unwrap().count("cb:3"), 0);
    }
}
