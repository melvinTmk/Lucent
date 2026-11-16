//! `valo` search provider: current rank and the last 5 matches from HenrikDev.
//! Results are cached 120 s; a fetch runs on its own thread and, once done, asks the search
//! page to re-run its query (`search://refresh`).
use super::{Entry, Kind, OpenMods};
use crate::valorant::assets::{self, Kind as Asset, Want};
use crate::valorant::{api, summary_line, tier_fr, ApiError, MatchSummary, Mmr, ValoRow};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

mod text {
    pub const CONFIGURE: &str = "Configurer Valorant dans les Réglages";
    pub const OPEN_SETTINGS: &str = "Entrée : ouvrir les Réglages";
    pub const LOADING: &str = "Chargement…";
    pub const LOADING_SUB: &str = "Valorant · HenrikDev";
    pub const LAST_MATCH: &str = "dernier match";
}

const TTL: Duration = Duration::from_secs(120);
/// Errors (other than 429, which carries its own delay) are retried sooner.
const ERROR_TTL: Duration = Duration::from_secs(30);
const MATCH_COUNT: u32 = 5;
pub const REFRESH_EVENT: &str = "search://refresh";

/// `valo` alone or followed by a space (case-insensitive, byte-safe).
pub fn is_valo_query(q: &str) -> bool {
    let b = q.trim_start().as_bytes();
    match b.get(..4) {
        Some(p) if p.eq_ignore_ascii_case(b"valo") => b.len() == 4 || b[4] == b' ',
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Data {
    pub mmr: Mmr,
    pub matches: Vec<MatchSummary>,
}

pub type Outcome = Result<Data, ApiError>;

/// What the rows show.
#[derive(Debug, PartialEq)]
pub enum View {
    NotConfigured,
    Loading,
    Done(Outcome),
}

struct Cached {
    key: u64,
    at: Instant,
    outcome: Outcome,
}

#[derive(Default)]
pub struct Store {
    cached: Option<Cached>,
    loading: Option<u64>,
}

fn ttl(o: &Outcome) -> Duration {
    match o {
        Ok(_) => TTL,
        Err(ApiError::RateLimited(s)) => Duration::from_secs(*s),
        Err(_) => ERROR_TTL,
    }
}

impl Store {
    /// The outcome to show for account `key` (fresh or stale, with a 429 delay counted down)
    /// and whether a fetch must start now. Marks the fetch as started.
    pub fn lookup(&mut self, key: u64, now: Instant) -> (Option<Outcome>, bool) {
        let mut fresh = false;
        let shown = self.cached.as_ref().filter(|c| c.key == key).map(|c| {
            let age = now.saturating_duration_since(c.at);
            fresh = age < ttl(&c.outcome);
            match &c.outcome {
                Err(ApiError::RateLimited(s)) => Err(ApiError::RateLimited(s.saturating_sub(age.as_secs()).max(1))),
                o => o.clone(),
            }
        });
        let fetch = !fresh && self.loading != Some(key);
        if fetch {
            self.loading = Some(key);
        }
        (shown, fetch)
    }

    pub fn finish(&mut self, key: u64, at: Instant, outcome: Outcome) {
        self.cached = Some(Cached { key, at, outcome });
        if self.loading == Some(key) {
            self.loading = None;
        }
    }
}

fn row(id: String, title: String, subtitle: String) -> Entry {
    Entry { id, kind: Kind::Valorant, title, subtitle, icon: None, keywords: Vec::new(), valo: None }
}

fn signed(n: i32) -> String {
    format!("{n:+}")
}

/// Pictures of the rows: rank icon, then per match its map strip and agent icon.
pub fn wants(d: &Data) -> Vec<Want> {
    let mut w = vec![Want::tier(d.mmr.tier_id)];
    for m in d.matches.iter().take(MATCH_COUNT as usize) {
        w.push(Want::new(Asset::MapList, &m.map_id, &m.map));
        w.push(Want::new(Asset::AgentIcon, &m.agent_id, &m.agent));
    }
    w
}

/// Rows for `view`; `asset` gives the URL of a picture already cached (None otherwise).
pub fn entries(view: &View, asset: &dyn Fn(&Want) -> Option<String>) -> Vec<Entry> {
    match view {
        View::NotConfigured => vec![row("valo:config".into(), text::CONFIGURE.into(), text::OPEN_SETTINGS.into())],
        View::Loading => vec![row("valo:loading".into(), text::LOADING.into(), text::LOADING_SUB.into())],
        View::Done(Err(e)) => vec![row("valo:error".into(), e.message(), text::OPEN_SETTINGS.into())],
        View::Done(Ok(d)) => {
            let mut rank = row(
                "valo:rank".into(),
                format!("{} · {} RR", tier_fr(&d.mmr.tier), d.mmr.rr),
                format!("{} {} RR", text::LAST_MATCH, signed(d.mmr.last_change)),
            );
            let step = crate::valorant::next_step(d.mmr.tier_id, d.mmr.rr);
            rank.valo = Some(ValoRow::Rank {
                tier: tier_fr(&d.mmr.tier),
                rr: d.mmr.rr,
                last_change: d.mmr.last_change,
                tier_icon: asset(&Want::tier(d.mmr.tier_id)),
                next_tier: step.as_ref().map(|s| s.0.clone()),
                rr_to_next: step.map(|s| s.1),
            });
            let mut out = vec![rank];
            out.extend(d.matches.iter().take(MATCH_COUNT as usize).map(|m| {
                let mut r = row(format!("valo:match:{}", m.id), summary_line(m, None), m.agent.clone());
                r.valo = Some(ValoRow::Match {
                    won: m.won,
                    score: m.score,
                    map: m.map.clone(),
                    agent: m.agent.clone(),
                    kda: m.kda,
                    hs_pct: m.hs_pct,
                    acs: m.acs,
                    competitive: m.is_competitive(),
                    map_image: asset(&Want::new(Asset::MapList, &m.map_id, &m.map)),
                    agent_icon: asset(&Want::new(Asset::AgentIcon, &m.agent_id, &m.agent)),
                });
                r
            }));
            out
        }
    }
}

// ---------- wiring ----------

static APP: OnceLock<AppHandle> = OnceLock::new();
static STORE: LazyLock<Mutex<Store>> = LazyLock::new(Default::default);

/// Identifies the account + key in the cache without keeping the key around in clear.
fn account_key(a: &api::Account, s: &crate::settings::ValorantSettings) -> u64 {
    let mut h = DefaultHasher::new();
    (&a.name, &a.tag, &a.region, s.api_key.trim()).hash(&mut h);
    h.finish()
}

fn fetch(a: &api::Account) -> Outcome {
    let mmr = api::fetch_mmr(a)?;
    let matches = api::fetch_matches(a, MATCH_COUNT)?;
    Ok(Data { mmr, matches })
}

/// Called by the dynamic provider (off the main thread) for `valo` queries.
pub fn provider() -> Vec<Entry> {
    let Some(app) = APP.get() else { return Vec::new() };
    let settings = app.state::<crate::settings::SettingsStore>().get().valorant;
    let Some(account) = api::Account::from_settings(&settings) else {
        return entries(&View::NotConfigured, &no_assets);
    };
    let key = account_key(&account, &settings);
    let (shown, start) = STORE.lock().unwrap_or_else(|e| e.into_inner()).lookup(key, Instant::now());
    if start {
        let app = app.clone();
        let spawned = std::thread::Builder::new().name("valo-fetch".into()).spawn(move || {
            let outcome = fetch(&account);
            let pictures = outcome.as_ref().map(wants).unwrap_or_default();
            STORE.lock().unwrap_or_else(|e| e.into_inner()).finish(key, Instant::now(), outcome);
            refresh(&app);
            // Text rows first; the pictures follow once downloaded (cached on disk after).
            let missing: Vec<Want> = pictures.into_iter().filter(|w| assets::cached(w).is_none()).collect();
            if !missing.is_empty() && assets::ensure_all(&missing).iter().any(Option::is_some) {
                refresh(&app);
            }
        });
        if let Err(e) = spawned {
            eprintln!("lucent: cannot start valorant fetch: {e}");
            STORE.lock().unwrap_or_else(|e| e.into_inner()).finish(key, Instant::now(), Err(ApiError::Unreachable));
        }
    }
    entries(&shown.map_or(View::Loading, View::Done), &assets::cached)
}

fn no_assets(_: &Want) -> Option<String> {
    None
}

fn refresh(app: &AppHandle) {
    if let Err(e) = app.emit_to(crate::search_window::LABEL, REFRESH_EVENT, ()) {
        eprintln!("lucent: cannot emit {REFRESH_EVENT}: {e}");
    }
}

fn open(e: &Entry, _mods: OpenMods) {
    if e.id == "valo:config" || e.id == "valo:error" {
        let Some(app) = APP.get() else { return };
        if let Err(err) = crate::settings_window::open_tab(app, Some("valorant")) {
            eprintln!("lucent: cannot open settings window: {err}");
        }
    }
}

pub fn init(app: AppHandle) {
    let _ = APP.set(app);
    super::register_opener(Kind::Valorant, open);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> Data {
        let m = MatchSummary {
            id: "m1".into(),
            map: "Ascent".into(),
            queue: "competitive".into(),
            won: Some(true),
            score: (13, 9),
            kda: (24, 15, 6),
            agent: "Jett".into(),
            ..Default::default()
        };
        Data {
            mmr: Mmr { tier: "Platinum 2".into(), tier_id: 17, rr: 47, last_change: 18, elo: 1547 },
            matches: vec![m.clone(); 7].into_iter().enumerate().map(|(i, mut m)| { m.id = format!("m{i}"); m }).collect(),
        }
    }

    fn entries_plain(v: &View) -> Vec<Entry> {
        entries(v, &no_assets)
    }

    #[test]
    fn structured_rows_with_and_without_pictures() {
        let plain = entries_plain(&View::Done(Ok(data())));
        assert_eq!(
            plain[0].valo,
            Some(ValoRow::Rank {
                tier: "Platine 2".into(),
                rr: 47,
                last_change: 18,
                tier_icon: None,
                next_tier: Some("Diamant 1".into()), // tier_id 17 is Platinum 3
                rr_to_next: Some(53),
            })
        );
        match &plain[1].valo {
            Some(ValoRow::Match { won, score, map, kda, map_image, agent_icon, competitive, .. }) => {
                assert_eq!((*won, *score, map.as_str(), *kda, *competitive), (Some(true), (13, 9), "Ascent", (24, 15, 6), true));
                assert_eq!((map_image, agent_icon), (&None, &None));
            }
            other => panic!("{other:?}"),
        }
        let with = entries(&View::Done(Ok(data())), &|w: &Want| Some(format!("u:{:?}", w.kind)));
        assert!(matches!(&with[0].valo, Some(ValoRow::Rank { tier_icon: Some(u), .. }) if u == "u:Tier"));
        assert!(matches!(&with[1].valo, Some(ValoRow::Match { map_image: Some(m), agent_icon: Some(a), .. }) if m == "u:MapList" && a == "u:AgentIcon"));
        let w = wants(&data());
        assert_eq!(w.len(), 1 + 2 * 5);
        assert_eq!(w[0], Want::tier(17));
        // Error / loading rows carry no Valorant data.
        assert_eq!(entries_plain(&View::Loading)[0].valo, None);
        let json = serde_json::to_value(&plain[0]).unwrap();
        assert_eq!(json["valo"]["type"], "rank");
        assert_eq!(json["valo"]["lastChange"], 18);
        assert!(serde_json::to_value(&entries_plain(&View::Loading)[0]).unwrap().get("valo").is_none());
    }

    #[test]
    fn valo_query_prefix() {
        for q in ["valo", "VALO", "  valo", "valo ", "valo x"] {
            assert!(is_valo_query(q), "{q:?}");
        }
        for q in ["val", "valorant", "valo#", "xvalo", "", "é", "valé"] {
            assert!(!is_valo_query(q), "{q:?}");
        }
    }

    #[test]
    fn rows_for_each_view() {
        let nc = entries_plain(&View::NotConfigured);
        assert_eq!(nc.len(), 1);
        assert_eq!((nc[0].id.as_str(), nc[0].title.as_str()), ("valo:config", "Configurer Valorant dans les Réglages"));
        assert_eq!(entries_plain(&View::Loading)[0].title, "Chargement…");
        assert_eq!(entries_plain(&View::Done(Err(ApiError::Unauthorized)))[0].title, "Clé HenrikDev refusée");
        assert_eq!(entries_plain(&View::Done(Err(ApiError::NotFound)))[0].title, "Joueur introuvable");
        assert_eq!(entries_plain(&View::Done(Err(ApiError::Unreachable)))[0].title, "API injoignable");
        assert_eq!(entries_plain(&View::Done(Err(ApiError::RateLimited(9))))[0].title, "Trop de requêtes, réessaie dans 9 s");

        let ok = entries_plain(&View::Done(Ok(data())));
        assert_eq!(ok.len(), 6); // rank + 5 matches
        assert_eq!(ok[0].title, "Platine 2 · 47 RR");
        assert_eq!(ok[0].subtitle, "dernier match +18 RR");
        assert_eq!(ok[1].title, "VICTOIRE 13-9 · Ascent · 24/15/6");
        assert_eq!(ok[1].id, "valo:match:m0");
        assert!(ok.iter().all(|e| e.kind == Kind::Valorant));
    }

    #[test]
    fn valorant_group_comes_first() {
        let app = Entry { id: "a".into(), kind: Kind::App, title: "VALORANT".into(), subtitle: String::new(), icon: None, keywords: vec![], valo: None };
        let gs = super::super::rank::rank("valo", &[app], &super::super::history::History::empty(), 8);
        let mut gs = gs;
        super::super::rank::merge_dynamic(&mut gs, entries_plain(&View::Loading), 8);
        assert_eq!(gs.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(), ["valorant", "app"]);
    }

    #[test]
    fn cache_fetches_once_then_serves_until_ttl() {
        let mut s = Store::default();
        let t0 = Instant::now();
        assert_eq!(s.lookup(1, t0), (None, true));
        assert_eq!(s.lookup(1, t0), (None, false)); // already loading: no second fetch
        s.finish(1, t0, Ok(data()));
        assert_eq!(s.lookup(1, t0 + Duration::from_secs(119)), (Some(Ok(data())), false));
        // Stale: the old data stays shown while one refetch starts.
        assert_eq!(s.lookup(1, t0 + Duration::from_secs(121)), (Some(Ok(data())), true));
        assert_eq!(s.lookup(1, t0 + Duration::from_secs(122)), (Some(Ok(data())), false));
        // Another account (settings changed) does not see the cache.
        assert_eq!(s.lookup(2, t0 + Duration::from_secs(122)), (None, true));
    }

    #[test]
    fn rate_limit_counts_down_and_errors_retry_sooner() {
        let mut s = Store::default();
        let t0 = Instant::now();
        s.lookup(1, t0);
        s.finish(1, t0, Err(ApiError::RateLimited(20)));
        assert_eq!(s.lookup(1, t0 + Duration::from_secs(5)), (Some(Err(ApiError::RateLimited(15))), false));
        assert_eq!(s.lookup(1, t0 + Duration::from_secs(20)).1, true);
        s.finish(1, t0, Err(ApiError::Unreachable));
        assert_eq!(s.lookup(1, t0 + Duration::from_secs(29)).1, false);
        assert_eq!(s.lookup(1, t0 + Duration::from_secs(31)).1, true);
    }
}
