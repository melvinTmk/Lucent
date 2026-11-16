//! Launch history for frecency: per entry id, a use count and the last-use time.
//! Stored in `%APPDATA%\Lucent\search-history.json`.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Recency half-life: a use loses half of its recency bonus every 14 days.
const HALF_LIFE_SECS: f64 = 14.0 * 24.0 * 3600.0;
/// Frecency weight per `ln(1 + count)`.
const COUNT_WEIGHT: f64 = 0.15;
/// Oldest entries are dropped beyond this many ids.
const MAX_ENTRIES: usize = 2000;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
struct Stat {
    count: u32,
    /// Unix seconds.
    last: u64,
}

#[derive(Debug, Default)]
pub struct History {
    path: Option<PathBuf>,
    map: HashMap<String, Stat>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn history_path() -> PathBuf {
    crate::paths::config_dir().join("search-history.json")
}

impl History {
    /// In-memory only (tests, and the fallback when nothing can be read).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Missing or corrupt file: start empty (a bad history must never break search).
    pub fn load(path: &Path) -> Self {
        let map = fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { path: Some(path.to_path_buf()), map }
    }

    pub fn bump(&mut self, id: &str) {
        self.bump_at(id, now_secs());
    }

    pub fn bump_at(&mut self, id: &str, now: u64) {
        let s = self.map.entry(id.to_string()).or_insert(Stat { count: 0, last: now });
        s.count = s.count.saturating_add(1);
        s.last = now;
        if self.map.len() > MAX_ENTRIES {
            if let Some(oldest) = self.map.iter().min_by_key(|(_, s)| s.last).map(|(k, _)| k.clone()) {
                self.map.remove(&oldest);
            }
        }
    }

    pub fn count(&self, id: &str) -> u32 {
        self.map.get(id).map_or(0, |s| s.count)
    }

    /// Frecency multiplier: exactly 1.0 for unknown ids, always `>= 1.0` for used ones
    /// (age only shrinks the bonus, it never turns it into a penalty).
    pub fn boost(&self, id: &str) -> f32 {
        self.boost_at(id, now_secs())
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Current unix time, so a ranking pass reads the clock once.
    pub fn now() -> u64 {
        now_secs()
    }

    pub fn boost_at(&self, id: &str, now: u64) -> f32 {
        let Some(s) = self.map.get(id) else { return 1.0 };
        let age = now.saturating_sub(s.last) as f64;
        let recency = 0.5f64.powf(age / HALF_LIFE_SECS);
        (1.0 + COUNT_WEIGHT * (1.0 + s.count as f64).ln() * recency) as f32
    }

    /// Ids that have been used, most frecent first (ties: most recent first).
    pub fn top(&self, n: usize) -> Vec<String> {
        let now = now_secs();
        let mut v: Vec<(&String, f32, u64)> =
            self.map.iter().map(|(k, s)| (k, self.boost_at(k, now), s.last)).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(b.0)));
        v.into_iter().take(n).map(|(k, _, _)| k.clone()).collect()
    }

    /// Atomic write (temp file + rename). No-op for an in-memory history.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(&self.map).map_err(std::io::Error::other)?)?;
        fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boost_grows_with_count_and_decays_with_age() {
        let mut h = History::empty();
        assert_eq!(h.boost_at("a", 1000), 1.0);
        h.bump_at("a", 1000);
        let one = h.boost_at("a", 1000);
        h.bump_at("a", 1000);
        assert!(h.boost_at("a", 1000) > one && one > 1.0);
        assert!(h.boost_at("a", 1000 + 60 * 24 * 3600) < h.boost_at("a", 1000));
    }

    #[test]
    fn old_use_never_ranks_below_never_used() {
        let mut h = History::empty();
        h.bump_at("a", 0);
        let year = 365 * 24 * 3600;
        assert!(h.boost_at("a", 60 * 24 * 3600) >= 1.0);
        assert!(h.boost_at("a", year) >= 1.0);
        assert!(h.boost_at("a", 60 * 24 * 3600) > h.boost_at("never", 0));
    }

    #[test]
    fn persists_and_survives_corruption() {
        let dir = std::env::temp_dir().join(format!("gd-hist-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("h.json");
        let mut h = History::load(&p);
        h.bump("x");
        h.bump("x");
        h.save().unwrap();
        assert_eq!(History::load(&p).count("x"), 2);
        fs::write(&p, "{not json").unwrap();
        assert_eq!(History::load(&p).count("x"), 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn top_orders_by_frecency() {
        let mut h = History::empty();
        h.bump("a");
        h.bump("b");
        h.bump("b");
        assert_eq!(h.top(1), vec!["b".to_string()]);
        assert_eq!(h.top(5).len(), 2);
    }
}
