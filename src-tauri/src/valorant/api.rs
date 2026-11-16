//! HenrikDev HTTP calls (blocking: worker threads only, never the main thread).
//! The key travels in the `Authorization` header only and is never logged.
use super::{error_for_status, parse_matches, parse_mmr, parse_riot_id, ApiError, MatchSummary, Mmr};
use crate::settings::ValorantSettings;
use std::sync::LazyLock;
use std::time::Duration;

const BASE: &str = "https://api.henrikdev.xyz";
const TIMEOUT: Duration = Duration::from_secs(4);

static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| ureq::AgentBuilder::new().timeout(TIMEOUT).build());

/// The player and key of valid settings; None when not configured (no key or bad Riot ID).
pub struct Account {
    pub name: String,
    pub tag: String,
    pub region: String,
    key: String,
}

impl Account {
    pub fn from_settings(s: &ValorantSettings) -> Option<Self> {
        let key = s.api_key.trim();
        if key.is_empty() {
            return None;
        }
        let (name, tag) = parse_riot_id(&s.riot_id)?;
        Some(Self { name, tag, region: s.region.clone(), key: key.to_string() })
    }
}

/// Path-segment encoding (Riot IDs contain spaces and unicode).
pub fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn mmr_url(a: &Account) -> String {
    format!(
        "{BASE}/valorant/v3/mmr/{}/pc/{}/{}",
        encode_segment(&a.region),
        encode_segment(&a.name),
        encode_segment(&a.tag)
    )
}

pub fn matches_url(a: &Account, size: u32) -> String {
    format!(
        "{BASE}/valorant/v4/matches/{}/pc/{}/{}?size={size}",
        encode_segment(&a.region),
        encode_segment(&a.name),
        encode_segment(&a.tag)
    )
}

fn get(a: &Account, url: &str) -> Result<String, ApiError> {
    match AGENT.get(url).set("Authorization", &a.key).set("Accept", "application/json").call() {
        Ok(resp) => resp.into_string().map_err(|e| {
            eprintln!("lucent: henrikdev: cannot read body: {e}");
            ApiError::Unreachable
        }),
        Err(ureq::Error::Status(code, resp)) => {
            let err = error_for_status(code, resp.header("Retry-After"));
            eprintln!("lucent: henrikdev: HTTP {code}");
            Err(err)
        }
        Err(ureq::Error::Transport(t)) => {
            // The transport error never carries the headers (no key in it).
            eprintln!("lucent: henrikdev: {}", t.kind());
            Err(ApiError::Unreachable)
        }
    }
}

pub fn fetch_mmr(a: &Account) -> Result<Mmr, ApiError> {
    parse_mmr(&get(a, &mmr_url(a))?)
}

/// Newest first, at most `size` (1..=10).
pub fn fetch_matches(a: &Account, size: u32) -> Result<Vec<MatchSummary>, ApiError> {
    let body = get(a, &matches_url(a, size.clamp(1, 10)))?;
    parse_matches(&body, &a.name, &a.tag)
}

/// `valo_test`: one MMR call, French result line.
pub fn test_line(s: &ValorantSettings) -> String {
    let Some(a) = Account::from_settings(s) else {
        return "Renseigne ton Riot ID (Pseudo#TAG) et ta clé HenrikDev".into();
    };
    match fetch_mmr(&a) {
        Ok(m) => format!("OK : {}#{} · {} · {} RR", a.name, a.tag, super::tier_fr(&m.tier), m.rr),
        Err(e) => e.message(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(riot_id: &str, key: &str) -> ValorantSettings {
        ValorantSettings { riot_id: riot_id.into(), region: "eu".into(), api_key: key.into(), post_match: true, pin_card: false }
    }

    #[test]
    fn urls_encode_riot_id_and_never_hold_the_key() {
        let a = Account::from_settings(&settings("Père Noël#É 1", "SECRET-KEY")).unwrap();
        let u = mmr_url(&a);
        assert_eq!(u, "https://api.henrikdev.xyz/valorant/v3/mmr/eu/pc/P%C3%A8re%20No%C3%ABl/%C3%89%201");
        assert_eq!(
            matches_url(&a, 5),
            "https://api.henrikdev.xyz/valorant/v4/matches/eu/pc/P%C3%A8re%20No%C3%ABl/%C3%89%201?size=5"
        );
        assert!(!u.contains("SECRET"));
    }

    #[test]
    fn account_requires_key_and_valid_riot_id() {
        assert!(Account::from_settings(&settings("Sentinel#EUW", "")).is_none());
        assert!(Account::from_settings(&settings("Sentinel#EUW", "   ")).is_none());
        assert!(Account::from_settings(&settings("Sentinel", "k")).is_none());
        assert!(Account::from_settings(&settings("Sentinel#EUW", "k")).is_some());
        assert_eq!(test_line(&settings("", "")), "Renseigne ton Riot ID (Pseudo#TAG) et ta clé HenrikDev");
    }
}
