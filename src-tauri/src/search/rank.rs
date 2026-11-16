//! Ranking: nucleo fuzzy score on the title (keywords at 0.8x), prefix and initials
//! bonuses, then frecency. Nucleo folds case and latin diacritics itself.
use super::history::History;
use super::{Entry, Group, Hit, Kind};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{chars, Config, Matcher, Utf32Str};
use std::collections::HashSet;

/// Group display order (the `recent` group only exists for the empty query).
/// Valorant first: it only appears for an explicit `valo` query.
const ORDER: [Kind; 7] = [Kind::Valorant, Kind::App, Kind::Project, Kind::Command, Kind::Clipboard, Kind::Emoji, Kind::File];
const RECENT_LIMIT: usize = 8;
const KEYWORD_FACTOR: f32 = 0.8;
const PREFIX_BONUS: f32 = 1.25;
const INITIALS_BONUS: f32 = 1.15;

pub fn kind_key(k: Kind) -> &'static str {
    match k {
        Kind::App => "app",
        Kind::Project => "project",
        Kind::Command => "command",
        Kind::Clipboard => "clipboard",
        Kind::Emoji => "emoji",
        Kind::File => "file",
        Kind::Valorant => "valorant",
    }
}

/// Case + latin diacritics folding, one char to one char (so indices never shift).
pub fn fold(c: char) -> char {
    chars::to_lower_case(chars::normalize(c))
}

fn starts_with_folded(title: &str, query: &str) -> bool {
    let mut t = title.chars().map(fold);
    query.chars().map(fold).all(|q| t.next() == Some(q))
}

fn initials_equal(title: &str, query: &str) -> bool {
    let initials = title
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|w| w.chars().next())
        .map(fold);
    initials.eq(query.chars().map(fold))
}

pub fn rank(query: &str, entries: &[Entry], history: &History, limit_per_kind: usize) -> Vec<Group> {
    rank_slices(query, &[entries], history, limit_per_kind)
}

/// Same as `rank` over several slices (one per source), so callers never copy the index.
pub fn rank_slices(query: &str, slices: &[&[Entry]], history: &History, limit_per_kind: usize) -> Vec<Group> {
    // User input is literal: words are ANDed, and nucleo's own syntax (`!`, `^`, `'`, `$`)
    // is not interpreted (`Pattern::new` never parses it). Backslashes are dropped since
    // nucleo would read `\ ` as an escaped space.
    let cleaned = query.replace('\\', " ").split_whitespace().collect::<Vec<_>>().join(" ");
    let query = cleaned.as_str();
    if query.is_empty() {
        return recent(slices, history);
    }
    let pattern = Pattern::new(query, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy);
    let now = History::now();
    let used = !history.is_empty();
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let single_word = !query.contains(char::is_whitespace);
    // (score, slice index, entry index) per kind.
    let mut per_kind: Vec<Vec<(f32, usize, usize)>> = vec![Vec::new(); ORDER.len()];
    for (si, slice) in slices.iter().enumerate() {
        for (ei, entry) in slice.iter().enumerate() {
            let mut best = pattern
                .score(Utf32Str::new(&entry.title, &mut buf), &mut matcher)
                .map(|s| s as f32);
            for k in &entry.keywords {
                if let Some(s) = pattern.score(Utf32Str::new(k, &mut buf), &mut matcher) {
                    best = Some(best.unwrap_or(0.0).max(s as f32 * KEYWORD_FACTOR));
                }
            }
            let Some(mut score) = best else { continue };
            if starts_with_folded(&entry.title, query) {
                score *= PREFIX_BONUS;
            }
            if single_word && query.chars().count() >= 2 && initials_equal(&entry.title, query) {
                score *= INITIALS_BONUS;
            }
            if used {
                score *= history.boost_at(&entry.id, now);
            }
            let slot = ORDER.iter().position(|k| *k == entry.kind).expect("kind in ORDER");
            per_kind[slot].push((score, si, ei));
        }
    }
    let by_score = |a: &(f32, usize, usize), b: &(f32, usize, usize)| {
        b.0.total_cmp(&a.0)
            .then_with(|| slices[a.1][a.2].title.cmp(&slices[b.1][b.2].title))
            .then_with(|| slices[a.1][a.2].id.cmp(&slices[b.1][b.2].id))
    };
    let mut groups = Vec::new();
    for (slot, mut hits) in per_kind.into_iter().enumerate() {
        if hits.is_empty() || limit_per_kind == 0 {
            continue;
        }
        if hits.len() > limit_per_kind {
            hits.select_nth_unstable_by(limit_per_kind - 1, by_score);
            hits.truncate(limit_per_kind);
        }
        hits.sort_by(by_score);
        // Highlight indices are only computed for the few survivors.
        let items = hits
            .into_iter()
            .map(|(score, si, ei)| {
                let entry = &slices[si][ei];
                let mut indices = Vec::new();
                pattern.indices(Utf32Str::new(&entry.title, &mut buf), &mut matcher, &mut indices);
                indices.sort_unstable();
                indices.dedup();
                Hit { entry: entry.clone(), score, indices }
            })
            .collect();
        groups.push(Group { key: kind_key(ORDER[slot]).into(), items });
    }
    groups
}

/// Empty query: the most frecent entries that still exist, one mixed group.
fn recent(slices: &[&[Entry]], history: &History) -> Vec<Group> {
    let ids = history.top(RECENT_LIMIT * 8);
    if ids.is_empty() {
        return Vec::new();
    }
    let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let mut found = std::collections::HashMap::new();
    for e in slices.iter().flat_map(|s| s.iter()) {
        if wanted.contains(e.id.as_str()) {
            found.entry(e.id.as_str()).or_insert(e);
        }
    }
    let items: Vec<Hit> = ids
        .iter()
        .filter_map(|id| found.get(id.as_str()))
        .take(RECENT_LIMIT)
        .map(|e| Hit { entry: (*e).clone(), score: history.boost(&e.id), indices: Vec::new() })
        .collect();
    if items.is_empty() { Vec::new() } else { vec![Group { key: "recent".into(), items }] }
}

/// Dynamic entries (computed from the query by a provider, e.g. a calculator) are already
/// relevant: they lead their kind's group, whatever their title looks like.
///
/// Clipboard and emoji lists are dedicated modes (`cb`, `:`): their providers already bound
/// them (50 / 40), so they are not cut to `limit_per_kind`.
pub fn merge_dynamic(groups: &mut Vec<Group>, dynamic: Vec<Entry>, limit_per_kind: usize) {
    for entry in dynamic.into_iter().rev() {
        let key = kind_key(entry.kind);
        let capped = !matches!(entry.kind, Kind::Clipboard | Kind::Emoji);
        let hit = Hit { entry, score: f32::MAX, indices: Vec::new() };
        match groups.iter_mut().find(|g| g.key == key) {
            Some(g) => {
                g.items.insert(0, hit);
                if capped {
                    g.items.truncate(limit_per_kind.max(1));
                }
            }
            None => {
                let at = ORDER
                    .iter()
                    .position(|k| kind_key(*k) == key)
                    .map(|slot| groups.iter().take_while(|g| g.key == "recent" || order_of(&g.key) < slot).count())
                    .unwrap_or(groups.len());
                groups.insert(at, Group { key: key.into(), items: vec![hit] });
            }
        }
    }
}

fn order_of(key: &str) -> usize {
    ORDER.iter().position(|k| kind_key(*k) == key).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Kind;
    use std::time::Instant;

    fn e(id: &str, kind: Kind, title: &str) -> Entry {
        Entry { id: id.into(), kind, title: title.into(), subtitle: String::new(), icon: None, keywords: vec![], valo: None }
    }
    fn titles(g: &Group) -> Vec<&str> {
        g.items.iter().map(|h| h.entry.title.as_str()).collect()
    }
    fn group<'a>(gs: &'a [Group], key: &str) -> &'a Group {
        gs.iter().find(|g| g.key == key).unwrap_or_else(|| panic!("no group {key}"))
    }

    #[test]
    fn folds_diacritics() {
        let es = [e("1", Kind::App, "Réglages de Lucent")];
        let gs = rank("reglages", &es, &History::empty(), 8);
        assert_eq!(titles(group(&gs, "app")), ["Réglages de Lucent"]);
    }

    #[test]
    fn indices_point_into_the_original_title() {
        let es = [e("1", Kind::App, "Réglages")];
        let gs = rank("reg", &es, &History::empty(), 8);
        assert_eq!(group(&gs, "app").items[0].indices, vec![0, 1, 2]);
        let gs = rank("glag", &es, &History::empty(), 8);
        assert_eq!(group(&gs, "app").items[0].indices, vec![2, 3, 4, 5]);
    }

    #[test]
    fn initials_match_multiword_names() {
        let es = [
            e("1", Kind::App, "Savvy Scanner Clock"),
            e("2", Kind::App, "Visual Studio Code"),
            e("3", Kind::App, "VLC"),
        ];
        let gs = rank("vsc", &es, &History::empty(), 8);
        assert_eq!(titles(group(&gs, "app")), ["Visual Studio Code", "Savvy Scanner Clock"]);
    }

    #[test]
    fn prefix_beats_subsequence() {
        let es = [
            e("1", Kind::App, "Media Player Classic Displays"),
            e("2", Kind::App, "Discord"),
        ];
        let gs = rank("dis", &es, &History::empty(), 8);
        assert_eq!(titles(group(&gs, "app"))[0], "Discord");
    }

    #[test]
    fn frecency_breaks_close_scores() {
        let es = [e("a", Kind::App, "Notes"), e("b", Kind::App, "Notes")];
        let mut h = History::empty();
        for _ in 0..5 {
            h.bump("b");
        }
        let gs = rank("notes", &es, &h, 8);
        assert_eq!(group(&gs, "app").items[0].entry.id, "b");
    }

    #[test]
    fn keywords_match_below_the_title() {
        let mut k = e("1", Kind::Command, "Verrouiller");
        k.keywords = vec!["lock".into()];
        let gs = rank("lock", &[k], &History::empty(), 8);
        assert_eq!(group(&gs, "command").items[0].indices, Vec::<u32>::new());
    }

    #[test]
    fn limits_per_kind_and_orders_groups() {
        let mut es = Vec::new();
        for i in 0..5 {
            es.push(e(&format!("f{i}"), Kind::File, &format!("note{i}.txt")));
            es.push(e(&format!("a{i}"), Kind::App, &format!("Note app {i}")));
        }
        es.push(e("c", Kind::Command, "Note rapide"));
        es.push(e("p", Kind::Project, "notes-project"));
        es.push(e("m", Kind::Emoji, "note musique"));
        es.push(e("k", Kind::Clipboard, "note copiée"));
        let gs = rank("note", &es, &History::empty(), 3);
        let keys: Vec<&str> = gs.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, ["app", "project", "command", "clipboard", "emoji", "file"]);
        assert_eq!(group(&gs, "app").items.len(), 3);
        assert_eq!(group(&gs, "file").items.len(), 3);
    }

    #[test]
    fn empty_query_lists_recent_entries_only() {
        let es = [e("a", Kind::App, "Alpha"), e("b", Kind::Project, "Beta"), e("c", Kind::App, "Gamma")];
        assert!(rank("", &es, &History::empty(), 8).is_empty());
        let mut h = History::empty();
        h.bump("b");
        h.bump("b");
        h.bump("a");
        h.bump("gone");
        let gs = rank("  ", &es, &h, 8);
        assert_eq!(gs.len(), 1);
        assert_eq!(gs[0].key, "recent");
        assert_eq!(titles(&gs[0]), ["Beta", "Alpha"]);
    }

    #[test]
    fn dynamic_entries_lead_their_group() {
        let es = [e("1", Kind::Command, "Calculatrice")];
        let mut gs = rank("calc", &es, &History::empty(), 8);
        merge_dynamic(&mut gs, vec![e("d", Kind::Command, "= 4")], 8);
        assert_eq!(titles(group(&gs, "command")), ["= 4", "Calculatrice"]);
        let mut gs = Vec::new();
        merge_dynamic(&mut gs, vec![e("d", Kind::Command, "= 4")], 8);
        assert_eq!(gs[0].key, "command");
    }

    #[test]
    fn user_input_is_literal() {
        let es = [e("1", Kind::App, "Discord"), e("2", Kind::App, "Notes"), e("3", Kind::App, "c'est la vie"), e("4", Kind::App, "price$")];
        let h = History::empty();
        assert!(rank("!dis", &es, &h, 8).is_empty());
        assert_eq!(titles(group(&rank("dis", &es, &h, 8), "app")), ["Discord"]);
        assert!(rank("^", &es, &h, 8).is_empty());
        assert_eq!(titles(group(&rank("'", &es, &h, 8), "app")), ["c'est la vie"]);
        assert_eq!(titles(group(&rank("c'est", &es, &h, 8), "app")), ["c'est la vie"]);
        assert_eq!(titles(group(&rank("price$", &es, &h, 8), "app")), ["price$"]);
        assert!(rank("\\", &es, &h, 8).is_empty());
        assert_eq!(titles(group(&rank("dis cord", &es, &h, 8), "app")), ["Discord"]);
    }

    fn synthetic_200k() -> Vec<Entry> {
        (0..200_000)
            .map(|i| {
                let t = if i % 5000 == 0 { format!("README-{i}.md") } else { format!("dossier{}/archive_{}.txt", i % 977, i) };
                e(&i.to_string(), Kind::File, &t)
            })
            .collect()
    }

    #[test]
    #[ignore]
    fn rank_200k_dense_under_budget() {
        let es = synthetic_200k();
        let mut h = History::empty();
        for i in 0..300 {
            h.bump(&(i * 600).to_string());
        }
        for q in ["e", "er", "ar", "archive txt"] {
            let _ = rank(q, &es, &h, 8);
            let t = Instant::now();
            let gs = rank(q, &es, &h, 8);
            let ms = t.elapsed().as_millis();
            println!("rank 200k dense {q:?}: {ms} ms, {} hits", gs[0].items.len());
            assert!(ms < 50, "{q:?} took {ms} ms");
        }
    }

    #[test]
    #[ignore]
    fn rank_200k_under_budget() {
        let es = synthetic_200k();
        let h = History::empty();
        let _ = rank("readme", &es, &h, 8);
        let t = Instant::now();
        let gs = rank("readme", &es, &h, 8);
        let ms = t.elapsed().as_millis();
        println!("rank 200k: {ms} ms, {} hits", gs[0].items.len());
        assert!(ms < 50, "took {ms} ms");
    }
}
