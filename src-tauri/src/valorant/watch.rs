//! Post-match summary card watcher for Valorant.
//! Active while Valorant is running (`valorant-win64-shipping.exe`).
//! Polls HenrikDev matches (size=1) and MMR, and emits `valo://match` with
//! a `ValoMatchCard` (text line + structured stats + picture URLs) when a new match is
//! detected.
use super::assets::{self, Kind, Want};
use super::{api, summary_line, tier_fr, ApiError, MatchSummary, Mmr};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub const DEFAULT_POLL: Duration = Duration::from_secs(180);
pub const EXIT_POLL: Duration = Duration::from_secs(30);
pub const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
pub const VALORANT_EXE: &str = "valorant-win64-shipping.exe";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValoMatchCard {
    /// Plain-text summary: the card's layout when no picture could be loaded.
    pub line: String,
    pub won: Option<bool>,
    #[serde(default)]
    pub map: String,
    #[serde(default)]
    pub map_id: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub competitive: bool,
    /// Rounds, the player's team first.
    #[serde(default)]
    pub score: (u32, u32),
    #[serde(default)]
    pub kda: (u32, u32, u32),
    #[serde(default)]
    pub hs_pct: Option<u32>,
    #[serde(default)]
    pub acs: Option<u32>,
    /// RR won/lost and the rank after the match (competitive with an MMR only).
    #[serde(default)]
    pub rr_change: Option<i32>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub tier_id: Option<u32>,
    /// RR inside the current tier, next rank and the RR missing to reach it.
    #[serde(default)]
    pub rr: Option<i32>,
    #[serde(default)]
    pub next_tier: Option<String>,
    #[serde(default)]
    pub rr_to_next: Option<u32>,
    /// `valo` scheme URLs (None: not downloaded, offline).
    #[serde(default)]
    pub map_splash: Option<String>,
    #[serde(default)]
    pub agent_portrait: Option<String>,
    #[serde(default)]
    pub tier_icon: Option<String>,
}

/// The card of match `m` (no pictures yet: see `with_assets`).
pub fn card_for(m: &MatchSummary, mmr: Option<&Mmr>) -> ValoMatchCard {
    let rank = mmr.filter(|_| m.is_competitive());
    let step = rank.and_then(|r| super::next_step(r.tier_id, r.rr));
    ValoMatchCard {
        line: summary_line(m, mmr),
        won: m.won,
        map: m.map.clone(),
        map_id: m.map_id.clone(),
        agent: m.agent.clone(),
        agent_id: m.agent_id.clone(),
        competitive: m.is_competitive(),
        score: m.score,
        kda: m.kda,
        hs_pct: m.hs_pct,
        acs: m.acs,
        rr_change: rank.map(|r| r.last_change),
        tier: rank.map(|r| tier_fr(&r.tier)),
        tier_id: rank.map(|r| r.tier_id),
        rr: rank.map(|r| r.rr),
        next_tier: step.as_ref().map(|s| s.0.clone()),
        rr_to_next: step.map(|s| s.1),
        map_splash: None,
        agent_portrait: None,
        tier_icon: None,
    }
}

/// Pictures the card shows: map splash, agent portrait, rank icon (when ranked).
pub fn card_wants(c: &ValoMatchCard) -> Vec<Want> {
    let mut w = vec![Want::new(Kind::MapSplash, &c.map_id, &c.map), Want::new(Kind::AgentPortrait, &c.agent_id, &c.agent)];
    if let Some(t) = c.tier_id {
        w.push(Want::tier(t));
    }
    w
}

/// Puts the URLs `got` (in `card_wants` order) on the card.
pub fn apply_assets(c: &mut ValoMatchCard, got: &[Option<String>]) {
    let at = |i: usize| got.get(i).cloned().flatten();
    c.map_splash = at(0);
    c.agent_portrait = at(1);
    c.tier_icon = if c.tier_id.is_some() { at(2) } else { None };
}

/// Downloads the card's pictures (once, cached on disk). Blocking: watcher thread only.
pub fn with_assets(mut c: ValoMatchCard) -> ValoMatchCard {
    let got = assets::ensure_all(&card_wants(&c));
    apply_assets(&mut c, &got);
    c
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Start,
    MatchesResult(Result<Vec<MatchSummary>, ApiError>),
    MmrResult(Result<Mmr, ApiError>),
    GameExit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    FetchMmr(MatchSummary),
    EmitCard(ValoMatchCard),
    ScheduleExitPoll(Duration),
}

#[derive(Debug)]
pub struct WatchReducer {
    pub baseline_id: Option<String>,
    pub current_interval: Duration,
    pub pending_match: Option<MatchSummary>,
}

impl Default for WatchReducer {
    fn default() -> Self {
        Self {
            baseline_id: None,
            current_interval: DEFAULT_POLL,
            pending_match: None,
        }
    }
}

impl WatchReducer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn step(&mut self, action: Action) -> Option<Effect> {
        match action {
            Action::Start => {
                self.baseline_id = None;
                self.current_interval = DEFAULT_POLL;
                self.pending_match = None;
                None
            }
            Action::MatchesResult(res) => match res {
                Ok(matches) => {
                    self.current_interval = DEFAULT_POLL;
                    if let Some(first) = matches.first() {
                        if self.baseline_id.is_none() {
                            // Baseline: record first match seen, emit no card.
                            self.baseline_id = Some(first.id.clone());
                            None
                        } else if self.baseline_id.as_deref() != Some(&first.id) {
                            // New match!
                            self.baseline_id = Some(first.id.clone());
                            self.pending_match = Some(first.clone());
                            Some(Effect::FetchMmr(first.clone()))
                        } else {
                            // Same match id: no card
                            None
                        }
                    } else {
                        None
                    }
                }
                Err(ApiError::RateLimited(s)) => {
                    self.current_interval = Duration::from_secs((s as u64).max(DEFAULT_POLL.as_secs()));
                    None
                }
                Err(_) => {
                    self.current_interval = (self.current_interval * 2).min(MAX_BACKOFF);
                    None
                }
            },
            Action::MmrResult(mmr_res) => {
                if let Some(m) = self.pending_match.take() {
                    let mmr = mmr_res.ok();
                    Some(Effect::EmitCard(card_for(&m, mmr.as_ref())))
                } else {
                    None
                }
            }
            Action::GameExit => {
                self.current_interval = EXIT_POLL;
                Some(Effect::ScheduleExitPoll(EXIT_POLL))
            }
        }
    }
}

/// "Always shown" card: at startup and whenever the setting is switched on, the last
/// match is fetched once on a worker and sent to the docks (no peek, nothing to dismiss).
/// Afterwards new matches arrive through the normal post-match path.
pub fn init_pinned(app: &AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::Listener;
    static WAS_ON: AtomicBool = AtomicBool::new(false);
    fn wanted(s: &crate::settings::Settings) -> bool {
        s.valorant.post_match && s.valorant.pin_card
    }
    fn fetch(app: &AppHandle) {
        let app = app.clone();
        let spawned = std::thread::Builder::new().name("lucent-valo-pin".into()).spawn(move || {
            // The dock pages must be listening: give them time after startup.
            std::thread::sleep(std::time::Duration::from_secs(4));
            let s = app.state::<crate::settings::SettingsStore>().get();
            let Some(account) = api::Account::from_settings(&s.valorant) else { return };
            let Some(m) = api::fetch_matches(&account, 1).ok().and_then(|v| v.into_iter().next()) else { return };
            let mmr = api::fetch_mmr(&account).ok();
            if let Err(e) = app.emit("valo://match", with_assets(card_for(&m, mmr.as_ref()))) {
                eprintln!("lucent: cannot emit valo://match: {e}");
            }
        });
        if let Err(e) = spawned {
            eprintln!("lucent: cannot spawn valo pin thread: {e}");
        }
    }
    let on = wanted(&app.state::<crate::settings::SettingsStore>().get());
    WAS_ON.store(on, Ordering::SeqCst);
    if on {
        fetch(app);
    }
    let handle = app.clone();
    app.listen_any("settings://changed", move |event| {
        let Ok(s) = serde_json::from_str::<crate::settings::Settings>(event.payload()) else { return };
        let on = wanted(&s);
        if on && !WAS_ON.swap(on, Ordering::SeqCst) {
            fetch(&handle);
        } else {
            WAS_ON.store(on, Ordering::SeqCst);
        }
    });
}

/// Réglages › Valorant › « Tester »: shows the card of the last match played, so the
/// card can be seen without waiting for the next game. Blocking (network).
pub fn preview_last_match(app: &AppHandle) {
    let s = app.state::<crate::settings::SettingsStore>().get();
    let Some(account) = api::Account::from_settings(&s.valorant) else { return };
    let last = match api::fetch_matches(&account, 1) {
        Ok(m) => m.into_iter().next(),
        Err(e) => {
            eprintln!("lucent: valorant preview: {}", e.message());
            return;
        }
    };
    let Some(m) = last else { return };
    let mmr = api::fetch_mmr(&account).ok();
    show_or_queue_card(app, with_assets(card_for(&m, mmr.as_ref())));
}

static QUEUED_CARD: Mutex<Option<ValoMatchCard>> = Mutex::new(None);

/// Called when a card is ready. Peeks the dock on non-game monitors, or queues it if
/// the only monitor is currently running and focusing the game.
pub fn show_or_queue_card(app: &AppHandle, card: ValoMatchCard) {
    let settings = app.state::<crate::settings::SettingsStore>().get();
    let card_duration_ms = settings.animations.card_duration_ms as u64;

    #[cfg(windows)]
    {
        let all_monitors = crate::monitors::win::enumerate();
        let game_state = crate::game::service::state();
        let non_game: Vec<_> = all_monitors
            .iter()
            .filter(|m| !game_state.owns(&m.id))
            .collect();

        if non_game.is_empty() && game_state.active && game_state.focused {
            // Single monitor and game is focused: queue until unfocused or exit
            *QUEUED_CARD.lock().unwrap_or_else(|e| e.into_inner()) = Some(card);
            return;
        }

        // Emit and peek on non-game monitor(s), or the primary monitor if not focused
        for m in &non_game {
            crate::dock_host::peek(m.id.clone(), card_duration_ms);
        }
    }

    if let Err(e) = app.emit("valo://match", card) {
        eprintln!("lucent: cannot emit valo://match: {e}");
    }
}

/// Called on `game://state` changes to flush any queued card if the game is no longer
/// owning/focusing the monitor.
pub fn check_queued_card(app: &AppHandle) {
    let card = {
        let mut q = QUEUED_CARD.lock().unwrap_or_else(|e| e.into_inner());
        let game_state = crate::game::service::state();
        if !game_state.active || !game_state.focused {
            q.take()
        } else {
            None
        }
    };

    if let Some(card) = card {
        let settings = app.state::<crate::settings::SettingsStore>().get();
        let card_duration_ms = settings.animations.card_duration_ms as u64;
        #[cfg(windows)]
        {
            crate::dock_host::peek(String::new(), card_duration_ms);
        }
        if let Err(e) = app.emit("valo://match", card) {
            eprintln!("lucent: cannot emit valo://match: {e}");
        }
    }
}

#[cfg(windows)]
static RUNNING_SESSION: Mutex<bool> = Mutex::new(false);

/// Starts background watcher if Valorant session starts.
#[cfg(windows)]
pub fn on_game_state(app: &AppHandle, game_active: bool) {
    check_queued_card(app);

    let is_val = crate::game::service::active_exe()
        .map(|e| e.ends_with(VALORANT_EXE))
        .unwrap_or(false);

    let mut running = RUNNING_SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if game_active && is_val {
        if !*running {
            *running = true;
            let app_handle = app.clone();
            std::thread::Builder::new()
                .name("valo-watcher".into())
                .spawn(move || run_watcher_loop(app_handle))
                .ok();
        }
    }
}

#[cfg(windows)]
fn run_watcher_loop(app: AppHandle) {
    let mut reducer = WatchReducer::new();
    let _ = reducer.step(Action::Start);

    // Initial poll for baseline
    let s = app.state::<crate::settings::SettingsStore>().get();
    if !s.valorant.post_match {
        *RUNNING_SESSION.lock().unwrap_or_else(|e| e.into_inner()) = false;
        return;
    }
    let Some(account) = api::Account::from_settings(&s.valorant) else {
        *RUNNING_SESSION.lock().unwrap_or_else(|e| e.into_inner()) = false;
        return;
    };

    let first_matches = api::fetch_matches(&account, 1);
    let _ = reducer.step(Action::MatchesResult(first_matches));

    loop {
        std::thread::sleep(reducer.current_interval);

        let is_val_active = crate::game::service::is_active()
            && crate::game::service::active_exe()
                .map(|e| e.ends_with(VALORANT_EXE))
                .unwrap_or(false);

        let s = app.state::<crate::settings::SettingsStore>().get();
        if !s.valorant.post_match {
            break;
        }
        let Some(account) = api::Account::from_settings(&s.valorant) else {
            break;
        };

        if !is_val_active {
            // Game exited: perform one final exit poll
            let _ = reducer.step(Action::GameExit);
            std::thread::sleep(reducer.current_interval);
            let matches = api::fetch_matches(&account, 1);
            if let Some(Effect::FetchMmr(_m)) = reducer.step(Action::MatchesResult(matches)) {
                let mmr_res = api::fetch_mmr(&account);
                if let Some(Effect::EmitCard(card)) = reducer.step(Action::MmrResult(mmr_res)) {
                    show_or_queue_card(&app, with_assets(card));
                }
            }
            break;
        }

        let matches = api::fetch_matches(&account, 1);
        if let Some(Effect::FetchMmr(_m)) = reducer.step(Action::MatchesResult(matches)) {
            let mmr_res = api::fetch_mmr(&account);
            if let Some(Effect::EmitCard(card)) = reducer.step(Action::MmrResult(mmr_res)) {
                show_or_queue_card(&app, with_assets(card));
            }
        }
    }

    *RUNNING_SESSION.lock().unwrap_or_else(|e| e.into_inner()) = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_match(id: &str, won: Option<bool>) -> MatchSummary {
        MatchSummary {
            id: id.to_string(),
            map: "Ascent".to_string(),
            queue: "competitive".to_string(),
            won,
            score: (13, 10),
            kda: (20, 10, 5),
            agent: "Jett".to_string(),
            ..Default::default()
        }
    }

    fn dummy_mmr() -> Mmr {
        Mmr {
            tier: "Platinum 2".to_string(),
            tier_id: 17,
            rr: 45,
            last_change: 18,
            elo: 1245,
        }
    }

    #[test]
    fn baseline_no_card() {
        let mut r = WatchReducer::new();
        r.step(Action::Start);
        let eff = r.step(Action::MatchesResult(Ok(vec![dummy_match("m1", Some(true))])));
        assert_eq!(eff, None);
        assert_eq!(r.baseline_id.as_deref(), Some("m1"));
        assert_eq!(r.current_interval, DEFAULT_POLL);
    }

    #[test]
    fn same_id_no_card() {
        let mut r = WatchReducer::new();
        r.step(Action::Start);
        r.step(Action::MatchesResult(Ok(vec![dummy_match("m1", Some(true))])));
        let eff = r.step(Action::MatchesResult(Ok(vec![dummy_match("m1", Some(true))])));
        assert_eq!(eff, None);
        assert_eq!(r.baseline_id.as_deref(), Some("m1"));
    }

    #[test]
    fn new_id_emits_card() {
        let mut r = WatchReducer::new();
        r.step(Action::Start);
        r.step(Action::MatchesResult(Ok(vec![dummy_match("m1", Some(true))])));

        let m2 = dummy_match("m2", Some(true));
        let eff = r.step(Action::MatchesResult(Ok(vec![m2.clone()])));
        assert_eq!(eff, Some(Effect::FetchMmr(m2)));
        assert_eq!(r.baseline_id.as_deref(), Some("m2"));

        let card_eff = r.step(Action::MmrResult(Ok(dummy_mmr())));
        match card_eff {
            Some(Effect::EmitCard(card)) => {
                assert!(card.line.contains("Ascent"));
                assert_eq!(card.won, Some(true));
            }
            other => panic!("expected EmitCard, got {other:?}"),
        }
    }

    #[test]
    fn card_fields_and_assets() {
        let mut m = dummy_match("m", Some(true));
        m.map_id = "2c9d57ec-4431-9c5e-2939-8f9ef6dd5cba".into();
        m.hs_pct = Some(24);
        let c = card_for(&m, Some(&dummy_mmr()));
        assert_eq!((c.rr_change, c.tier.as_deref(), c.tier_id), (Some(18), Some("Platine 2"), Some(17)));
        assert_eq!((c.score, c.kda, c.hs_pct, c.competitive), ((13, 10), (20, 10, 5), Some(24), true));
        let wants = card_wants(&c);
        assert_eq!(wants.len(), 3);
        assert_eq!(wants[0], Want::new(Kind::MapSplash, "2c9d57ec-4431-9c5e-2939-8f9ef6dd5cba", "Ascent"));
        assert_eq!(wants[2], Want::tier(17));
        let mut c2 = c.clone();
        apply_assets(&mut c2, &[Some("http://valo.localhost/a.png".into()), None, Some("t".into())]);
        assert_eq!((c2.map_splash.as_deref(), c2.agent_portrait.as_deref(), c2.tier_icon.as_deref()), (Some("http://valo.localhost/a.png"), None, Some("t")));

        // Unrated: no RR, no rank icon wanted; no MMR at all: same.
        m.queue = "unrated".into();
        let u = card_for(&m, Some(&dummy_mmr()));
        assert_eq!((u.rr_change, u.tier.as_ref(), u.tier_id), (None, None, None));
        assert_eq!(card_wants(&u).len(), 2);
        let json = serde_json::to_value(&u).unwrap();
        assert_eq!(json["score"], serde_json::json!([13, 10]));
        assert_eq!(json["hsPct"], serde_json::json!(24));
        assert!(json["mapSplash"].is_null());
    }

    #[test]
    fn backoff_intervals() {
        let mut r = WatchReducer::new();
        r.step(Action::Start);
        assert_eq!(r.current_interval, DEFAULT_POLL);

        // Rate limited < 180s -> stays 180s
        r.step(Action::MatchesResult(Err(ApiError::RateLimited(60))));
        assert_eq!(r.current_interval, Duration::from_secs(180));

        // Rate limited > 180s -> waits that delay
        r.step(Action::MatchesResult(Err(ApiError::RateLimited(300))));
        assert_eq!(r.current_interval, Duration::from_secs(300));

        // Success resets interval
        r.step(Action::MatchesResult(Ok(vec![dummy_match("m1", Some(true))])));
        assert_eq!(r.current_interval, DEFAULT_POLL);

        // General error doubles interval up to 15 min
        r.step(Action::MatchesResult(Err(ApiError::Unreachable)));
        assert_eq!(r.current_interval, Duration::from_secs(360));
        r.step(Action::MatchesResult(Err(ApiError::Unreachable)));
        assert_eq!(r.current_interval, Duration::from_secs(720));
        r.step(Action::MatchesResult(Err(ApiError::Unreachable)));
        assert_eq!(r.current_interval, Duration::from_secs(900)); // capped at 15m
    }

    #[test]
    fn game_exit_schedules_30s_poll() {
        let mut r = WatchReducer::new();
        let eff = r.step(Action::GameExit);
        assert_eq!(eff, Some(Effect::ScheduleExitPoll(EXIT_POLL)));
        assert_eq!(r.current_interval, EXIT_POLL);
    }
}
