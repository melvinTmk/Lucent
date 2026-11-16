//! Icon service: disk cache + `icon` URI scheme. Extraction is behind `IconSource`.
use crate::settings::AppId;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
pub mod win;

const ICON_SIZE: u32 = 256;
pub(crate) const GENERIC_ICON: &[u8] = include_bytes!("../../assets/generic-app.png");

#[derive(Debug)]
pub struct IconError(pub String);

impl std::fmt::Display for IconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait IconSource {
    /// PNG bytes of the app icon at about `size` px.
    fn extract(&self, app: &AppId, size: u32) -> Result<Vec<u8>, IconError>;
}

/// Lowercase hex FNV-1a 64 of the `AppId` JSON. Stable across runs and versions.
pub fn cache_key(app: &AppId) -> String {
    let json = serde_json::to_string(app).unwrap_or_default();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in json.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// What a protocol key resolves to.
#[derive(Clone, Debug)]
pub enum IconEntry {
    App(AppId),
    /// In-memory PNG (tray icons, key `tray-<TrayKey>`).
    Png(std::sync::Arc<Vec<u8>>),
}

/// Process-wide key -> source map read by the `icon` protocol handler.
fn registry() -> &'static Mutex<HashMap<String, IconEntry>> {
    static REG: OnceLock<Mutex<HashMap<String, IconEntry>>> = OnceLock::new();
    REG.get_or_init(Default::default)
}

fn register(key: String, entry: IconEntry) {
    registry().lock().unwrap_or_else(|e| e.into_inner()).insert(key, entry);
}

/// Serves `png` at `http://icon.localhost/<key>.png` (replaces any previous entry).
pub fn register_png(key: &str, png: Vec<u8>) {
    register(key.to_string(), IconEntry::Png(std::sync::Arc::new(png)));
}

/// Drops in-memory PNG entries whose key starts with `prefix` and fails `keep`.
pub fn retain_pngs(prefix: &str, keep: impl Fn(&str) -> bool) {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|k, e| !(matches!(e, IconEntry::Png(_)) && k.starts_with(prefix)) || keep(k));
}

fn lookup(key: &str) -> Option<IconEntry> {
    registry().lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned()
}

/// URL the webview uses for the app icon (WebView2 exposes custom scheme `icon` as
/// `http://icon.localhost`). Also records the key so the protocol can resolve it.
pub fn icon_url(app: &AppId) -> String {
    let key = cache_key(app);
    register(key.clone(), IconEntry::App(app.clone()));
    format!("http://icon.localhost/{key}.png")
}

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

pub struct IconCache<S: IconSource> {
    pub dir: PathBuf,
    pub source: S,
    /// Single-flight: one lock per key so concurrent misses extract once.
    locks: Mutex<HashMap<String, std::sync::Arc<Mutex<()>>>>,
}

impl<S: IconSource> IconCache<S> {
    pub fn new(dir: PathBuf, source: S) -> Self {
        Self { dir, source, locks: Mutex::new(HashMap::new()) }
    }

    /// Cached file if present and a plausible PNG; a corrupt one is deleted (treated as a miss).
    fn read_cached(path: &std::path::Path) -> Option<Vec<u8>> {
        let bytes = std::fs::read(path).ok()?;
        if bytes.starts_with(PNG_SIGNATURE) {
            Some(bytes)
        } else {
            let _ = std::fs::remove_file(path);
            None
        }
    }

    /// Cache hit: file. Miss: extract at 256 px and write. Error: generic icon, not cached.
    pub fn get_png(&self, app: &AppId) -> Vec<u8> {
        let key = cache_key(app);
        let path = self.dir.join(format!("{key}.png"));
        if let Some(bytes) = Self::read_cached(&path) {
            return bytes;
        }
        let lock = self
            .locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key.clone())
            .or_default()
            .clone();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        // Another thread may have filled the cache while we waited.
        if let Some(bytes) = Self::read_cached(&path) {
            return bytes;
        }
        match self.source.extract(app, ICON_SIZE) {
            Ok(png) => {
                static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let _ = std::fs::create_dir_all(&self.dir);
                let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let tmp = self.dir.join(format!("{key}.{}.{n}.tmp", std::process::id()));
                let ok = std::fs::write(&tmp, &png).is_ok() && std::fs::rename(&tmp, &path).is_ok();
                if !ok {
                    let _ = std::fs::remove_file(&tmp);
                }
                png
            }
            Err(e) => {
                eprintln!("lucent: icon extraction failed for {app:?}: {e}");
                GENERIC_ICON.to_vec()
            }
        }
    }
}

/// Converts a premultiplied top-down BGRA buffer to straight RGBA and encodes a PNG.
/// A buffer whose alpha is entirely zero is treated as opaque (icon without alpha).
pub fn bgra_premul_to_png(bgra: &[u8], width: u32, height: u32) -> Result<Vec<u8>, IconError> {
    let expected = (width as usize) * (height as usize) * 4;
    if width == 0 || height == 0 || bgra.len() != expected {
        return Err(IconError(format!("bad bitmap buffer {}x{} len {}", width, height, bgra.len())));
    }
    let opaque = bgra.chunks_exact(4).all(|p| p[3] == 0);
    let mut rgba = Vec::with_capacity(expected);
    for p in bgra.chunks_exact(4) {
        let a = if opaque { 255 } else { p[3] };
        let un = |c: u8| -> u8 {
            if opaque || a == 255 { c } else if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 }
        };
        rgba.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), a]);
    }
    encode_png(rgba, width, height)
}

pub fn encode_png(rgba: Vec<u8>, width: u32, height: u32) -> Result<Vec<u8>, IconError> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| IconError(format!("png encode: {e}")))?;
    Ok(out)
}

/// Registers the `icon` scheme. Extraction can block on the shell, so it runs off the
/// main thread (asynchronous protocol).
pub fn register_protocol<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    #[cfg(windows)]
    let cache = std::sync::Arc::new(IconCache::new(
        crate::paths::cache_dir().join("icons"),
        win::WinIconSource,
    ));
    builder.register_asynchronous_uri_scheme_protocol("icon", move |_ctx, request, responder| {
        let key = request
            .uri()
            .path()
            .trim_start_matches('/')
            .trim_end_matches(".png")
            .to_string();
        #[cfg(windows)]
        let cache = cache.clone();
        std::thread::spawn(move || {
            let png = match lookup(&key) {
                #[cfg(windows)]
                Some(IconEntry::App(app)) => Some(cache.get_png(&app)),
                #[cfg(not(windows))]
                Some(IconEntry::App(_)) => Some(GENERIC_ICON.to_vec()),
                Some(IconEntry::Png(png)) => Some(png.as_ref().clone()),
                None => None,
            };
            let resp = match png {
                Some(bytes) => tauri::http::Response::builder()
                    .status(200)
                    .header("Content-Type", "image/png")
                    .header("Access-Control-Allow-Origin", "*")
                    .body(bytes),
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
    use std::sync::Mutex;

    struct FakeSource {
        calls: Mutex<Vec<(AppId, u32)>>,
        fail: bool,
    }
    impl FakeSource {
        fn new(fail: bool) -> Self {
            Self { calls: Mutex::new(vec![]), fail }
        }
    }
    impl IconSource for FakeSource {
        fn extract(&self, app: &AppId, size: u32) -> Result<Vec<u8>, IconError> {
            self.calls.lock().unwrap().push((app.clone(), size));
            if self.fail { Err(IconError("access denied".into())) } else { Ok(fake_png()) }
        }
    }

    fn fake_png() -> Vec<u8> {
        let mut v = PNG_SIGNATURE.to_vec();
        v.extend_from_slice(&[1, 2, 3]);
        v
    }
    fn a() -> AppId { AppId::Exe(r"c:\a.exe".into()) }
    fn b() -> AppId { AppId::Aumid("Pkg!App".into()) }
    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lucent-icons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn cache_key_is_stable_and_distinct() {
        assert_eq!(cache_key(&a()), cache_key(&a().clone()));
        assert_ne!(cache_key(&a()), cache_key(&b()));
        assert_eq!(cache_key(&a()).len(), 16);
    }

    #[test]
    fn miss_extracts_at_256_and_writes_file() {
        let dir = tmp("miss");
        let c = IconCache::new(dir.clone(), FakeSource::new(false));
        assert_eq!(c.get_png(&a()), fake_png());
        assert_eq!(*c.source.calls.lock().unwrap(), vec![(a(), 256)]);
        assert!(dir.join(format!("{}.png", cache_key(&a()))).exists());
    }

    #[test]
    fn hit_does_not_call_source() {
        let c = IconCache::new(tmp("hit"), FakeSource::new(false));
        c.get_png(&a());
        assert_eq!(c.get_png(&a()), fake_png());
        assert_eq!(c.source.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn access_denied_falls_back_to_generic_icon() {
        let dir = tmp("fail");
        let c = IconCache::new(dir.clone(), FakeSource::new(true));
        assert_eq!(c.get_png(&a()), include_bytes!("../../assets/generic-app.png"));
        assert!(!dir.join(format!("{}.png", cache_key(&a()))).exists());
    }

    #[test]
    fn bgra_premultiplied_becomes_straight_rgba() {
        // one pixel: premultiplied B=0, G=64, R=128 at alpha 128 -> straight R=255, G=127/128, B=0
        let png = bgra_premul_to_png(&[0, 64, 128, 128], 1, 1).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        let p = img.get_pixel(0, 0).0;
        assert_eq!((p[0], p[2], p[3]), (255, 0, 128));
        assert!((126..=129).contains(&p[1]));
        // no alpha at all -> opaque
        let img = image::load_from_memory(&bgra_premul_to_png(&[10, 20, 30, 0], 1, 1).unwrap()).unwrap().to_rgba8();
        assert_eq!(img.get_pixel(0, 0).0, [30, 20, 10, 255]);
    }

    #[test]
    fn concurrent_misses_extract_once_and_file_is_valid() {
        let dir = tmp("conc");
        let c = std::sync::Arc::new(IconCache::new(dir.clone(), FakeSource::new(false)));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let c = c.clone();
                std::thread::spawn(move || c.get_png(&a()))
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap(), fake_png());
        }
        assert_eq!(c.source.calls.lock().unwrap().len(), 1);
        let file = dir.join(format!("{}.png", cache_key(&a())));
        assert_eq!(std::fs::read(file).unwrap(), fake_png());
        let tmps = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "tmp")).count();
        assert_eq!(tmps, 0);
    }

    #[test]
    fn corrupt_cache_file_is_reextracted() {
        let dir = tmp("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{}.png", cache_key(&a())));
        std::fs::write(&file, b"\x89PN").unwrap(); // truncated
        let c = IconCache::new(dir, FakeSource::new(false));
        assert_eq!(c.get_png(&a()), fake_png());
        assert_eq!(c.source.calls.lock().unwrap().len(), 1);
        assert_eq!(std::fs::read(file).unwrap(), fake_png());
    }

    #[test]
    fn register_png_then_retain() {
        register_png("tray-a", vec![1]);
        register_png("tray-b", vec![2]);
        assert!(matches!(lookup("tray-a"), Some(IconEntry::Png(p)) if *p == vec![1]));
        retain_pngs("tray-", |k| k == "tray-b");
        assert!(lookup("tray-a").is_none());
        assert!(lookup("tray-b").is_some());
        retain_pngs("tray-", |_| false);
        assert!(lookup("tray-b").is_none());
    }

    #[test]
    fn icon_url_format() {
        assert_eq!(icon_url(&a()), format!("http://icon.localhost/{}.png", cache_key(&a())));
    }
}
