//! Clipboard for search: the Win32 writer, plus the in-memory history (never written to disk).
use super::{Entry, Kind};
use std::sync::Mutex;

pub const RING_CAP: usize = 50;
const TITLE_MAX: usize = 80;
/// Texts longer than this (in UTF-16 units) are not recorded.
const MAX_TEXT_UNITS: usize = 100_000;

pub const FORMAT_EXCLUDE_MONITOR: &str = "ExcludeClipboardContentFromMonitorProcessing";
pub const FORMAT_CAN_INCLUDE_HISTORY: &str = "CanIncludeInClipboardHistory";
pub const FORMAT_CAN_UPLOAD_CLOUD: &str = "CanUploadToCloudClipboard";

mod text {
    pub const SUB_COPY: &str = "Entrée : copier";
}

/// A registered clipboard format present on the clipboard, with its DWORD value when it
/// holds one.
pub struct ClipFormat<'a> {
    pub name: &'a str,
    pub dword: Option<u32>,
}

/// True when the clipboard content must not enter the history: the monitor-exclusion
/// format is present, or an include/upload-permission format is present with value 0 or an
/// unreadable value (fail closed: a password manager's intent is never guessed).
pub fn is_excluded(formats: &[ClipFormat]) -> bool {
    formats.iter().any(|f| {
        f.name.eq_ignore_ascii_case(FORMAT_EXCLUDE_MONITOR)
            || ((f.name.eq_ignore_ascii_case(FORMAT_CAN_INCLUDE_HISTORY)
                || f.name.eq_ignore_ascii_case(FORMAT_CAN_UPLOAD_CLOUD))
                && f.dword.is_none_or(|v| v == 0))
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: u64,
    pub text: String,
    /// Unix seconds.
    pub at: u64,
}

/// Newest first, deduplicated by exact text, capped at `RING_CAP`.
#[derive(Default)]
pub struct Ring {
    items: Vec<Item>,
    next_id: u64,
}

impl Ring {
    pub const fn new() -> Self {
        Self { items: Vec::new(), next_id: 0 }
    }
    /// Records `text`; an identical older item is removed so the text appears once, on top.
    pub fn push(&mut self, text: &str, now: u64) {
        if text.trim().is_empty() {
            return;
        }
        self.items.retain(|i| i.text != text);
        self.next_id += 1;
        self.items.insert(0, Item { id: self.next_id, text: text.to_string(), at: now });
        self.items.truncate(RING_CAP);
    }
    pub fn items(&self) -> &[Item] {
        &self.items
    }
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// The ring plus the setting, under one lock: once `set_enabled(false)` has run, a listener
/// update already in flight can no longer record, and nothing can be read back.
pub struct Store {
    ring: Ring,
    enabled: bool,
}

impl Store {
    pub const fn new() -> Self {
        Self { ring: Ring::new(), enabled: false }
    }
    pub fn record(&mut self, text: &str, now: u64) {
        if self.enabled {
            self.ring.push(text, now);
        }
    }
    pub fn snapshot(&self) -> Vec<Item> {
        if self.enabled { self.ring.items().to_vec() } else { Vec::new() }
    }
    /// Turning it off drops the whole history.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.ring.clear();
        }
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

static STORE: Mutex<Store> = Mutex::new(Store::new());

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Ignored while the history is disabled.
pub fn record(text: &str) {
    STORE.lock().unwrap_or_else(|e| e.into_inner()).record(text, now_secs());
}

/// Empty while the history is disabled.
pub fn snapshot() -> Vec<Item> {
    STORE.lock().unwrap_or_else(|e| e.into_inner()).snapshot()
}

/// "à l'instant", "il y a 3 min", "il y a 2 h", "hier", "il y a 3 j".
pub fn relative_time_fr(now: u64, then: u64) -> String {
    let d = now.saturating_sub(then);
    match d {
        0..=59 => "à l'instant".into(),
        60..=3599 => format!("il y a {} min", d / 60),
        3600..=86_399 => format!("il y a {} h", d / 3600),
        86_400..=172_799 => "hier".into(),
        _ => format!("il y a {} j", d / 86_400),
    }
}

/// First non-empty line, trimmed, at most 80 chars (ellipsis when cut). Char-based.
pub fn title_of(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() <= TITLE_MAX {
        return line.to_string();
    }
    let mut t: String = line.chars().take(TITLE_MAX - 1).collect();
    t.push('…');
    t
}

/// The argument of a `cb` / `cb <filter>` query (case-insensitive, byte-safe), else None.
pub fn cb_arg(q: &str) -> Option<&str> {
    let b = q.as_bytes();
    if !b.get(..2).is_some_and(|p| p.eq_ignore_ascii_case(b"cb")) {
        return None;
    }
    match b.get(2) {
        None => Some(""),
        Some(b' ') => q.get(3..).map(str::trim),
        _ => None,
    }
}

/// History entries (newest first) whose text contains `filter` (case-insensitive).
/// The full text travels in `keywords[0]`; ids are per-item counters, never the content.
pub fn entries(filter: &str, items: &[Item], now: u64) -> Vec<Entry> {
    let needle = filter.to_lowercase();
    items
        .iter()
        .filter(|i| needle.is_empty() || i.text.to_lowercase().contains(&needle))
        .map(|i| Entry {
            id: format!("cb:{}", i.id),
            kind: Kind::Clipboard,
            title: title_of(&i.text),
            subtitle: format!("{} · {}", relative_time_fr(now, i.at), text::SUB_COPY),
            icon: None,
            keywords: vec![i.text.clone()],
            valo: None,
        })
        .collect()
}

/// Entry opener (Clipboard kind): puts the text back on the clipboard. The write triggers a
/// clipboard update that `record` dedups: the item just moves to the top.
pub fn open(e: &Entry, _mods: super::OpenMods) {
    if let Some(t) = e.keywords.first() {
        clipboard_write(t);
    }
}

/// Registers the opener and applies the initial setting.
pub fn init(enabled: bool) {
    super::register_opener(Kind::Clipboard, open);
    set_enabled(enabled);
}

/// Starts or stops the clipboard listener. Turning it off drops the whole history; the flag
/// flips first so an update the listener is still handling cannot record afterwards.
pub fn set_enabled(enabled: bool) {
    STORE.lock().unwrap_or_else(|e| e.into_inner()).set_enabled(enabled);
    #[cfg(windows)]
    win::set_enabled(enabled);
}

/// Puts `text` on the clipboard as CF_UNICODETEXT. Errors are logged, never panic.
pub fn clipboard_write(text: &str) {
    #[cfg(windows)]
    if let Err(e) = win::write(text) {
        eprintln!("lucent: cannot write clipboard: {e}");
    }
    #[cfg(not(windows))]
    let _ = text;
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use super::{ClipFormat, MAX_TEXT_UNITS};
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Mutex, Once};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::DataExchange::{
        AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
        OpenClipboard, RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
        PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE,
        WM_CLOSE, WM_DESTROY, WNDCLASSW,
    };
    use windows::core::{w, HSTRING};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};

    const CF_UNICODETEXT: u32 = 13;

    pub fn write(text: &str) -> Result<(), String> {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = wide.len() * 2;
        unsafe {
            // The clipboard can be held by another process for a moment: retry briefly.
            let mut opened = false;
            for _ in 0..10 {
                if OpenClipboard(None).is_ok() {
                    opened = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if !opened {
                return Err("OpenClipboard kept failing".into());
            }
            let r = (|| -> Result<(), String> {
                let h: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| e.to_string())?;
                let p = GlobalLock(h) as *mut u16;
                if p.is_null() {
                    let _ = GlobalFree(Some(h));
                    return Err("GlobalLock failed".into());
                }
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                let _ = GlobalUnlock(h);
                // Only now (buffer ready) is the previous content discarded.
                if let Err(e) = EmptyClipboard() {
                    let _ = GlobalFree(Some(h));
                    return Err(e.to_string());
                }
                // On success the system owns the memory; on failure we free it.
                if let Err(e) = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(h.0))) {
                    let _ = GlobalFree(Some(h));
                    return Err(e.to_string());
                }
                Ok(())
            })();
            let _ = CloseClipboard();
            r
        }
    }

    /// Window of the running listener (0 = none). Also guards against a stale window's updates.
    static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
    static LIFECYCLE: Mutex<()> = Mutex::new(());
    static CLASS: Once = Once::new();

    fn open_clipboard(owner: Option<HWND>) -> bool {
        for _ in 0..5 {
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    /// Reads the DWORD held by a registered format (clipboard must be open).
    unsafe fn read_dword(fmt: u32) -> Option<u32> {
        unsafe {
            let h = GetClipboardData(fmt).ok()?;
            let hg = HGLOBAL(h.0);
            if GlobalSize(hg) < 4 {
                return None;
            }
            let p = GlobalLock(hg) as *const u32;
            if p.is_null() {
                return None;
            }
            let v = p.read_unaligned();
            let _ = GlobalUnlock(hg);
            Some(v)
        }
    }

    /// Clipboard text to record, or None when excluded / not text / empty / too large.
    fn read_history_text(hwnd: HWND) -> Option<String> {
        if !open_clipboard(Some(hwnd)) {
            return None;
        }
        let r = (|| unsafe {
            let mut present: Vec<(&'static str, Option<u32>)> = Vec::new();
            for name in [super::FORMAT_EXCLUDE_MONITOR, super::FORMAT_CAN_INCLUDE_HISTORY, super::FORMAT_CAN_UPLOAD_CLOUD] {
                let id = RegisterClipboardFormatW(&HSTRING::from(name));
                if id != 0 && IsClipboardFormatAvailable(id).is_ok() {
                    present.push((name, read_dword(id)));
                }
            }
            let formats: Vec<ClipFormat> = present.iter().map(|(n, d)| ClipFormat { name: n, dword: *d }).collect();
            if super::is_excluded(&formats) || IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
                return None;
            }
            let h = GetClipboardData(CF_UNICODETEXT).ok()?;
            let hg = HGLOBAL(h.0);
            let max_units = GlobalSize(hg) / 2;
            let p = GlobalLock(hg) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut len = 0;
            while len < max_units && *p.add(len) != 0 {
                len += 1;
            }
            let text = (len <= MAX_TEXT_UNITS).then(|| String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
            let _ = GlobalUnlock(hg);
            text
        })();
        unsafe {
            let _ = CloseClipboard();
        }
        r
    }

    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_CLIPBOARDUPDATE => {
                if HWND_RAW.load(Ordering::SeqCst) == hwnd.0 as isize {
                    if let Some(t) = read_history_text(hwnd) {
                        super::record(&t);
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                unsafe {
                    let _ = RemoveClipboardFormatListener(hwnd);
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    fn run_listener(ready: mpsc::Sender<bool>) {
        unsafe {
            let Ok(hmodule) = GetModuleHandleW(None) else {
                let _ = ready.send(false);
                return;
            };
            let class = w!("LucentClipboard");
            CLASS.call_once(|| {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wnd_proc),
                    hInstance: hmodule.into(),
                    lpszClassName: class,
                    ..Default::default()
                };
                if RegisterClassW(&wc) == 0 {
                    eprintln!("lucent: RegisterClassW failed for clipboard window");
                }
            });
            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("Lucent clipboard"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hmodule.into()),
                None,
            ) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("lucent: CreateWindowExW failed for clipboard window: {e}");
                    let _ = ready.send(false);
                    return;
                }
            };
            if let Err(e) = AddClipboardFormatListener(hwnd) {
                eprintln!("lucent: AddClipboardFormatListener failed: {e}");
                let _ = DestroyWindow(hwnd);
                let _ = ready.send(false);
                return;
            }
            HWND_RAW.store(hwnd.0 as isize, Ordering::SeqCst);
            let _ = ready.send(true);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    pub fn set_enabled(enabled: bool) {
        let _g = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());
        let cur = HWND_RAW.load(Ordering::SeqCst);
        if enabled {
            if cur != 0 {
                return;
            }
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new().name("lucent-clipboard".into()).spawn(move || run_listener(tx)) {
                Ok(_) => {
                    if rx.recv_timeout(std::time::Duration::from_secs(5)) != Ok(true) {
                        eprintln!("lucent: clipboard listener did not start");
                    }
                }
                Err(e) => eprintln!("lucent: cannot spawn clipboard thread: {e}"),
            }
        } else if cur != 0 {
            // Detach first: updates still queued for the closing window are ignored.
            HWND_RAW.store(0, Ordering::SeqCst);
            if let Err(e) = unsafe { PostMessageW(Some(HWND(cur as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)) } {
                eprintln!("lucent: cannot close clipboard window: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_dedups_and_caps_at_50() {
        let mut r = Ring::new();
        for i in 0..60 {
            r.push(&format!("t{i}"), 100);
        }
        assert_eq!(r.items().len(), RING_CAP);
        assert_eq!(r.items()[0].text, "t59");
        assert_eq!(r.items()[49].text, "t10");
        r.push("t30", 200); // dedup: moved to the top, still one entry
        assert_eq!(r.items().len(), RING_CAP);
        assert_eq!(r.items()[0].text, "t30");
        assert_eq!(r.items().iter().filter(|i| i.text == "t30").count(), 1);
        assert_eq!(r.items()[0].at, 200);
        r.push("   ", 300);
        r.push("", 300);
        assert_eq!(r.items()[0].text, "t30");
        r.clear();
        assert!(r.items().is_empty());
    }

    fn f(name: &str, dword: Option<u32>) -> ClipFormat<'_> {
        ClipFormat { name, dword }
    }

    #[test]
    fn excluded_formats_are_skipped() {
        assert!(!is_excluded(&[]));
        assert!(!is_excluded(&[f("HTML Format", None)]));
        assert!(is_excluded(&[f("ExcludeClipboardContentFromMonitorProcessing", None)]));
        assert!(is_excluded(&[f("CanIncludeInClipboardHistory", Some(0))]));
        assert!(is_excluded(&[f("CanUploadToCloudClipboard", Some(0))]));
        assert!(!is_excluded(&[f("CanIncludeInClipboardHistory", Some(1))]));
        // Present but unreadable: excluded (fail closed).
        assert!(is_excluded(&[f("CanUploadToCloudClipboard", None)]));
        assert!(is_excluded(&[f("CanIncludeInClipboardHistory", None)]));
        assert!(!is_excluded(&[f("CanUploadToCloudClipboard", Some(1)), f("HTML Format", None)]));
        assert!(is_excluded(&[f("HTML Format", None), f("CanUploadToCloudClipboard", Some(0))]));
    }

    #[test]
    fn disabled_store_neither_records_nor_shows() {
        let mut s = Store::new();
        s.record("avant init", 1);
        assert!(s.snapshot().is_empty(), "disabled until the setting is applied");
        s.set_enabled(true);
        s.record("secret", 2);
        assert_eq!(s.snapshot().len(), 1);
        s.set_enabled(false);
        // A WM_CLIPBOARDUPDATE already past the window check records after the disable.
        s.record("late update", 3);
        assert!(s.snapshot().is_empty());
        s.set_enabled(true);
        assert!(s.snapshot().is_empty(), "the history was dropped, the late update never kept");
    }

    #[test]
    fn cb_prefix_lists_newest_first() {
        let mut r = Ring::new();
        r.push("premier", 1000);
        r.push("deuxième\nsuite", 1000);
        r.push("troisième", 1000 - 0);
        let es = entries("", r.items(), 1000 + 180);
        let titles: Vec<_> = es.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["troisième", "deuxième", "premier"]);
        assert!(es[0].subtitle.starts_with("il y a 3 min"));
        assert_eq!(es[0].kind, Kind::Clipboard);
        assert_eq!(es[1].keywords[0], "deuxième\nsuite");
        assert_eq!(entries("DEUX", r.items(), 1000).len(), 1);
        assert_eq!(cb_arg("cb"), Some(""));
        assert_eq!(cb_arg("CB "), Some(""));
        assert_eq!(cb_arg("cb  abc "), Some("abc"));
        assert_eq!(cb_arg("cbx"), None);
        assert_eq!(cb_arg("c"), None);
        assert_eq!(cb_arg("é"), None);
        assert_eq!(cb_arg("cé"), None);
    }

    #[test]
    fn writing_back_dedups_to_a_single_top_entry() {
        let mut r = Ring::new();
        r.push("a", 1);
        r.push("b", 2);
        r.push("a", 3); // what the listener sees after "put back"
        let t: Vec<_> = r.items().iter().map(|i| i.text.as_str()).collect();
        assert_eq!(t, ["a", "b"]);
    }

    #[test]
    fn relative_time_in_french() {
        assert_eq!(relative_time_fr(100, 100), "à l'instant");
        assert_eq!(relative_time_fr(159, 100), "à l'instant");
        assert_eq!(relative_time_fr(100 + 180, 100), "il y a 3 min");
        assert_eq!(relative_time_fr(100 + 2 * 3600 + 5, 100), "il y a 2 h");
        assert_eq!(relative_time_fr(100 + 30 * 3600, 100), "hier");
        assert_eq!(relative_time_fr(100 + 3 * 86_400, 100), "il y a 3 j");
        assert_eq!(relative_time_fr(0, 100), "à l'instant"); // clock skew never underflows
    }

    #[test]
    fn title_is_first_line_capped_at_80_chars() {
        assert_eq!(title_of("\n  bonjour \nsuite"), "bonjour");
        let long = "é".repeat(200);
        let t = title_of(&long);
        assert_eq!(t.chars().count(), 80);
        assert!(t.ends_with('…'));
        assert_eq!(title_of(&"a".repeat(80)).chars().count(), 80);
    }
}
