//! Valorant pictures (agent portraits, map splashes, rank icons) from valorant-api.com, a
//! fan API serving Riot's assets without a key. Each picture is downloaded once, on a worker
//! thread, into `%LOCALAPPDATA%\Lucent\valo-assets\{kind}-{key}.png` and served to the
//! pages by the `valo` scheme (`http://valo.localhost/{file}`), which only ever reads files
//! of that folder whose name matches the whitelist (`is_allowed_name`): no path from a page
//! reaches the file system. Offline or failed: no URL, and the UI shows its text layout.
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

const MEDIA: &str = "https://media.valorant-api.com/";
const API: &str = "https://valorant-api.com/v1";
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BYTES: u64 = 8 * 1024 * 1024;
/// A failed catalog fetch is retried after this long (not on every search keystroke).
const CATALOG_RETRY: Duration = Duration::from_secs(10 * 60);
/// Latest competitive tier set when `/v1/competitivetiers` cannot be read.
const FALLBACK_TIER_SET: &str = "03621f52-342b-cf4e-4f86-9350a49c6d04";
pub const SCHEME: &str = "valo";
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Agent full-body cut-out (the post-match card).
    AgentPortrait,
    /// Agent square head icon (search tiles).
    AgentIcon,
    /// Map loading screen 16:9 (card background).
    MapSplash,
    /// Map wide strip (search tiles).
    MapList,
    /// Rank emblem, keyed by tier number.
    Tier,
}

const KINDS: [Kind; 5] = [Kind::AgentPortrait, Kind::AgentIcon, Kind::MapSplash, Kind::MapList, Kind::Tier];

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Kind::AgentPortrait => "agent-portrait",
            Kind::AgentIcon => "agent-icon",
            Kind::MapSplash => "map-splash",
            Kind::MapList => "map-list",
            Kind::Tier => "tier",
        }
    }
}

/// One picture to show: by uuid (or tier number) when the match gives it, else by name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Want {
    pub kind: Kind,
    pub id: String,
    pub name: String,
}

impl Want {
    pub fn new(kind: Kind, id: &str, name: &str) -> Self {
        Self { kind, id: id.trim().to_string(), name: name.trim().to_string() }
    }
    pub fn tier(id: u32) -> Self {
        Self { kind: Kind::Tier, id: id.to_string(), name: String::new() }
    }
}

// ---------- pure mapping ----------

/// Lowercase canonical uuid (8-4-4-4-12 hex), None for anything else.
pub fn normalize_uuid(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    let ok = s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        });
    ok.then_some(s)
}

/// Tier numbers: 0 (unrated) ..= 99, decimal without leading zeros.
fn valid_tier(s: &str) -> bool {
    !s.is_empty() && s.len() <= 2 && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
}

fn valid_key(kind: Kind, key: &str) -> bool {
    match kind {
        Kind::Tier => valid_tier(key),
        _ => normalize_uuid(key).as_deref() == Some(key),
    }
}

/// Cache file name of a picture, None when `key` is not a canonical uuid / tier number.
pub fn file_name(kind: Kind, key: &str) -> Option<String> {
    valid_key(kind, key).then(|| format!("{}-{key}.png", kind.prefix()))
}

/// The `valo` scheme serves only names `file_name` can produce: no separator, no `..`.
pub fn is_allowed_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".png") else { return false };
    KINDS.iter().any(|&k| {
        stem.strip_prefix(k.prefix())
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|key| valid_key(k, key))
    })
}

/// What a page puts in `src` for a cached file.
pub fn url_for(name: &str) -> String {
    format!("http://{SCHEME}.localhost/{name}")
}

/// Download address of a picture. Agents and maps follow media.valorant-api.com's fixed
/// layout; rank icons come from the tier table (`tiers`), else the known latest set.
pub fn source_url(kind: Kind, key: &str, tiers: Option<&HashMap<u32, String>>) -> Option<String> {
    if !valid_key(kind, key) {
        return None;
    }
    let url = match kind {
        Kind::AgentPortrait => format!("{MEDIA}agents/{key}/fullportrait.png"),
        Kind::AgentIcon => format!("{MEDIA}agents/{key}/displayicon.png"),
        Kind::MapSplash => format!("{MEDIA}maps/{key}/splash.png"),
        Kind::MapList => format!("{MEDIA}maps/{key}/listviewicon.png"),
        Kind::Tier => {
            let n: u32 = key.parse().ok()?;
            match tiers.and_then(|t| t.get(&n)) {
                Some(u) => u.clone(),
                None => format!("{MEDIA}competitivetiers/{FALLBACK_TIER_SET}/{n}/largeicon.png"),
            }
        }
    };
    // Only ever fetch from the media host (the tier table comes from the network).
    url.starts_with(MEDIA).then_some(url)
}

/// `/v1/maps` or `/v1/agents` body → lowercase displayName → uuid.
pub fn parse_name_index(json: &str) -> HashMap<String, String> {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return HashMap::new() };
    let Some(list) = v.get("data").and_then(Value::as_array) else { return HashMap::new() };
    list.iter()
        .filter_map(|e| {
            let name = e.get("displayName")?.as_str()?.trim().to_lowercase();
            let uuid = normalize_uuid(e.get("uuid")?.as_str()?)?;
            (!name.is_empty()).then_some((name, uuid))
        })
        .collect()
}

/// `/v1/competitivetiers` body → tier number → `largeIcon` of the latest set (the last).
pub fn parse_tier_icons(json: &str) -> HashMap<u32, String> {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return HashMap::new() };
    let Some(set) = v.get("data").and_then(Value::as_array).and_then(|l| l.last()) else { return HashMap::new() };
    let Some(tiers) = set.get("tiers").and_then(Value::as_array) else { return HashMap::new() };
    tiers
        .iter()
        .filter_map(|t| {
            let n = u32::try_from(t.get("tier")?.as_u64()?).ok()?;
            Some((n, t.get("largeIcon")?.as_str()?.to_string()))
        })
        .collect()
}

/// The cache key of `w`: its own uuid / tier number, else its name looked up in `index`.
pub fn resolve_key(w: &Want, index: Option<&HashMap<String, String>>) -> Option<String> {
    if w.kind == Kind::Tier {
        return valid_tier(&w.id).then(|| w.id.clone());
    }
    normalize_uuid(&w.id).or_else(|| index?.get(&w.name.to_lowercase()).cloned())
}

fn is_agent(k: Kind) -> bool {
    matches!(k, Kind::AgentPortrait | Kind::AgentIcon)
}

fn is_map(k: Kind) -> bool {
    matches!(k, Kind::MapSplash | Kind::MapList)
}

// ---------- runtime: cache folder, catalog, downloads ----------

fn dir() -> PathBuf {
    crate::paths::cache_dir().join("valo-assets")
}

static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| ureq::AgentBuilder::new().timeout(TIMEOUT).build());

/// GET with the size cap; None on any failure (logged without the body).
fn get_bytes(url: &str) -> Option<Vec<u8>> {
    let resp = match AGENT.get(url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => {
            eprintln!("lucent: valorant-api: HTTP {code}");
            return None;
        }
        Err(ureq::Error::Transport(t)) => {
            eprintln!("lucent: valorant-api: {}", t.kind());
            return None;
        }
    };
    if resp.header("Content-Length").and_then(|l| l.parse::<u64>().ok()).is_some_and(|l| l > MAX_BYTES) {
        return None;
    }
    let mut buf = Vec::new();
    if let Err(e) = resp.into_reader().take(MAX_BYTES + 1).read_to_end(&mut buf) {
        eprintln!("lucent: valorant-api: cannot read body: {e}");
        return None;
    }
    (buf.len() as u64 <= MAX_BYTES).then_some(buf)
}

/// One lazily fetched lookup table; a failure is retried after `CATALOG_RETRY`.
struct Slot<T> {
    value: Option<Arc<T>>,
    failed_at: Option<Instant>,
}

impl<T> Slot<T> {
    const fn new() -> Self {
        Self { value: None, failed_at: None }
    }
}

static MAPS: Mutex<Slot<HashMap<String, String>>> = Mutex::new(Slot::new());
static AGENTS: Mutex<Slot<HashMap<String, String>>> = Mutex::new(Slot::new());
static TIERS: Mutex<Slot<HashMap<u32, String>>> = Mutex::new(Slot::new());

/// The table of `slot` if loaded; with `fetch`, loads it first (network, worker threads only).
fn table<T>(slot: &Mutex<Slot<T>>, fetch: bool, url: &str, parse: fn(&str) -> T, empty: fn(&T) -> bool) -> Option<Arc<T>> {
    {
        let s = slot.lock().unwrap_or_else(|e| e.into_inner());
        if s.value.is_some() || !fetch || s.failed_at.is_some_and(|t| t.elapsed() < CATALOG_RETRY) {
            return s.value.clone();
        }
    }
    let parsed = get_bytes(url).and_then(|b| String::from_utf8(b).ok()).map(|j| parse(&j)).filter(|t| !empty(t));
    let mut s = slot.lock().unwrap_or_else(|e| e.into_inner());
    match parsed {
        Some(t) => s.value = Some(Arc::new(t)),
        None => s.failed_at = Some(Instant::now()),
    }
    s.value.clone()
}

fn name_index(kind: Kind, fetch: bool) -> Option<Arc<HashMap<String, String>>> {
    if is_agent(kind) {
        table(&AGENTS, fetch, &format!("{API}/agents?isPlayableCharacter=true"), parse_name_index, HashMap::is_empty)
    } else if is_map(kind) {
        table(&MAPS, fetch, &format!("{API}/maps"), parse_name_index, HashMap::is_empty)
    } else {
        None
    }
}

fn existing(dir: &Path, w: &Want, fetch: bool) -> (Option<String>, bool) {
    let index = if w.kind != Kind::Tier && normalize_uuid(&w.id).is_none() { name_index(w.kind, fetch) } else { None };
    let Some(key) = resolve_key(w, index.as_deref()) else { return (None, false) };
    let Some(name) = file_name(w.kind, &key) else { return (None, false) };
    let present = dir.join(&name).is_file();
    (Some(name), present)
}

/// URL of `w` if already on disk. No network: safe from any thread that may touch files.
pub fn cached(w: &Want) -> Option<String> {
    match existing(&dir(), w, false) {
        (Some(name), true) => Some(url_for(&name)),
        _ => None,
    }
}

/// URL of `w`, downloading it first when missing. Blocking: worker threads only.
pub fn ensure(w: &Want) -> Option<String> {
    let dir = dir();
    let (name, present) = existing(&dir, w, true);
    let name = name?;
    if present {
        return Some(url_for(&name));
    }
    let key = name.strip_suffix(".png")?.strip_prefix(w.kind.prefix())?.strip_prefix('-')?;
    let tiers = if w.kind == Kind::Tier {
        table(&TIERS, true, &format!("{API}/competitivetiers"), parse_tier_icons, HashMap::is_empty)
    } else {
        None
    };
    let url = source_url(w.kind, key, tiers.as_deref())?;
    let bytes = get_bytes(&url).filter(|b| b.starts_with(PNG_SIGNATURE))?;
    let bytes = shrink(w.kind, bytes);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("lucent: cannot create valo-assets: {e}");
        return None;
    }
    // Write then rename: a page never reads a half-written picture.
    let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    let done = std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, dir.join(&name)));
    if let Err(e) = done {
        eprintln!("lucent: cannot store {name}: {e}");
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(url_for(&name))
}

/// Widest useful size per kind (2x the card's CSS px for HiDPI). The sources are 1920-2048
/// px wide: decoding those in the dock page on each card cost memory and, under load,
/// frames where the agent was missing.
fn max_width(kind: Kind) -> Option<u32> {
    match kind {
        Kind::AgentPortrait => Some(900),
        Kind::MapSplash => Some(860),
        _ => None,
    }
}

/// Downscales a big picture (PNG in, PNG out). Any failure keeps the original bytes.
fn shrink(kind: Kind, bytes: Vec<u8>) -> Vec<u8> {
    let Some(max) = max_width(kind) else { return bytes };
    let Ok(img) = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png) else { return bytes };
    if img.width() <= max {
        return bytes;
    }
    let h = (img.height() as u64 * max as u64 / img.width() as u64).max(1) as u32;
    let small = img.resize_exact(max, h, image::imageops::FilterType::Triangle);
    let mut out = std::io::Cursor::new(Vec::new());
    match small.write_to(&mut out, image::ImageFormat::Png) {
        Ok(()) => out.into_inner(),
        Err(_) => bytes,
    }
}

/// `ensure` for several pictures at once (one thread each, duplicates fetched once), in
/// the order of `wants`.
pub fn ensure_all(wants: &[Want]) -> Vec<Option<String>> {
    let mut unique: Vec<&Want> = Vec::new();
    for w in wants {
        if !unique.contains(&w) {
            unique.push(w);
        }
    }
    let got: HashMap<&Want, Option<String>> = std::thread::scope(|s| {
        let handles: Vec<_> = unique.iter().map(|&w| (w, s.spawn(move || ensure(w)))).collect();
        handles.into_iter().map(|(w, h)| (w, h.join().ok().flatten())).collect()
    });
    wants.iter().map(|w| got.get(w).cloned().flatten()).collect()
}

/// Registers the `valo` scheme (cache files only, by whitelisted name).
pub fn register_protocol<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol(SCHEME, move |_ctx, request, responder| {
        let name = request.uri().path().trim_start_matches('/').to_string();
        std::thread::spawn(move || {
            let bytes = if is_allowed_name(&name) { std::fs::read(dir().join(&name)).ok() } else { None };
            let resp = match bytes {
                Some(b) => tauri::http::Response::builder()
                    .status(200)
                    .header("Content-Type", "image/png")
                    .header("Cache-Control", "max-age=31536000, immutable")
                    .header("Access-Control-Allow-Origin", "*")
                    .body(b),
                None => tauri::http::Response::builder().status(404).body(Vec::new()),
            };
            if let Ok(r) = resp {
                responder.respond(r);
            }
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FADE: &str = "dade69b4-4f5a-8528-247b-219e5a1facd6";
    const BIND: &str = "2c9d57ec-4431-9c5e-2939-8f9ef6dd5cba";

    #[test]
    fn shrink_downscales_big_portraits_only() {
        let png = |w: u32, h: u32| {
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgba8(w, h).write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        let small = image::load_from_memory(&shrink(Kind::AgentPortrait, png(2048, 1860))).unwrap();
        assert_eq!((small.width(), small.height()), (900, 817));
        let icon = png(256, 256);
        assert_eq!(shrink(Kind::Tier, icon.clone()), icon);
        assert_eq!(shrink(Kind::AgentPortrait, b"not a png".to_vec()), b"not a png".to_vec());
    }

    #[test]
    fn uuid_normalization() {
        assert_eq!(normalize_uuid(" DADE69B4-4F5A-8528-247B-219E5A1FACD6 ").as_deref(), Some(FADE));
        for bad in ["", "dade69b4", "dade69b4-4f5a-8528-247b-219e5a1facdz", "dade69b4_4f5a-8528-247b-219e5a1facd6", "../../../../windows/system32/x"] {
            assert_eq!(normalize_uuid(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn file_names_and_urls() {
        assert_eq!(file_name(Kind::AgentPortrait, FADE).as_deref(), Some("agent-portrait-dade69b4-4f5a-8528-247b-219e5a1facd6.png"));
        assert_eq!(file_name(Kind::MapList, BIND).as_deref(), Some("map-list-2c9d57ec-4431-9c5e-2939-8f9ef6dd5cba.png"));
        assert_eq!(file_name(Kind::Tier, "19").as_deref(), Some("tier-19.png"));
        assert_eq!(file_name(Kind::Tier, "019"), None);
        assert_eq!(file_name(Kind::Tier, FADE), None);
        assert_eq!(file_name(Kind::MapSplash, "19"), None);
        assert_eq!(file_name(Kind::AgentIcon, &FADE.to_uppercase()), None, "keys are canonical");
        assert_eq!(url_for("tier-19.png"), "http://valo.localhost/tier-19.png");
    }

    #[test]
    fn whitelist_rejects_traversal_and_other_names() {
        for ok in [
            "tier-0.png".to_string(),
            "tier-27.png".to_string(),
            format!("agent-portrait-{FADE}.png"),
            format!("agent-icon-{FADE}.png"),
            format!("map-splash-{BIND}.png"),
            format!("map-list-{BIND}.png"),
        ] {
            assert!(is_allowed_name(&ok), "{ok}");
        }
        for bad in [
            "".to_string(),
            "tier-19".to_string(),
            "tier-.png".to_string(),
            "tier-100.png".to_string(),
            "../tier-19.png".to_string(),
            "..\\tier-19.png".to_string(),
            "tier-19.png/../../settings.json".to_string(),
            "tier-1%2e.png".to_string(),
            format!("map-splash-{BIND}.png.tmp"),
            format!("map-splash-../{BIND}.png"),
            format!("agent-{FADE}.png"),
            format!("map-splash-{}.png", BIND.to_uppercase()),
            format!("C:\\Windows\\map-list-{BIND}.png"),
            "settings.json".to_string(),
        ] {
            assert!(!is_allowed_name(&bad), "{bad}");
        }
    }

    #[test]
    fn source_urls_stay_on_the_media_host() {
        assert_eq!(
            source_url(Kind::AgentPortrait, FADE, None).as_deref(),
            Some("https://media.valorant-api.com/agents/dade69b4-4f5a-8528-247b-219e5a1facd6/fullportrait.png")
        );
        assert_eq!(
            source_url(Kind::MapSplash, BIND, None).as_deref(),
            Some("https://media.valorant-api.com/maps/2c9d57ec-4431-9c5e-2939-8f9ef6dd5cba/splash.png")
        );
        assert_eq!(
            source_url(Kind::Tier, "19", None).as_deref(),
            Some("https://media.valorant-api.com/competitivetiers/03621f52-342b-cf4e-4f86-9350a49c6d04/19/largeicon.png")
        );
        let mut t = HashMap::new();
        t.insert(19, "https://media.valorant-api.com/competitivetiers/x/19/largeicon.png".to_string());
        t.insert(20, "https://evil.example/20.png".to_string());
        assert_eq!(source_url(Kind::Tier, "19", Some(&t)).as_deref(), Some("https://media.valorant-api.com/competitivetiers/x/19/largeicon.png"));
        assert_eq!(source_url(Kind::Tier, "20", Some(&t)), None);
        assert_eq!(source_url(Kind::MapList, "Bind", None), None);
    }

    #[test]
    fn catalog_parsing_and_name_fallback() {
        let maps = format!(r#"{{"status":200,"data":[{{"uuid":"{BIND}","displayName":"Bind"}},{{"uuid":"bad","displayName":"X"}}]}}"#);
        let idx = parse_name_index(&maps);
        assert_eq!(idx.len(), 1);
        assert_eq!(resolve_key(&Want::new(Kind::MapSplash, "", "BIND"), Some(&idx)).as_deref(), Some(BIND));
        assert_eq!(resolve_key(&Want::new(Kind::MapSplash, "", "Lotus"), Some(&idx)), None);
        assert_eq!(resolve_key(&Want::new(Kind::MapSplash, "", "Bind"), None), None);
        // The match's own uuid wins over the name.
        assert_eq!(resolve_key(&Want::new(Kind::AgentIcon, &FADE.to_uppercase(), "Jett"), Some(&idx)).as_deref(), Some(FADE));
        assert_eq!(resolve_key(&Want::tier(19), None).as_deref(), Some("19"));
        assert!(parse_name_index("nope").is_empty());

        let tiers = r#"{"data":[{"tiers":[{"tier":19,"largeIcon":"old"}]},{"tiers":[{"tier":19,"largeIcon":"new"},{"tier":0,"largeIcon":null}]}]}"#;
        let t = parse_tier_icons(tiers);
        assert_eq!(t.get(&19).map(String::as_str), Some("new"));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn cached_without_files_or_ids_is_none() {
        assert_eq!(cached(&Want::new(Kind::MapSplash, "", "")), None);
        assert_eq!(cached(&Want::new(Kind::MapSplash, "not-a-uuid", "")), None);
    }
}
