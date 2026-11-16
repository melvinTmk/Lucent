//! Files source: user folders on Windows and project folders of a running WSL distro.
//!
//! A background thread rebuilds the list (first pass ~10 s after launch, then every 30 min) at
//! below-normal priority and sleeps while a fullscreen app owns the foreground. WSL is only
//! read while its distro runs; otherwise the previous pass's WSL entries are kept in memory.
use super::{Entry, Kind, OpenMods, SearchIndex};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const CAP: usize = 200_000;
pub const WINDOWS_DEPTH: usize = 6;
pub const WSL_DEPTH: usize = 4;
pub const REBUILD_EVERY: Duration = Duration::from_secs(30 * 60);
const FIRST_DELAY: Duration = Duration::from_secs(10);
const FULLSCREEN_POLL: Duration = Duration::from_secs(5);
/// How long a fullscreen probe (monitor enumeration + foreground check) is reused.
const FULLSCREEN_TTL: Duration = Duration::from_secs(1);
/// The WSL walk re-checks that its distro still runs at least this often.
pub const WSL_RECHECK: Duration = Duration::from_secs(2);
const WSL_PREFIX: &str = r"\\wsl.localhost\";

pub const SKIP_DIRS: [&str; 10] =
    ["node_modules", ".git", "vendor", "target", "tmp", "log", "dist", "build", ".cache", "__pycache__"];

/// Pure filter of the walk. `skip_dirs` is compared case-insensitively.
pub struct Walker {
    pub skip_dirs: Vec<String>,
    /// Depth of the deepest entry indexed: 1 = direct children of the root.
    pub max_depth: usize,
    pub cap: usize,
}

impl Walker {
    pub fn new(max_depth: usize, cap: usize) -> Self {
        let mut skip_dirs: Vec<String> = SKIP_DIRS.iter().map(|s| s.to_string()).collect();
        skip_dirs.push("AppData".into());
        Self { skip_dirs, max_depth, cap }
    }

    pub fn should_enter(&self, name: &str) -> bool {
        !name.starts_with('.') && !self.skip_dirs.iter().any(|d| d.eq_ignore_ascii_case(name))
    }

    pub fn should_index(&self, name: &str) -> bool {
        !name.starts_with('.')
    }
}

/// Hidden or system entry (Windows attributes; always false elsewhere).
fn is_hidden(md: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        md.file_attributes() & (HIDDEN | SYSTEM) != 0
    }
    #[cfg(not(windows))]
    {
        let _ = md;
        false
    }
}

/// Appends the files and folders under `root` to `out` (breadth-first, so the cap keeps the
/// shallow, most useful entries) until `out` holds `w.cap` paths. `tick` runs before each
/// directory read (the indexer pauses there while a fullscreen app runs); when it returns
/// false the walk stops at once and `walk` returns false (aborted).
pub fn walk(w: &Walker, root: &Path, out: &mut Vec<PathBuf>, tick: &mut dyn FnMut() -> bool) -> bool {
    let mut level = vec![root.to_path_buf()];
    for depth in 1..=w.max_depth {
        let mut next = Vec::new();
        for dir in &level {
            if !tick() {
                return false;
            }
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            for e in rd.flatten() {
                if out.len() >= w.cap {
                    return true;
                }
                let name = e.file_name();
                let Some(name) = name.to_str() else { continue };
                let Ok(md) = e.metadata() else { continue };
                if !w.should_index(name) || is_hidden(&md) {
                    continue;
                }
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir && !w.should_enter(name) {
                    continue;
                }
                out.push(e.path());
                if is_dir && depth < w.max_depth {
                    next.push(e.path());
                }
            }
        }
        level = next;
    }
    true
}

/// Tick of the WSL walk: waits out fullscreen apps, then re-checks that the distro still runs
/// after any wait and otherwise at most every `WSL_RECHECK` (a stopped distro must never be
/// read: `\\wsl.localhost` would boot the VM). False = stop the walk.
pub struct WslGate<'a> {
    /// Blocks while a fullscreen app runs; true when it waited.
    pub wait: &'a dyn Fn() -> bool,
    pub running: &'a dyn Fn() -> bool,
    pub now: &'a dyn Fn() -> Instant,
    /// Time of the last successful running check (`None` forces one on the next tick).
    pub last: Option<Instant>,
}

impl WslGate<'_> {
    pub fn tick(&mut self) -> bool {
        let waited = (self.wait)();
        let now = (self.now)();
        if waited || self.last.is_none_or(|t| now.saturating_duration_since(t) >= WSL_RECHECK) {
            if !(self.running)() {
                return false;
            }
            self.last = Some(now);
        }
        true
    }
}

/// Walks every WSL project root through `gate`, re-checking the distro before each project.
/// `None` when the distro stopped meanwhile (the caller keeps its previous WSL entries).
pub fn walk_projects(w: &Walker, roots: &[PathBuf], gate: &mut WslGate) -> Option<Vec<PathBuf>> {
    let mut out = Vec::new();
    for root in roots {
        gate.last = None;
        if !walk(w, root, &mut out, &mut || gate.tick()) {
            return None;
        }
    }
    Some(out)
}

/// `\\wsl.localhost\<distro>\a\b` → `(distro, "/a/b")`.
pub fn wsl_to_linux(p: &str) -> Option<(String, String)> {
    let rest = p.get(..WSL_PREFIX.len()).filter(|h| h.eq_ignore_ascii_case(WSL_PREFIX)).map(|_| &p[WSL_PREFIX.len()..])?;
    let (distro, tail) = rest.split_once('\\').unwrap_or((rest, ""));
    if distro.is_empty() {
        return None;
    }
    Some((distro.to_string(), format!("/{}", tail.replace('\\', "/"))))
}

/// Parent folder with the home directory replaced by `~`.
fn shorten(parent: &str, home: &str) -> String {
    let home = home.trim_end_matches(['\\', '/']);
    if !home.is_empty() && parent.len() >= home.len() && parent.is_char_boundary(home.len()) {
        let (head, tail) = parent.split_at(home.len());
        if head.eq_ignore_ascii_case(home) && (tail.is_empty() || tail.starts_with(['\\', '/'])) {
            return format!("~{tail}");
        }
    }
    parent.to_string()
}

/// Paths → entries. `win_home`: Windows home; `wsl_home`: `(distro, linux home)`.
pub fn build_entries(paths: &[PathBuf], win_home: &str, wsl_home: &(String, String)) -> Vec<Entry> {
    paths
        .iter()
        .filter_map(|p| {
            let full = p.to_string_lossy().into_owned();
            let title = p.file_name()?.to_string_lossy().into_owned();
            let parent = p.parent().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            let subtitle = match wsl_to_linux(&parent) {
                Some((d, lin)) if d.eq_ignore_ascii_case(&wsl_home.0) => shorten(&lin, &wsl_home.1),
                Some((_, lin)) => lin,
                None => shorten(&parent, win_home),
            };
            Some(Entry { id: format!("file:{full}"), kind: Kind::File, title, subtitle, icon: None, keywords: vec![], valo: None })
        })
        .collect()
}

/// Windows known folders worth indexing.
#[cfg(windows)]
fn known_folders() -> Vec<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Videos,
        KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };
    [&FOLDERID_Desktop, &FOLDERID_Documents, &FOLDERID_Downloads, &FOLDERID_Pictures, &FOLDERID_Videos, &FOLDERID_Music]
        .iter()
        .filter_map(|id| unsafe {
            let p = SHGetKnownFolderPath(*id, KF_FLAG_DEFAULT, None).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s.map(PathBuf::from)
        })
        .collect()
}

#[cfg(not(windows))]
fn known_folders() -> Vec<PathBuf> {
    Vec::new()
}

/// A fullscreen app (game, video) owns the foreground.
#[cfg(windows)]
fn fullscreen_active() -> bool {
    let Some(r) = crate::monitors::fullscreen_foreground_rect() else { return false };
    crate::monitors::win::enumerate().iter().any(|m| r.covers(&m.rect))
}

#[cfg(not(windows))]
fn fullscreen_active() -> bool {
    false
}

/// `compute()` unless `slot` holds a value younger than `ttl`.
fn cached(slot: &mut Option<(Instant, bool)>, now: Instant, ttl: Duration, compute: impl FnOnce() -> bool) -> bool {
    if let Some((at, v)) = *slot {
        if now.saturating_duration_since(at) < ttl {
            return v;
        }
    }
    let v = compute();
    *slot = Some((now, v));
    v
}

/// `fullscreen_active`, probed at most once per `FULLSCREEN_TTL` (the walk asks per directory).
fn fullscreen_active_cached() -> bool {
    static LAST: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut g = LAST.lock().unwrap_or_else(|e| e.into_inner());
    cached(&mut g, Instant::now(), FULLSCREEN_TTL, fullscreen_active)
}

/// Indexing must not compete with a game: paused while a fullscreen app owns the
/// foreground or a Game Mode session runs (windowed games included).
fn should_pause() -> bool {
    crate::game::service::is_active() || fullscreen_active_cached()
}

/// Sleeps while `should_pause`. True when it waited.
fn wait_while_fullscreen() -> bool {
    let mut waited = false;
    while should_pause() {
        waited = true;
        std::thread::sleep(FULLSCREEN_POLL);
    }
    waited
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Clears `RUNNING` when dropped, even if the pass panics.
struct RunGuard;

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// One full pass. Windows folders first, then WSL (cap shared). `prev_wsl` keeps the WSL paths so
/// they survive a stopped distro. Blocking.
fn pass(index: &SearchIndex, settings: &crate::settings::Settings, prev_wsl: &mut Vec<PathBuf>) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _guard = RunGuard;
    let mut paths: Vec<PathBuf> = Vec::new();
    for root in known_folders() {
        walk(&Walker::new(WINDOWS_DEPTH, CAP), &root, &mut paths, &mut || {
            wait_while_fullscreen();
            true
        });
    }
    let cache = super::projects::current();
    let distro = {
        let d = settings.search.wsl_distro.trim();
        if !d.is_empty() {
            d.to_string()
        } else {
            super::wsl::default_distro().or_else(|| cache.as_ref().map(|c| c.distro.clone())).unwrap_or_default()
        }
    };
    if let Some(c) = cache.as_ref().filter(|c| !distro.is_empty() && c.distro.eq_ignore_ascii_case(&distro)) {
        if super::wsl::is_running(&distro) {
            let w = Walker::new(WSL_DEPTH, CAP.saturating_sub(paths.len()));
            let roots: Vec<PathBuf> = c.projects.iter().map(|p| super::wsl::unc_path(&distro, p)).collect();
            let running = || super::wsl::is_running(&distro);
            let mut gate =
                WslGate { wait: &wait_while_fullscreen, running: &running, now: &Instant::now, last: Some(Instant::now()) };
            match walk_projects(&w, &roots, &mut gate) {
                Some(wsl) => *prev_wsl = wsl,
                None => eprintln!("lucent: WSL distro {distro} stopped during indexing, keeping the previous WSL files"),
            }
        }
    } else {
        prev_wsl.clear();
    }
    let room = CAP.saturating_sub(paths.len());
    paths.extend(prev_wsl.iter().take(room).cloned());
    let win_home = dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default();
    let wsl_home = match cache {
        Some(c) if !c.user.is_empty() => (c.distro.clone(), if c.user == "root" { "/root".into() } else { format!("/home/{}", c.user) }),
        _ => (String::new(), String::new()),
    };
    index.set_source(Kind::File, build_entries(&paths, &win_home, &wsl_home));
}

fn lower_priority() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

/// Registers the opener and starts the background indexer (call once at startup).
pub fn start(index: &SearchIndex, settings: crate::settings::Settings) {
    super::register_opener(Kind::File, open);
    let index = index.clone();
    let spawned = std::thread::Builder::new().name("lucent-files".into()).spawn(move || {
        lower_priority();
        std::thread::sleep(FIRST_DELAY);
        let mut prev_wsl = Vec::new();
        loop {
            wait_while_fullscreen();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pass(&index, &settings, &mut prev_wsl)));
            if r.is_err() {
                eprintln!("lucent: file indexer pass panicked");
            }
            std::thread::sleep(REBUILD_EVERY);
        }
    });
    if let Err(e) = spawned {
        eprintln!("lucent: cannot spawn file indexer: {e}");
    }
}

/// Opens the file (default app); Ctrl+Enter reveals it in Explorer. WSL files open in VS Code
/// Remote; revealing one needs the distro to run (never starts WSL).
pub fn open(entry: &Entry, mods: OpenMods) {
    let Some(path) = entry.id.strip_prefix("file:") else { return };
    #[cfg(windows)]
    {
        let wsl = wsl_to_linux(path);
        if mods.ctrl {
            if let Some((d, _)) = &wsl {
                if !super::wsl::is_running(d) {
                    eprintln!("lucent: WSL distro {d} is stopped, not revealing {} (would start it)", entry.title);
                    return;
                }
            }
            reveal(path);
            return;
        }
        match wsl {
            Some((d, linux)) => super::projects::launch_code(&d, &linux, &entry.title),
            None => shell_open(path, &entry.title),
        }
    }
    #[cfg(not(windows))]
    let _ = (path, mods);
}

/// `explorer.exe /select,"<path>"`: explorer wants ONE argument and is not a shell, so the
/// raw argument is safe (a Windows path cannot hold a double quote).
#[cfg(windows)]
fn reveal(path: &str) {
    use std::os::windows::process::CommandExt;
    if path.contains('"') {
        return;
    }
    if let Err(e) = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{path}\"")).spawn() {
        eprintln!("lucent: cannot reveal the file in Explorer: {e}");
    }
}

#[cfg(windows)]
fn shell_open(path: &str, title: &str) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};
    let target: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let r = unsafe { ShellExecuteW(None, w!("open"), PCWSTR(target.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    if r.0 as isize <= 32 {
        eprintln!("lucent: cannot open {title} (ShellExecute {})", r.0 as isize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lucent-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn walker(max_depth: usize, cap: usize) -> Walker {
        Walker { skip_dirs: ["node_modules", ".git", "AppData"].iter().map(|s| s.to_string()).collect(), max_depth, cap }
    }

    #[test]
    fn walker_skips_heavy_dirs_and_caps() {
        let d = tree("cap");
        std::fs::create_dir_all(d.join("node_modules")).unwrap();
        for i in 0..50 {
            std::fs::write(d.join("node_modules").join(format!("n{i}.js")), "x").unwrap();
        }
        for i in 0..50 {
            std::fs::write(d.join(format!("f{i}.txt")), "x").unwrap();
        }
        let mut out = Vec::new();
        walk(&walker(6, 10), &d, &mut out, &mut || true);
        assert_eq!(out.len(), 10);
        assert!(out.iter().all(|p| !p.to_string_lossy().contains("node_modules")));
        let mut all = Vec::new();
        walk(&walker(6, 1000), &d, &mut all, &mut || true);
        assert_eq!(all.len(), 50);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn respects_max_depth() {
        let d = tree("depth");
        let deep = d.join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(d.join("a").join("one.txt"), "x").unwrap();
        std::fs::write(d.join("a").join("b").join("two.txt"), "x").unwrap();
        std::fs::write(deep.join("three.txt"), "x").unwrap();
        let names = |depth| {
            let mut out = Vec::new();
            walk(&walker(depth, 100), &d, &mut out, &mut || true);
            let mut n: Vec<String> = out.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
            n.sort();
            n
        };
        // Depth 1 = direct children of the root.
        assert_eq!(names(1), ["a"]);
        assert_eq!(names(2), ["a", "b", "one.txt"]);
        assert_eq!(names(4), ["a", "b", "c", "one.txt", "three.txt", "two.txt"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn walk_stops_when_tick_says_so() {
        let d = tree("abort");
        for i in 0..3 {
            std::fs::create_dir_all(d.join(format!("d{i}"))).unwrap();
            std::fs::write(d.join(format!("d{i}")).join("f.txt"), "x").unwrap();
        }
        let mut out = Vec::new();
        let mut calls = 0;
        // First tick (root) passes, the second (first sub-directory) aborts.
        let done = walk(&walker(6, 100), &d, &mut out, &mut || {
            calls += 1;
            calls < 2
        });
        assert!(!done);
        assert_eq!(calls, 2);
        assert_eq!(out.len(), 3, "only the root listing: {out:?}");
        let mut all = Vec::new();
        assert!(walk(&walker(6, 100), &d, &mut all, &mut || true));
        assert_eq!(all.len(), 6);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn wsl_gate_rechecks_after_a_wait_and_every_two_seconds() {
        use std::cell::Cell;
        let t0 = Instant::now();
        let clock = Cell::new(t0);
        let waits = Cell::new(false);
        let checks = Cell::new(0u32);
        let up = Cell::new(true);
        let wait = || waits.replace(false);
        let running = || {
            checks.set(checks.get() + 1);
            up.get()
        };
        let now = || clock.get();
        let mut g = WslGate { wait: &wait, running: &running, now: &now, last: Some(t0) };
        assert!(g.tick());
        assert_eq!(checks.get(), 0, "checked just before the walk");
        clock.set(t0 + Duration::from_millis(1500));
        assert!(g.tick());
        assert_eq!(checks.get(), 0);
        clock.set(t0 + WSL_RECHECK);
        assert!(g.tick());
        assert_eq!(checks.get(), 1);
        // A fullscreen wait (a whole game) forces a check even inside the 2 s window.
        waits.set(true);
        up.set(false);
        assert!(!g.tick());
        assert_eq!(checks.get(), 2);
    }

    #[test]
    fn stopped_distro_aborts_the_wsl_walk() {
        let d = tree("wslabort");
        let (p1, p2) = (d.join("p1"), d.join("p2"));
        std::fs::create_dir_all(p1.join("src")).unwrap();
        std::fs::create_dir_all(&p2).unwrap();
        std::fs::write(p2.join("x.rb"), "x").unwrap();
        let t0 = Instant::now();
        let no_wait = || false;
        let now = || t0;
        let checks = std::cell::Cell::new(0u32);
        // Running for the first project, stopped when the second one starts.
        let running = || {
            checks.set(checks.get() + 1);
            checks.get() < 2
        };
        let mut g = WslGate { wait: &no_wait, running: &running, now: &now, last: Some(t0) };
        assert_eq!(walk_projects(&walker(4, 100), &[p1.clone(), p2.clone()], &mut g), None);
        assert_eq!(checks.get(), 2, "re-checked between projects");
        let up = || true;
        let mut g = WslGate { wait: &no_wait, running: &up, now: &now, last: Some(t0) };
        assert_eq!(walk_projects(&walker(4, 100), &[p1, p2], &mut g).map(|v| v.len()), Some(2));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn fullscreen_probe_is_cached_for_its_ttl() {
        let t0 = Instant::now();
        let mut slot = None;
        let mut probes = 0;
        let mut probe = |v: bool| {
            probes += 1;
            v
        };
        assert!(cached(&mut slot, t0, FULLSCREEN_TTL, || probe(true)));
        assert!(cached(&mut slot, t0 + Duration::from_millis(900), FULLSCREEN_TTL, || probe(false)));
        assert!(!cached(&mut slot, t0 + FULLSCREEN_TTL, FULLSCREEN_TTL, || probe(false)));
        assert_eq!(probes, 2);
    }

    #[test]
    fn wsl_path_maps_to_linux_path() {
        assert_eq!(
            wsl_to_linux(r"\\wsl.localhost\Ubuntu\home\u\a.rb"),
            Some(("Ubuntu".into(), "/home/u/a.rb".into()))
        );
        assert_eq!(
            crate::search::wsl::unc_path("Ubuntu", "/home/u/a.rb").to_string_lossy(),
            r"\\wsl.localhost\Ubuntu\home\u\a.rb"
        );
        assert_eq!(wsl_to_linux(r"C:\Users\a.txt"), None);
        assert_eq!(wsl_to_linux(r"\\wsl.localhost\Ubuntu"), Some(("Ubuntu".into(), "/".into())));
    }

    #[test]
    fn filters_and_subtitles() {
        let w = walker(6, 10);
        assert!(!w.should_enter("node_modules") && !w.should_enter("NODE_MODULES") && !w.should_enter(".hidden"));
        assert!(w.should_enter("src"));
        assert!(w.should_index("a.txt") && !w.should_index(".env"));
        assert_eq!(shorten(r"C:\Users\me\Documents\x", r"C:\Users\me"), r"~\Documents\x");
        assert_eq!(shorten(r"D:\data", r"C:\Users\me"), r"D:\data");
        assert_eq!(shorten("/home/u/proj", "/home/u"), "~/proj");
    }

    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_windows_pass_measure() {
        let t = std::time::Instant::now();
        let mut paths = Vec::new();
        for root in known_folders() {
            walk(&Walker::new(WINDOWS_DEPTH, CAP), &root, &mut paths, &mut || true);
        }
        println!("files indexed: {} in {:?}", paths.len(), t.elapsed());
        assert!(!paths.is_empty());
    }
}
