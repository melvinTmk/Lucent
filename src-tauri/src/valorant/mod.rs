//! HenrikDev Valorant API: pure parsing and formatting (no I/O here, see `api.rs`).
//! Used by the `valo` search provider and the post-match card.
pub mod api;
pub mod assets;
pub mod watch;

pub use watch::ValoMatchCard;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Structured data of a `valo` search row (the page draws Valorant-style tiles from it;
/// title/subtitle stay as the plain-text fallback). Picture URLs: `valo` scheme or None.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ValoRow {
    #[serde(rename_all = "camelCase")]
    Rank {
        /// French tier name ("Diamant 2").
        tier: String,
        rr: i32,
        last_change: i32,
        tier_icon: Option<String>,
        /// Next rank and the RR still missing (Iron 1 .. Ascendant 3 only).
        #[serde(default)]
        next_tier: Option<String>,
        #[serde(default)]
        rr_to_next: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    Match {
        won: Option<bool>,
        score: (u32, u32),
        map: String,
        agent: String,
        kda: (u32, u32, u32),
        hs_pct: Option<u32>,
        acs: Option<u32>,
        competitive: bool,
        map_image: Option<String>,
        agent_icon: Option<String>,
    },
}

/// What can go wrong talking to HenrikDev. Messages are French (`message`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// 401/403: the key is missing, wrong or revoked.
    Unauthorized,
    /// 404, or the player is not in the response.
    NotFound,
    /// 429: retry after this many seconds.
    RateLimited(u64),
    /// Network failure, timeout or 5xx.
    Unreachable,
    /// Body is not the expected shape.
    Invalid,
}

impl ApiError {
    pub fn message(&self) -> String {
        match self {
            ApiError::Unauthorized => "Clé HenrikDev refusée".into(),
            ApiError::NotFound => "Joueur introuvable".into(),
            ApiError::RateLimited(s) => format!("Trop de requêtes, réessaie dans {s} s"),
            ApiError::Unreachable => "API injoignable".into(),
            ApiError::Invalid => "Réponse inattendue de l'API HenrikDev".into(),
        }
    }
}

/// Seconds to wait when a 429 carries no usable `Retry-After`.
const DEFAULT_RETRY_SECS: u64 = 60;

/// Maps a non-2xx HTTP status (and its `Retry-After` header, in seconds) to an error.
pub fn error_for_status(status: u16, retry_after: Option<&str>) -> ApiError {
    match status {
        401 | 403 => ApiError::Unauthorized,
        404 => ApiError::NotFound,
        429 => ApiError::RateLimited(
            retry_after.and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(DEFAULT_RETRY_SECS).max(1),
        ),
        _ => ApiError::Unreachable,
    }
}

/// "Sentinel#EUW" → ("Sentinel", "EUW"). Trims around the id and each part; rejects a missing
/// `#` or an empty part. The tag is what follows the last `#`.
pub fn parse_riot_id(s: &str) -> Option<(String, String)> {
    let (name, tag) = s.trim().rsplit_once('#')?;
    let (name, tag) = (name.trim(), tag.trim());
    (!name.is_empty() && !tag.is_empty()).then(|| (name.to_string(), tag.to_string()))
}

/// Highest tier id with a 100 RR step to the next one: Ascendant 3 (→ Immortal 1).
/// Immortal RR is uncapped and Radiant is the top, so they have no "next" step.
const LAST_STEPPED_TIER: u32 = 23;

/// French name of tier `id` (3 Iron 1 .. 26 Immortal 3, 27 Radiant).
pub fn tier_name_fr(id: u32) -> Option<String> {
    const GROUPS: [&str; 8] = ["Iron", "Bronze", "Silver", "Gold", "Platinum", "Diamond", "Ascendant", "Immortal"];
    match id {
        27 => Some(tier_fr("Radiant")),
        3..=26 => {
            let i = (id - 3) as usize;
            Some(tier_fr(&format!("{} {}", GROUPS[i / 3], i % 3 + 1)))
        }
        _ => None,
    }
}

/// The next rank and the RR missing to reach it (`rr` is the RR inside the current tier).
pub fn next_step(tier_id: u32, rr: i32) -> Option<(String, u32)> {
    if !(3..=LAST_STEPPED_TIER).contains(&tier_id) {
        return None;
    }
    let missing = (100 - rr.clamp(0, 99)) as u32;
    Some((tier_name_fr(tier_id + 1)?, missing))
}

/// French rank name: "Platinum 2" → "Platine 2", "Unrated" → "Non classé". Unknown names
/// are returned unchanged.
pub fn tier_fr(en: &str) -> String {
    let en = en.trim();
    if en.eq_ignore_ascii_case("unrated") || en.is_empty() {
        return "Non classé".into();
    }
    let (word, rest) = match en.split_once(' ') {
        Some((w, r)) => (w, Some(r)),
        None => (en, None),
    };
    let fr = match word.to_ascii_lowercase().as_str() {
        "iron" => "Fer",
        "bronze" => "Bronze",
        "silver" => "Argent",
        "gold" => "Or",
        "platinum" => "Platine",
        "diamond" => "Diamant",
        "ascendant" => "Ascendant",
        "immortal" => "Immortel",
        "radiant" => "Radiant",
        _ => return en.to_string(),
    };
    match rest {
        Some(r) => format!("{fr} {r}"),
        None => fr.to_string(),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mmr {
    /// English tier name as the API gives it ("Diamond 2"); see `tier_fr`.
    pub tier: String,
    /// `tier.id` (0 unrated .. 27 radiant): the rank icon.
    pub tier_id: u32,
    pub rr: i32,
    pub last_change: i32,
    pub elo: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchSummary {
    pub id: String,
    pub map: String,
    /// `metadata.map.id` (uuid, may be empty): the map pictures.
    pub map_id: String,
    /// `metadata.queue.id` ("competitive", "unrated", ...): decides whether RR applies.
    pub queue: String,
    /// None for a draw.
    pub won: Option<bool>,
    /// Rounds, the player's team first.
    pub score: (u32, u32),
    pub kda: (u32, u32, u32),
    pub agent: String,
    /// `agent.id` (uuid, may be empty): the agent pictures.
    pub agent_id: String,
    /// Headshot share of the hits, percent; None without shot stats.
    pub hs_pct: Option<u32>,
    /// Average combat score (`stats.score` / rounds played); None without a score.
    pub acs: Option<u32>,
}

impl MatchSummary {
    pub fn is_competitive(&self) -> bool {
        self.queue.eq_ignore_ascii_case("competitive")
    }
}

fn i32_at(v: &Value, ptr: &str) -> Option<i32> {
    v.pointer(ptr)?.as_i64().and_then(|n| i32::try_from(n).ok())
}

fn u32_at(v: &Value, ptr: &str) -> u32 {
    v.pointer(ptr).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0)
}

fn str_at<'a>(v: &'a Value, ptr: &str) -> &'a str {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or("")
}

/// `GET /valorant/v3/mmr/...` body → current rank.
pub fn parse_mmr(json: &str) -> Result<Mmr, ApiError> {
    let v: Value = serde_json::from_str(json).map_err(|_| ApiError::Invalid)?;
    let cur = v.pointer("/data/current").ok_or(ApiError::Invalid)?;
    let tier = cur.pointer("/tier/name").and_then(Value::as_str).unwrap_or("Unrated").to_string();
    Ok(Mmr {
        tier,
        tier_id: cur.pointer("/tier/id").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
        rr: i32_at(cur, "/rr").unwrap_or(0),
        last_change: i32_at(cur, "/last_change").unwrap_or(0),
        elo: i32_at(cur, "/elo").unwrap_or(0),
    })
}

/// One v4 match for the player `name#tag` (case-insensitive), None when absent.
fn summarize(m: &Value, name: &str, tag: &str) -> Option<MatchSummary> {
    let players = m.get("players")?.as_array()?;
    let me = players.iter().find(|p| {
        str_at(p, "/name").eq_ignore_ascii_case(name) && str_at(p, "/tag").eq_ignore_ascii_case(tag)
    })?;
    let team_id = str_at(me, "/team_id");
    let team = m.get("teams")?.as_array()?.iter().find(|t| str_at(t, "/team_id").eq_ignore_ascii_case(team_id));
    let (score, won) = match team {
        Some(t) => {
            let score = (u32_at(t, "/rounds/won"), u32_at(t, "/rounds/lost"));
            let won = t.get("won").and_then(Value::as_bool).unwrap_or(false);
            (score, if score.0 == score.1 && !won { None } else { Some(won) })
        }
        None => ((0, 0), None),
    };
    // Older payloads carry the queue as a plain string.
    let queue = match m.pointer("/metadata/queue") {
        Some(Value::String(s)) => s.clone(),
        Some(q) => str_at(q, "/id").to_string(),
        None => String::new(),
    };
    let shots = [u32_at(me, "/stats/headshots"), u32_at(me, "/stats/bodyshots"), u32_at(me, "/stats/legshots")];
    let hits: u32 = shots.iter().sum();
    let hs_pct = (hits > 0).then(|| (shots[0] * 100 + hits / 2) / hits);
    let rounds = score.0 + score.1;
    let total = u32_at(me, "/stats/score");
    let acs = (rounds > 0 && total > 0).then(|| (total + rounds / 2) / rounds);
    Some(MatchSummary {
        id: str_at(m, "/metadata/match_id").to_string(),
        map: str_at(m, "/metadata/map/name").to_string(),
        map_id: str_at(m, "/metadata/map/id").to_string(),
        queue,
        won,
        score,
        kda: (u32_at(me, "/stats/kills"), u32_at(me, "/stats/deaths"), u32_at(me, "/stats/assists")),
        agent: str_at(me, "/agent/name").to_string(),
        agent_id: str_at(me, "/agent/id").to_string(),
        hs_pct,
        acs,
    })
}

/// `GET /valorant/v4/matches/...` body → newest first, as the API orders them. A match
/// without the player is skipped; when no match has them at all → `NotFound`.
pub fn parse_matches(json: &str, name: &str, tag: &str) -> Result<Vec<MatchSummary>, ApiError> {
    let v: Value = serde_json::from_str(json).map_err(|_| ApiError::Invalid)?;
    let list = v.get("data").and_then(Value::as_array).ok_or(ApiError::Invalid)?;
    let out: Vec<MatchSummary> = list.iter().filter_map(|m| summarize(m, name, tag)).collect();
    if out.is_empty() && !list.is_empty() {
        return Err(ApiError::NotFound);
    }
    Ok(out)
}

/// "VICTOIRE 13-9 · Ascent · 24/15/6 · +18 RR → Platine 2". The RR part only with an MMR
/// and a competitive match.
pub fn summary_line(m: &MatchSummary, mmr: Option<&Mmr>) -> String {
    let result = match m.won {
        Some(true) => "VICTOIRE",
        Some(false) => "DÉFAITE",
        None => "ÉGALITÉ",
    };
    let mut line = format!(
        "{result} {}-{} · {} · {}/{}/{}",
        m.score.0, m.score.1, m.map, m.kda.0, m.kda.1, m.kda.2
    );
    if let Some(mmr) = mmr.filter(|_| m.is_competitive()) {
        line.push_str(&format!(" · {:+} RR → {}", mmr.last_change, tier_fr(&mmr.tier)));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    const MMR: &str = include_str!("fixtures/mmr-v3.json");
    const MATCHES: &str = include_str!("fixtures/matches-v4.json");

    fn summary(won: Option<bool>, queue: &str) -> MatchSummary {
        MatchSummary {
            id: "x".into(),
            map: "Ascent".into(),
            queue: queue.into(),
            won,
            score: (13, 9),
            kda: (24, 15, 6),
            agent: "Jett".into(),
            ..Default::default()
        }
    }

    fn mmr(tier: &str, change: i32) -> Mmr {
        Mmr { tier: tier.into(), tier_id: 0, rr: 47, last_change: change, elo: 1547 }
    }

    #[test]
    fn parse_riot_id_cases() {
        assert_eq!(parse_riot_id("Sentinel#EUW"), Some(("Sentinel".into(), "EUW".into())));
        assert_eq!(parse_riot_id("  Mon Pseudo # EUW  "), Some(("Mon Pseudo".into(), "EUW".into())));
        assert_eq!(parse_riot_id("Père Noël#ÉTÉ"), Some(("Père Noël".into(), "ÉTÉ".into())));
        for bad in ["", "Sentinel", "#EUW", "Sentinel#", "  #  ", "Sentinel EUW"] {
            assert_eq!(parse_riot_id(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn tier_fr_table() {
        let table = [
            ("Iron 1", "Fer 1"),
            ("Bronze 3", "Bronze 3"),
            ("Silver 2", "Argent 2"),
            ("Gold 1", "Or 1"),
            ("Platinum 2", "Platine 2"),
            ("Diamond 2", "Diamant 2"),
            ("Ascendant 3", "Ascendant 3"),
            ("Immortal 1", "Immortel 1"),
            ("Radiant", "Radiant"),
            ("Unrated", "Non classé"),
            ("Unknown thing", "Unknown thing"),
        ];
        for (en, fr) in table {
            assert_eq!(tier_fr(en), fr, "{en}");
        }
    }

    #[test]
    fn next_rank_steps() {
        assert_eq!(tier_name_fr(3).as_deref(), Some("Fer 1"));
        assert_eq!(tier_name_fr(19).as_deref(), Some("Diamant 2"));
        assert_eq!(tier_name_fr(24).as_deref(), Some("Immortel 1"));
        assert_eq!(tier_name_fr(27).as_deref(), Some("Radiant"));
        assert_eq!(tier_name_fr(0), None);
        assert_eq!(next_step(22, 80), Some(("Ascendant 3".into(), 20)));
        assert_eq!(next_step(23, 0), Some(("Immortel 1".into(), 100)));
        assert_eq!(next_step(19, 76), Some(("Diamant 3".into(), 24)));
        assert_eq!(next_step(19, 120), Some(("Diamant 3".into(), 1)));
        assert_eq!(next_step(24, 150), None);
        assert_eq!(next_step(0, 0), None);
    }

    #[test]
    fn parse_mmr_real_fixture() {
        assert_eq!(parse_mmr(MMR), Ok(Mmr { tier: "Diamond 2".into(), tier_id: 19, rr: 76, last_change: 16, elo: 1676 }));
        assert_eq!(parse_mmr("not json"), Err(ApiError::Invalid));
        assert_eq!(parse_mmr(r#"{"status":200,"data":{}}"#), Err(ApiError::Invalid));
    }

    #[test]
    fn parse_matches_real_fixture_win_and_loss_on_blue() {
        let ms = parse_matches(MATCHES, "sentinel", "EUW").unwrap();
        assert_eq!(ms.len(), 2);
        assert_eq!(
            ms[0],
            MatchSummary {
                id: "4cb409fe-1ecb-4673-a6c3-77e2cc77f41f".into(),
                map: "Bind".into(),
                queue: "competitive".into(),
                won: Some(true),
                score: (13, 10),
                kda: (15, 16, 9),
                agent: "Fade".into(),
                ..Default::default()
            }
        );
        assert_eq!(ms[1].map, "Sunset");
        assert_eq!(ms[1].won, Some(false));
        assert_eq!(ms[1].score, (6, 13));
        assert_eq!(ms[1].kda, (12, 16, 5));
        assert!(ms[1].is_competitive());
    }

    const RED_DRAW: &str = r#"{"status":200,"data":[
      {"metadata":{"match_id":"m1","map":{"name":"Lotus"},"queue":{"id":"unrated","name":"Unrated"}},
       "teams":[{"team_id":"Red","rounds":{"won":13,"lost":7},"won":true},
                {"team_id":"Blue","rounds":{"won":7,"lost":13},"won":false}],
       "players":[{"name":"Mon Pseudo","tag":"EUW","team_id":"Red","agent":{"name":"Sage"},
                   "stats":{"kills":10,"deaths":5,"assists":3}}]},
      {"metadata":{"match_id":"m2","map":{"name":"Haven"},"queue":{"id":"premier","name":"Premier"}},
       "teams":[{"team_id":"Red","rounds":{"won":12,"lost":12},"won":false},
                {"team_id":"Blue","rounds":{"won":12,"lost":12},"won":false}],
       "players":[{"name":"MON PSEUDO","tag":"euw","team_id":"Red","agent":{"name":"Sova"},
                   "stats":{"kills":20,"deaths":20,"assists":4}}]}
    ]}"#;

    #[test]
    fn parse_matches_red_team_and_draw() {
        let ms = parse_matches(RED_DRAW, "Mon Pseudo", "EUW").unwrap();
        assert_eq!(ms[0].won, Some(true));
        assert_eq!(ms[0].score, (13, 7));
        assert_eq!(ms[0].queue, "unrated");
        assert!(!ms[0].is_competitive());
        assert_eq!(ms[1].won, None);
        assert_eq!(ms[1].score, (12, 12));
        assert_eq!(ms[1].agent, "Sova");
    }

    #[test]
    fn parse_matches_unknown_player_is_not_found() {
        assert_eq!(parse_matches(MATCHES, "Someone", "0000"), Err(ApiError::NotFound));
        assert_eq!(parse_matches(MATCHES, "Sentinel", "EUX"), Err(ApiError::NotFound));
        assert_eq!(parse_matches(r#"{"status":200,"data":[]}"#, "a", "b"), Ok(vec![]));
        assert_eq!(parse_matches("{}", "a", "b"), Err(ApiError::Invalid));
    }

    #[test]
    fn summary_line_exact_strings() {
        let m = mmr("Platinum 2", 18);
        assert_eq!(
            summary_line(&summary(Some(true), "competitive"), Some(&m)),
            "VICTOIRE 13-9 · Ascent · 24/15/6 · +18 RR → Platine 2"
        );
        assert_eq!(
            summary_line(&summary(Some(false), "competitive"), Some(&mmr("Platinum 2", -15))),
            "DÉFAITE 13-9 · Ascent · 24/15/6 · -15 RR → Platine 2"
        );
        assert_eq!(summary_line(&summary(None, "competitive"), None), "ÉGALITÉ 13-9 · Ascent · 24/15/6");
        assert_eq!(summary_line(&summary(Some(true), "unrated"), Some(&m)), "VICTOIRE 13-9 · Ascent · 24/15/6");
        let real = parse_matches(MATCHES, "Sentinel", "EUW").unwrap();
        let real_mmr = parse_mmr(MMR).unwrap();
        assert_eq!(summary_line(&real[0], Some(&real_mmr)), "VICTOIRE 13-10 · Bind · 15/16/9 · +16 RR → Diamant 2");
        assert_eq!(summary_line(&real[1], None), "DÉFAITE 6-13 · Sunset · 12/16/5");
    }

    #[test]
    fn parse_matches_ids_headshots_and_acs() {
        let json = r#"{"status":200,"data":[{"metadata":{"match_id":"m","map":{"id":"2c9d57ec-4431-9c5e-2939-8f9ef6dd5cba","name":"Bind"},"queue":{"id":"competitive"}},
          "teams":[{"team_id":"Blue","rounds":{"won":13,"lost":10},"won":true}],
          "players":[{"name":"a","tag":"b","team_id":"Blue","agent":{"id":"dade69b4-4f5a-8528-247b-219e5a1facd6","name":"Fade"},
                      "stats":{"score":5313,"kills":15,"deaths":16,"assists":9,"headshots":12,"bodyshots":36,"legshots":2}}]}]}"#;
        let m = &parse_matches(json, "A", "B").unwrap()[0];
        assert_eq!(m.map_id, "2c9d57ec-4431-9c5e-2939-8f9ef6dd5cba");
        assert_eq!(m.agent_id, "dade69b4-4f5a-8528-247b-219e5a1facd6");
        assert_eq!(m.hs_pct, Some(24));
        assert_eq!(m.acs, Some(231));
        // Trimmed fixture: no shot stats, no score.
        let real = parse_matches(MATCHES, "Sentinel", "EUW").unwrap();
        assert_eq!((real[0].hs_pct, real[0].acs, real[0].map_id.as_str()), (None, None, ""));
    }

    #[test]
    fn status_429_reads_retry_after() {
        assert_eq!(error_for_status(429, Some("17")), ApiError::RateLimited(17));
        assert_eq!(error_for_status(429, Some(" 5 ")), ApiError::RateLimited(5));
        assert_eq!(error_for_status(429, None), ApiError::RateLimited(60));
        assert_eq!(error_for_status(429, Some("Wed, 21 Oct 2015")), ApiError::RateLimited(60));
        assert_eq!(error_for_status(401, None), ApiError::Unauthorized);
        assert_eq!(error_for_status(403, None), ApiError::Unauthorized);
        assert_eq!(error_for_status(404, None), ApiError::NotFound);
        assert_eq!(error_for_status(503, None), ApiError::Unreachable);
        assert_eq!(ApiError::RateLimited(17).message(), "Trop de requêtes, réessaie dans 17 s");
    }
}
