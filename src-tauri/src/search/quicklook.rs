//! Quick Look: preview of the selected search result (Maj+Espace in the search page).
//!
//! The page only ever sends an entry id; the path is resolved here from the index. Media is
//! served through the `qlook` custom protocol (`http://qlook.localhost/<token>` in WebView2):
//! a random token maps to one path, lives until the preview closes (or the next one opens),
//! and an unknown token is a 404. WSL files are only read while their distro runs.
use super::{Entry, Kind, SearchIndex};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager};

/// Text previews: files up to 1 MiB, the first 400 lines.
pub const TEXT_MAX_BYTES: u64 = 1024 * 1024;
pub const TEXT_MAX_LINES: usize = 400;
/// Largest slice answered to one (range) request.
const CHUNK: u64 = 8 * 1024 * 1024;
/// Largest file answered whole to a request without `Range` (else 416).
const WHOLE_MAX: u64 = 256 * 1024 * 1024;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PreviewKind {
    Image,
    Pdf,
    Text,
    Video,
    #[default]
    None,
}

/// Matches the TS `QuickLookPreview`; `name`/`size`/`modified` (ms since epoch) feed the
/// `none` card.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub kind: PreviewKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<u64>,
}

fn none(name: &str) -> Preview {
    Preview { name: Some(name.to_string()), ..Default::default() }
}

fn ext_of(path: &str) -> String {
    path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Preview kind by file extension (case-insensitive).
pub fn kind_for(path: &str) -> PreviewKind {
    match ext_of(path).as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => PreviewKind::Image,
        "pdf" => PreviewKind::Pdf,
        "mp4" | "webm" | "mov" => PreviewKind::Video,
        "txt" | "md" | "rs" | "ts" | "js" | "json" | "toml" | "yaml" | "yml" | "rb" | "py" | "html" | "css" | "sh"
        | "log" | "csv" => PreviewKind::Text,
        _ => PreviewKind::None,
    }
}

fn mime_for(path: &str) -> &'static str {
    match ext_of(path).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// First `max_lines` lines of `bytes` (UTF-8 lossy).
pub fn first_lines(bytes: &[u8], max_lines: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    for (i, line) in text.lines().take(max_lines).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

// ---------- tokens ----------

/// Random token -> path. Not consumed by a read (video seeks send many requests): cleared
/// when the preview closes.
#[derive(Default)]
pub struct Tokens {
    map: Mutex<HashMap<String, PathBuf>>,
}

fn random_token() -> Option<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

impl Tokens {
    /// A fresh token for `path`; any previous one is dropped (one preview at a time).
    pub fn issue(&self, path: PathBuf) -> Option<String> {
        let token = random_token()?;
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        m.clear();
        m.insert(token.clone(), path);
        Some(token)
    }
    pub fn resolve(&self, token: &str) -> Option<PathBuf> {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).get(token).cloned()
    }
    pub fn clear(&self) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

fn tokens() -> &'static Tokens {
    static T: OnceLock<Tokens> = OnceLock::new();
    T.get_or_init(Default::default)
}

// ---------- preview ----------

fn modified_ms(m: &std::fs::Metadata) -> Option<u64> {
    let d = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_millis() as u64)
}

/// Preview of the file at `path` (`title` = its display name).
pub fn preview_path(path: &Path, title: &str, tokens: &Tokens) -> Preview {
    let Ok(meta) = std::fs::metadata(path) else { return none(title) };
    let mut card = none(title);
    card.modified = modified_ms(&meta);
    if meta.is_dir() {
        return card;
    }
    card.size = Some(meta.len());
    match kind_for(&path.to_string_lossy()) {
        PreviewKind::None => card,
        PreviewKind::Text => {
            if meta.len() > TEXT_MAX_BYTES {
                return card;
            }
            let mut buf = Vec::new();
            match std::fs::File::open(path).and_then(|f| f.take(TEXT_MAX_BYTES).read_to_end(&mut buf)) {
                Ok(_) => Preview { kind: PreviewKind::Text, text: Some(first_lines(&buf, TEXT_MAX_LINES)), ..card },
                Err(_) => card,
            }
        }
        k => match tokens.issue(path.to_path_buf()) {
            Some(t) => Preview { kind: k, url: Some(format!("http://qlook.localhost/{t}")), ..card },
            None => card,
        },
    }
}

/// Preview of an indexed entry. Apps and projects have no file to show (name + icon card);
/// WSL files are not touched unless `wsl_running(distro)`.
pub fn preview_entry(entry: &Entry, tokens: &Tokens, wsl_running: &dyn Fn(&str) -> bool) -> Preview {
    if entry.kind != Kind::File {
        return none(&entry.title);
    }
    let Some(path) = entry.id.strip_prefix("file:") else { return none(&entry.title) };
    if let Some((d, _)) = super::files::wsl_to_linux(path) {
        if !wsl_running(&d) {
            return none(&entry.title);
        }
    }
    preview_path(Path::new(path), &entry.title, tokens)
}

// ---------- commands ----------

#[tauri::command]
pub async fn quicklook_open(app: AppHandle, id: String) -> Result<Preview, String> {
    let index = app.state::<SearchIndex>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        tokens().clear();
        match index.find(&id, "") {
            Some(e) => preview_entry(&e, tokens(), &super::wsl::is_running),
            None => Preview::default(),
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// The preview closed: its token stops working.
#[tauri::command]
pub fn quicklook_close() {
    tokens().clear();
}

// ---------- protocol ----------

/// `bytes=a-b` / `bytes=a-` / `bytes=-n` -> inclusive `(start, end)` within `len`.
pub fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let spec = h.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (len.saturating_sub(n), len - 1)
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() { len - 1 } else { b.parse().ok()? };
        (s, e.min(len - 1))
    };
    (start <= end && start < len).then_some((start, end))
}

/// No CORS header (only the app's own pages load these URLs as media); `nosniff` so the
/// declared type is the only one used; `sandbox` so a served document (an SVG opened as a
/// page) never runs script with an origin. Not on PDFs: WebView2 refuses to run its PDF
/// viewer in a sandboxed document.
fn respond(status: u16, ct: &str, extra: Vec<(&str, String)>, body: Vec<u8>) -> tauri::http::Response<Vec<u8>> {
    let mut b = tauri::http::Response::builder()
        .status(status)
        .header("Content-Type", ct)
        .header("Accept-Ranges", "bytes")
        .header("X-Content-Type-Options", "nosniff");
    if ct != "application/pdf" {
        b = b.header("Content-Security-Policy", "sandbox");
    }
    for (k, v) in extra {
        b = b.header(k, v);
    }
    b.body(body).unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
}

/// Serves `token` (optionally a byte `range`). Unknown/expired token or unreadable file: 404.
pub fn serve(tokens: &Tokens, token: &str, range: Option<&str>) -> tauri::http::Response<Vec<u8>> {
    serve_capped(tokens, token, range, WHOLE_MAX)
}

/// `serve` with the whole-file cap as a parameter. With a valid `Range`: 206 with at most
/// `CHUNK` bytes. Without (or unparseable): 200 with the whole file, or 416 past `whole_max`.
fn serve_capped(tokens: &Tokens, token: &str, range: Option<&str>, whole_max: u64) -> tauri::http::Response<Vec<u8>> {
    let not_found = || respond(404, "text/plain", vec![], Vec::new());
    let Some(path) = tokens.resolve(token) else { return not_found() };
    let Ok(mut f) = std::fs::File::open(&path) else { return not_found() };
    let Ok(len) = f.metadata().map(|m| m.len()) else { return not_found() };
    let ct = mime_for(&path.to_string_lossy());
    let (start, end, partial) = match range.and_then(|r| parse_range(r, len)) {
        Some((s, e)) => (s, e.min(s + CHUNK - 1), true),
        None if len > whole_max => {
            return respond(416, "text/plain", vec![("Content-Range", format!("bytes */{len}"))], Vec::new());
        }
        None => (0, len.saturating_sub(1), false),
    };
    let mut body = Vec::new();
    if len > 0 && (f.seek(SeekFrom::Start(start)).is_err() || f.take(end - start + 1).read_to_end(&mut body).is_err()) {
        return not_found();
    }
    if partial {
        let cr = format!("bytes {start}-{end}/{len}");
        respond(206, ct, vec![("Content-Range", cr)], body)
    } else {
        respond(200, ct, vec![], body)
    }
}

/// Registers the `qlook` scheme (reads files, so off the main thread).
pub fn register_protocol<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol("qlook", move |_ctx, request, responder| {
        let token = request.uri().path().trim_start_matches('/').to_string();
        let range = request.headers().get("range").and_then(|v| v.to_str().ok()).map(str::to_string);
        std::thread::spawn(move || {
            responder.respond(serve(tokens(), &token, range.as_deref()));
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lucent-qlook-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn kind_detection_table() {
        use PreviewKind::*;
        let table = [
            ("a.png", Image), ("a.JPG", Image), ("a.jpeg", Image), ("a.gif", Image), ("a.webp", Image),
            ("a.bmp", Image), ("a.svg", Image), ("a.pdf", Pdf), ("a.mp4", Video), ("a.webm", Video),
            ("a.MOV", Video), ("a.txt", Text), ("a.md", Text), ("a.rs", Text), ("a.ts", Text), ("a.js", Text),
            ("a.json", Text), ("a.toml", Text), ("a.yaml", Text), ("a.yml", Text), ("a.rb", Text),
            ("a.py", Text), ("a.html", Text), ("a.css", Text), ("a.sh", Text), ("a.log", Text), ("a.csv", Text),
            ("a.exe", None), ("noext", None), ("a.", None), (r"C:\x.d\file", None),
        ];
        for (p, k) in table {
            assert_eq!(kind_for(p), k, "{p}");
        }
    }

    #[test]
    fn text_is_capped_to_400_lines_and_1_mib() {
        let d = tmp("text");
        let many: String = (0..1000).map(|i| format!("l{i}\n")).collect();
        std::fs::write(d.join("a.txt"), &many).unwrap();
        let t = Tokens::default();
        let p = preview_path(&d.join("a.txt"), "a.txt", &t);
        assert_eq!(p.kind, PreviewKind::Text);
        let text = p.text.unwrap();
        assert_eq!(text.lines().count(), 400);
        assert!(text.ends_with("l399"));

        std::fs::write(d.join("lossy.txt"), [b'a', 0xff, b'b']).unwrap();
        assert_eq!(preview_path(&d.join("lossy.txt"), "x", &t).text.unwrap(), "a\u{fffd}b");

        std::fs::write(d.join("big.log"), vec![b'x'; TEXT_MAX_BYTES as usize + 1]).unwrap();
        let big = preview_path(&d.join("big.log"), "big.log", &t);
        assert_eq!(big.kind, PreviewKind::None);
        assert_eq!(big.size, Some(TEXT_MAX_BYTES + 1));
        std::fs::write(d.join("edge.log"), vec![b'x'; TEXT_MAX_BYTES as usize]).unwrap();
        assert_eq!(preview_path(&d.join("edge.log"), "e", &t).kind, PreviewKind::Text);
    }

    #[test]
    fn unknown_kind_and_missing_files_give_the_card() {
        let d = tmp("none");
        std::fs::write(d.join("a.bin"), b"1234").unwrap();
        let t = Tokens::default();
        let p = preview_path(&d.join("a.bin"), "a.bin", &t);
        assert_eq!((p.kind, p.size, p.name.as_deref()), (PreviewKind::None, Some(4), Some("a.bin")));
        assert!(p.modified.is_some() && p.url.is_none());
        assert_eq!(preview_path(&d.join("gone.png"), "gone.png", &t).kind, PreviewKind::None);
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn media_gets_a_random_token_url_that_hides_the_path() {
        let d = tmp("media");
        std::fs::write(d.join("a.png"), b"png").unwrap();
        let t = Tokens::default();
        let p = preview_path(&d.join("a.png"), "a.png", &t);
        assert_eq!(p.kind, PreviewKind::Image);
        let url = p.url.unwrap();
        let token = url.strip_prefix("http://qlook.localhost/").unwrap();
        assert_eq!(token.len(), 32);
        assert!(!url.contains("a.png") && !url.contains("lucent"));
        assert_eq!(t.resolve(token), Some(d.join("a.png")));
        // A second preview replaces the first token.
        std::fs::write(d.join("b.pdf"), b"pdf").unwrap();
        let p2 = preview_path(&d.join("b.pdf"), "b.pdf", &t);
        let token2 = p2.url.unwrap().rsplit('/').next().unwrap().to_string();
        assert_ne!(token, token2);
        assert_eq!(t.resolve(token), None);
        t.clear();
        assert_eq!(t.resolve(&token2), None);
    }

    #[test]
    fn unknown_or_expired_token_is_404_and_known_serves_ranges() {
        let d = tmp("serve");
        std::fs::write(d.join("v.mp4"), b"0123456789").unwrap();
        let t = Tokens::default();
        assert_eq!(serve(&t, "nope", None).status(), 404);
        let token = t.issue(d.join("v.mp4")).unwrap();
        let r = serve(&t, &token, None);
        assert_eq!((r.status().as_u16(), r.body().as_slice()), (200, &b"0123456789"[..]));
        let r = serve(&t, &token, Some("bytes=2-4"));
        assert_eq!(r.status().as_u16(), 206);
        assert_eq!(r.body().as_slice(), b"234");
        assert_eq!(r.headers().get("Content-Range").unwrap(), "bytes 2-4/10");
        t.clear();
        assert_eq!(serve(&t, &token, None).status(), 404);
        let token = t.issue(d.join("gone.mp4")).unwrap();
        assert_eq!(serve(&t, &token, None).status(), 404);
    }

    #[test]
    fn without_range_the_whole_file_is_200_up_to_the_cap() {
        let d = tmp("whole");
        std::fs::write(d.join("v.mp4"), b"0123456789").unwrap();
        let t = Tokens::default();
        let token = t.issue(d.join("v.mp4")).unwrap();
        let r = serve_capped(&t, &token, None, 10);
        assert_eq!((r.status().as_u16(), r.body().as_slice()), (200, &b"0123456789"[..]));
        assert!(r.headers().get("Content-Range").is_none());
        let r = serve_capped(&t, &token, None, 9);
        assert_eq!(r.status().as_u16(), 416);
        assert_eq!(r.headers().get("Content-Range").unwrap(), "bytes */10");
        assert!(r.body().is_empty());
        // A range is still served past the whole-file cap.
        assert_eq!(serve_capped(&t, &token, Some("bytes=0-1"), 1).status().as_u16(), 206);
    }

    #[test]
    fn responses_are_not_cors_open_and_are_sandboxed() {
        let d = tmp("headers");
        std::fs::write(d.join("a.svg"), b"<svg/>").unwrap();
        std::fs::write(d.join("a.pdf"), b"%PDF").unwrap();
        let t = Tokens::default();
        let token = t.issue(d.join("a.svg")).unwrap();
        let r = serve(&t, &token, None);
        assert!(r.headers().get("Access-Control-Allow-Origin").is_none());
        assert_eq!(r.headers().get("X-Content-Type-Options").unwrap(), "nosniff");
        assert_eq!(r.headers().get("Content-Security-Policy").unwrap(), "sandbox");
        let token = t.issue(d.join("a.pdf")).unwrap();
        let r = serve(&t, &token, None);
        assert_eq!(r.headers().get("X-Content-Type-Options").unwrap(), "nosniff");
        assert!(r.headers().get("Access-Control-Allow-Origin").is_none());
        assert_eq!(serve(&t, "nope", None).headers().get("X-Content-Type-Options").unwrap(), "nosniff");
    }

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range("bytes=0-3", 10), Some((0, 3)));
        assert_eq!(parse_range("bytes=5-", 10), Some((5, 9)));
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=2-99", 10), Some((2, 9)));
        assert_eq!(parse_range("bytes=10-", 10), None);
        assert_eq!(parse_range("bytes=4-2", 10), None);
        assert_eq!(parse_range("bytes=0-1,3-4", 10), None);
        assert_eq!(parse_range("items=0-1", 10), None);
    }

    fn entry(id: &str, kind: Kind) -> Entry {
        Entry { id: id.into(), kind, title: "t".into(), subtitle: String::new(), icon: None, keywords: vec![], valo: None }
    }

    #[test]
    fn wsl_files_need_a_running_distro_and_apps_projects_get_the_card() {
        let t = Tokens::default();
        let wsl = entry(r"file:\\wsl.localhost\Ubuntu\home\u\a.png", Kind::File);
        let asked = std::cell::Cell::new(0);
        let p = preview_entry(&wsl, &t, &|_| {
            asked.set(asked.get() + 1);
            false
        });
        assert_eq!((p.kind, asked.get()), (PreviewKind::None, 1));
        assert!(p.url.is_none() && p.size.is_none());
        assert_eq!(preview_entry(&entry("app:x", Kind::App), &t, &|_| true).kind, PreviewKind::None);
        assert_eq!(preview_entry(&entry("proj:/home/u/p", Kind::Project), &t, &|_| true).kind, PreviewKind::None);
    }
}
