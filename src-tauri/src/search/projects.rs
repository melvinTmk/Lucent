//! Projects source: git projects inside WSL, opened in VS Code Remote.
//!
//! Lucent never starts the VM: a stopped distro is served from `projects.json` and the
//! filesystem (`\\wsl.localhost`) is only read while the distro runs (see `collect`).
use super::wsl::{RealWsl, Wsl};
use super::{Entry, Kind, OpenMods, SearchIndex};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The search window refreshes the list when it is older than this.
pub const MAX_AGE: Duration = Duration::from_secs(2 * 60);

/// Listing of Linux paths. The real one reads `\\wsl.localhost\...`, so it is only ever
/// used while the distro is running.
pub trait Fs {
    /// Names of the sub-directories of `path` (empty when unreadable).
    fn list_dirs(&self, path: &str) -> Vec<String>;
    fn exists(&self, path: &str) -> bool;
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Cache {
    pub distro: String,
    pub user: String,
    pub roots: Vec<String>,
    pub projects: Vec<String>,
    /// Unix seconds.
    pub scanned_at: u64,
}

fn home_of(user: &str) -> String {
    if user == "root" { "/root".into() } else { format!("/home/{user}") }
}

/// `~`, `~/x` and absolute Linux paths, without trailing slash.
pub fn expand_tilde(root: &str, home: &str) -> String {
    let root = root.trim();
    let full = if root == "~" {
        home.to_string()
    } else if let Some(rest) = root.strip_prefix("~/") {
        format!("{}/{}", home.trim_end_matches('/'), rest)
    } else {
        root.to_string()
    };
    let t = full.trim_end_matches('/');
    if t.is_empty() { "/".into() } else { t.to_string() }
}

/// `/home/u/workspace` → `~/workspace`.
fn tildify(path: &str, home: &str) -> String {
    if path == home {
        "~".into()
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
        format!("~/{rest}")
    } else {
        path.to_string()
    }
}

fn visible_dirs(fs: &dyn Fs, path: &str) -> Vec<String> {
    let mut v: Vec<String> = fs.list_dirs(path).into_iter().filter(|n| !n.starts_with('.')).collect();
    v.sort();
    v
}

fn join(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

/// `(roots, projects)`. Every visible sub-folder of a root is a project. Roots are the
/// configured ones, or (auto-detect) every visible folder of `home` holding at least one
/// child with a `.git` entry.
pub fn scan(fs: &dyn Fs, home: &str, configured: &[String]) -> (Vec<String>, Vec<String>) {
    let mut roots = Vec::new();
    let mut projects = Vec::new();
    let configured: Vec<&String> = configured.iter().filter(|r| !r.trim().is_empty()).collect();
    if !configured.is_empty() {
        for r in configured {
            let root = expand_tilde(r, home);
            for name in visible_dirs(fs, &root) {
                projects.push(join(&root, &name));
            }
            roots.push(root);
        }
    } else {
        for top in visible_dirs(fs, home) {
            let root = join(home, &top);
            let children: Vec<String> = visible_dirs(fs, &root).into_iter().map(|c| join(&root, &c)).collect();
            // The .git rule only detects roots; once detected, every visible child is a project.
            if children.iter().any(|c| fs.exists(&join(c, ".git"))) {
                roots.push(root);
                projects.extend(children);
            }
        }
    }
    (roots, projects)
}

/// New cache for the current state of WSL. A stopped distro (or no distro) returns the cache
/// without calling `fs` or `home_user`.
pub fn collect(wsl: &dyn Wsl, fs: &dyn Fs, distro_setting: &str, roots: &[String], cache: Cache, now: u64) -> Cache {
    let distro = if !distro_setting.trim().is_empty() {
        distro_setting.trim().to_string()
    } else {
        wsl.default_distro().unwrap_or_else(|| cache.distro.clone())
    };
    let same = cache.distro.eq_ignore_ascii_case(&distro);
    if distro.is_empty() || !wsl.is_running(&distro) {
        return if same { cache } else { Cache { distro, ..Cache::default() } };
    }
    let user = wsl.home_user(&distro).or_else(|| same.then(|| cache.user.clone()).filter(|u| !u.is_empty()));
    let Some(user) = user else {
        return if same { cache } else { Cache { distro, ..Cache::default() } };
    };
    let (roots, projects) = scan(fs, &home_of(&user), roots);
    // Empty result while we know some: share not ready or listing error, keep what we had.
    if projects.is_empty() && same && !cache.projects.is_empty() {
        return cache;
    }
    Cache { distro, user, roots, projects, scanned_at: now }
}

pub fn build_entries(cache: &Cache, icon: Option<String>) -> Vec<Entry> {
    let home = home_of(&cache.user);
    cache
        .projects
        .iter()
        .map(|p| {
            let (root, name) = p.rsplit_once('/').unwrap_or(("", p));
            let root_name = root.rsplit('/').next().unwrap_or("").to_string();
            Entry {
                id: format!("proj:{p}"),
                kind: Kind::Project,
                title: name.to_string(),
                subtitle: tildify(root, &home),
                icon: icon.clone(),
                keywords: if root_name.is_empty() { vec![] } else { vec![root_name] },
                valo: None,
            }
        })
        .collect()
}

/// Characters cmd.exe would interpret (or that make an argument unsafe for it).
fn has_cmd_metachar(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '&' | '|' | '<' | '>' | '^' | '%' | '(' | ')' | '"' | '!') || c.is_control())
}

/// Fallback launch through `cmd.exe /c code`. `None` (refused) when an argument holds a cmd
/// metacharacter: with a batch shim in between there is no reliable escaping.
pub fn open_command_line(distro: &str, path: &str) -> Option<Vec<String>> {
    if has_cmd_metachar(distro) || has_cmd_metachar(path) {
        return None;
    }
    let mut v: Vec<String> = ["cmd.exe", "/c", "code", "--remote"].iter().map(|s| s.to_string()).collect();
    v.push(format!("wsl+{distro}"));
    v.push(path.to_string());
    Some(v)
}

/// Arguments of `Code.exe` (with `ELECTRON_RUN_AS_NODE=1`) that do what `code.cmd` does.
/// No shell is involved, so nothing needs escaping.
pub fn direct_args(cli_js: &str, distro: &str, path: &str) -> Vec<String> {
    vec![cli_js.into(), "--remote".into(), format!("wsl+{distro}"), path.into()]
}

/// The `<rel>\cli.js` part of the `"%~dp0..\<rel>\cli.js"` argument of `bin\code.cmd`.
pub fn parse_cli_rel(shim: &str) -> Option<String> {
    const MARK: &str = "%~dp0..\\";
    shim.lines().filter(|l| l.contains("Code.exe")).find_map(|line| {
        line.split(MARK).skip(1).find_map(|part| {
            let rel = &part[..part.find('"')?];
            rel.to_ascii_lowercase().ends_with("cli.js").then(|| rel.to_string())
        })
    })
}

fn cache_path() -> std::path::PathBuf {
    crate::paths::config_dir().join("projects.json")
}

fn load_cache() -> Cache {
    std::fs::read_to_string(cache_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_cache(c: &Cache) {
    match serde_json::to_string_pretty(c) {
        Ok(s) => {
            let path = cache_path();
            let tmp = path.with_extension("json.tmp");
            if let Err(e) = std::fs::write(&tmp, s).and_then(|_| std::fs::rename(&tmp, &path)) {
                eprintln!("lucent: cannot save projects.json: {e}");
            }
        }
        Err(e) => eprintln!("lucent: cannot serialise projects cache: {e}"),
    }
}

/// Latest cache, for the opener (distro name).
static CURRENT: Mutex<Option<Cache>> = Mutex::new(None);

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Opens the project in VS Code Remote; Ctrl+Enter opens the folder in Explorer, only when
/// the distro already runs (never starts WSL). Shift is ignored.
pub fn open(entry: &Entry, mods: OpenMods) {
    let Some(path) = entry.id.strip_prefix("proj:") else { return };
    let distro = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|c| c.distro.clone()).unwrap_or_default();
    if distro.is_empty() {
        eprintln!("lucent: no WSL distro known, cannot open {}", entry.title);
        return;
    }
    #[cfg(windows)]
    {
        if mods.ctrl {
            if !super::wsl::is_running(&distro) {
                eprintln!("lucent: WSL distro {distro} is stopped, not opening the folder (would start it)");
                return;
            }
            use std::os::windows::process::CommandExt;
            let unc = super::wsl::unc_path(&distro, path);
            let Some(arg) = explorer_folder_arg(&unc.to_string_lossy()) else {
                eprintln!("lucent: refusing to open {path} in Explorer: double quote in the path");
                return;
            };
            if let Err(e) = std::process::Command::new("explorer.exe").raw_arg(arg).spawn() {
                eprintln!("lucent: cannot open the folder in Explorer: {e}");
            }
            return;
        }
        launch_code(&distro, path, &entry.title);
    }
    #[cfg(not(windows))]
    let _ = (path, mods, distro);
}

/// Explorer splits an unquoted argument on commas: the folder goes as ONE quoted raw token.
/// `None` for a path holding `"` (Linux allows it; it cannot be quoted for explorer).
fn explorer_folder_arg(path: &str) -> Option<String> {
    (!path.contains('"')).then(|| format!("\"{path}\""))
}

/// Opens a Linux path (folder or file) in VS Code Remote WSL. No shell sees the path when
/// the direct CLI is found; the cmd.exe fallback refuses unsafe characters.
#[cfg(windows)]
pub(super) fn launch_code(distro: &str, path: &str, title: &str) {
    use std::os::windows::process::CommandExt;
    let mut cmd = match code_install() {
        // What bin\code.cmd does, without a shell: no parsing of the path by anyone.
        Some(CodeInstall { exe, cli_js: Some(cli) }) => {
            let mut c = std::process::Command::new(exe);
            c.args(direct_args(&cli, distro, path)).env("ELECTRON_RUN_AS_NODE", "1").env_remove("VSCODE_DEV");
            c
        }
        found => {
            if found.is_none() {
                eprintln!("lucent: VS Code not found (where.exe code), trying `cmd /c code` for {title}");
            }
            let Some(argv) = open_command_line(distro, path) else {
                eprintln!("lucent: refusing to open {path}: unsafe characters and no direct VS Code CLI found");
                return;
            };
            let mut c = std::process::Command::new(&argv[0]);
            c.args(&argv[1..]);
            c
        }
    };
    cmd.creation_flags(super::wsl::win::CREATE_NO_WINDOW);
    if let Err(e) = cmd.spawn() {
        eprintln!("lucent: cannot start VS Code for {title}: {e}");
    }
}

/// Latest projects cache (distro, user, project paths), if one was published.
pub fn current() -> Option<Cache> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[derive(Clone, Debug, PartialEq)]
struct CodeInstall {
    exe: String,
    /// Absolute cli.js when `code.cmd` could be parsed.
    cli_js: Option<String>,
}

/// True when the cached VS Code install must be looked up again: nothing cached (not
/// installed yet, stale PATH) or a cached file vanished (each VS Code update deletes the
/// per-commit folder holding cli.js).
fn needs_refresh(cached: Option<&CodeInstall>, exists: impl Fn(&str) -> bool) -> bool {
    match cached {
        None => true,
        Some(c) => !exists(&c.exe) || c.cli_js.as_deref().is_some_and(|cli| !exists(cli)),
    }
}

static CODE: Mutex<Option<CodeInstall>> = Mutex::new(None);

/// VS Code found through `where.exe code`; re-resolved whenever `needs_refresh` says so.
fn code_install() -> Option<CodeInstall> {
    let mut g = CODE.lock().unwrap_or_else(|e| e.into_inner());
    if needs_refresh(g.as_ref(), |p| std::path::Path::new(p).exists()) {
        if let Some(old) = g.as_ref() {
            eprintln!("lucent: VS Code install changed ({:?} missing), looking it up again", old.cli_js.as_deref().unwrap_or(&old.exe));
        }
        *g = find_code();
    }
    g.clone()
}

fn code_exe() -> Option<String> {
    code_install().map(|c| c.exe)
}

#[cfg(windows)]
fn find_code() -> Option<CodeInstall> {
    let out = super::wsl::win::run_hidden("where.exe", &["code"])?;
    let text = String::from_utf8_lossy(&out).into_owned();
    // `where` also lists the extensionless sh script; only the .cmd shim is parsed.
    text.lines().map(str::trim).filter(|l| l.to_ascii_lowercase().ends_with(".cmd")).find_map(|shim| {
        let exe = code_exe_from_shim(shim).filter(|p| std::path::Path::new(p).exists())?;
        let root = exe.rsplit_once('\\')?.0.to_string();
        let cli_js = std::fs::read_to_string(shim)
            .ok()
            .and_then(|c| parse_cli_rel(&c))
            .map(|rel| format!("{root}\\{rel}"))
            .filter(|p| std::path::Path::new(p).exists());
        Some(CodeInstall { exe, cli_js })
    })
}

#[cfg(not(windows))]
fn find_code() -> Option<CodeInstall> {
    None
}

/// `...\Microsoft VS Code\bin\code.cmd` → `...\Microsoft VS Code\Code.exe`.
fn code_exe_from_shim(shim: &str) -> Option<String> {
    let (bin, _) = shim.rsplit_once('\\')?;
    let (root, last) = bin.rsplit_once('\\')?;
    last.eq_ignore_ascii_case("bin").then(|| format!("{root}\\Code.exe"))
}

/// Registers the opener (call once at startup).
pub fn init() {
    super::register_opener(Kind::Project, open);
}

static LAST: Mutex<Option<Instant>> = Mutex::new(None);
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn is_stale(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.duration_since(t) >= MAX_AGE)
}

#[cfg(windows)]
struct WinFs {
    distro: String,
}

#[cfg(windows)]
impl Fs for WinFs {
    fn list_dirs(&self, path: &str) -> Vec<String> {
        std::fs::read_dir(super::wsl::unc_path(&self.distro, path))
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default()
    }
    fn exists(&self, path: &str) -> bool {
        super::wsl::unc_path(&self.distro, path).exists()
    }
}

#[cfg(not(windows))]
struct NoFs;
#[cfg(not(windows))]
impl Fs for NoFs {
    fn list_dirs(&self, _: &str) -> Vec<String> {
        Vec::new()
    }
    fn exists(&self, _: &str) -> bool {
        false
    }
}

fn publish(index: &SearchIndex, cache: &Cache) {
    let icon = code_exe().map(|p| crate::icons::icon_url(&crate::settings::AppId::Exe(p)));
    index.set_source(Kind::Project, build_entries(cache, icon));
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(cache.clone());
}

/// Publishes the cache at once, then rescans if the distro runs. Blocking: run off the UI threads.
pub fn refresh(index: &SearchIndex, settings: &crate::settings::Settings) {
    let cache = load_cache();
    publish(index, &cache);
    let s = &settings.search;
    #[cfg(windows)]
    let fs: Box<dyn Fs> = {
        // The listing needs the distro name; resolve it like `collect` does (registry only).
        let d = if s.wsl_distro.trim().is_empty() {
            super::wsl::default_distro().unwrap_or_else(|| cache.distro.clone())
        } else {
            s.wsl_distro.trim().to_string()
        };
        Box::new(WinFs { distro: d })
    };
    #[cfg(not(windows))]
    let fs: Box<dyn Fs> = Box::new(NoFs);
    let fresh = collect(&RealWsl, fs.as_ref(), &s.wsl_distro, &s.project_roots, cache.clone(), now_secs());
    let same_content = |a: &Cache, b: &Cache| {
        (&a.distro, &a.user, &a.roots, &a.projects) == (&b.distro, &b.user, &b.roots, &b.projects)
    };
    if !same_content(&fresh, &cache) {
        save_cache(&fresh);
        publish(index, &fresh);
    }
}

/// Cheap: spawns a background refresh when the list is missing or older than `MAX_AGE`.
pub fn refresh_if_stale(index: &SearchIndex, settings: crate::settings::Settings) {
    use std::sync::atomic::Ordering;
    let last = *LAST.lock().unwrap_or_else(|e| e.into_inner());
    if !is_stale(last, Instant::now()) || RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    // Stamped before the scan: a stopped distro is not re-probed on every reveal.
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    let index = index.clone();
    let spawned = std::thread::Builder::new().name("lucent-projects".into()).spawn(move || {
        lower_priority();
        refresh(&index, &settings);
        RUNNING.store(false, Ordering::SeqCst);
    });
    if let Err(e) = spawned {
        RUNNING.store(false, Ordering::SeqCst);
        eprintln!("lucent: cannot spawn projects refresh: {e}");
    }
}

fn lower_priority() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    struct MapFs {
        dirs: HashMap<String, Vec<String>>,
        exist: HashSet<String>,
    }
    impl MapFs {
        fn new(dirs: &[(&str, &[&str])], exist: &[&str]) -> Self {
            Self {
                dirs: dirs.iter().map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect())).collect(),
                exist: exist.iter().map(|s| s.to_string()).collect(),
            }
        }
    }
    impl Fs for MapFs {
        fn list_dirs(&self, p: &str) -> Vec<String> {
            self.dirs.get(p).cloned().unwrap_or_default()
        }
        fn exists(&self, p: &str) -> bool {
            self.exist.contains(p)
        }
    }

    struct PanicFs;
    impl Fs for PanicFs {
        fn list_dirs(&self, p: &str) -> Vec<String> {
            panic!("touched fs: list_dirs {p}")
        }
        fn exists(&self, p: &str) -> bool {
            panic!("touched fs: exists {p}")
        }
    }

    struct FakeWsl {
        running: bool,
    }
    impl Wsl for FakeWsl {
        fn default_distro(&self) -> Option<String> {
            Some("Ubuntu".into())
        }
        fn is_running(&self, _: &str) -> bool {
            self.running
        }
        fn home_user(&self, _: &str) -> Option<String> {
            assert!(self.running, "home_user on a stopped distro");
            Some("u".into())
        }
    }

    fn cache() -> Cache {
        Cache {
            distro: "Ubuntu".into(),
            user: "u".into(),
            roots: vec!["/home/u/workspace".into()],
            projects: vec!["/home/u/workspace/myapp".into(), "/home/u/workspace/lucent".into()],
            scanned_at: 5,
        }
    }

    #[test]
    fn stopped_distro_uses_cache_without_touching_fs() {
        let out = collect(&FakeWsl { running: false }, &PanicFs, "", &[], cache(), 100);
        assert_eq!(out, cache());
        let es = build_entries(&out, None);
        assert_eq!(es.len(), 2);
        assert_eq!(es[0].id, "proj:/home/u/workspace/myapp");
        assert_eq!(es[0].title, "myapp");
        assert_eq!(es[0].subtitle, "~/workspace");
        assert!(es[0].keywords.contains(&"workspace".to_string()));
    }

    #[test]
    fn auto_roots_need_a_git_child() {
        let fs = MapFs::new(
            &[
                ("/home/u", &["workspace", "docs", ".cache"]),
                ("/home/u/workspace", &["myapp", "notes"]),
                ("/home/u/docs", &["a"]),
                ("/home/u/.cache", &["x"]),
            ],
            &["/home/u/workspace/myapp/.git", "/home/u/.cache/x/.git"],
        );
        let (roots, projects) = scan(&fs, "/home/u", &[]);
        assert_eq!(roots, ["/home/u/workspace"]);
        assert_eq!(projects, ["/home/u/workspace/myapp", "/home/u/workspace/notes"]);
    }

    #[test]
    fn tilde_roots_expand_to_home() {
        assert_eq!(expand_tilde("~/x", "/home/u"), "/home/u/x");
        assert_eq!(expand_tilde("~", "/home/u"), "/home/u");
        assert_eq!(expand_tilde("/home/u/y/", "/home/u"), "/home/u/y");
        let fs = MapFs::new(&[("/home/u/x", &["p1", ".hidden", "p2"])], &[]);
        let (roots, projects) = scan(&fs, "/home/u", &["~/x".into()]);
        assert_eq!(roots, ["/home/u/x"]);
        assert_eq!(projects, ["/home/u/x/p1", "/home/u/x/p2"]);
    }

    #[test]
    fn running_distro_is_scanned_and_stamped() {
        let fs = MapFs::new(&[("/home/u/x", &["p"])], &[]);
        let out = collect(&FakeWsl { running: true }, &fs, "", &["~/x".into()], Cache::default(), 42);
        assert_eq!(out.distro, "Ubuntu");
        assert_eq!(out.user, "u");
        assert_eq!(out.projects, ["/home/u/x/p"]);
        assert_eq!(out.scanned_at, 42);
    }

    #[test]
    fn empty_scan_keeps_cached_projects() {
        let fs = MapFs::new(&[], &[]);
        let out = collect(&FakeWsl { running: true }, &fs, "", &[], cache(), 100);
        assert_eq!(out, cache());
    }

    #[test]
    fn parses_cli_js_from_code_cmd() {
        let cmd = "@echo off\r\nsetlocal\r\nset VSCODE_DEV=\r\nset ELECTRON_RUN_AS_NODE=1\r\n\"%~dp0..\\Code.exe\" \"%~dp0..\\a717861c07\\resources\\app\\out\\cli.js\" %*\r\n";
        assert_eq!(parse_cli_rel(cmd).as_deref(), Some("a717861c07\\resources\\app\\out\\cli.js"));
        assert_eq!(parse_cli_rel("@echo off\r\nfoo"), None);
    }

    #[test]
    fn fallback_refuses_metachar_paths() {
        for bad in ["/home/u/a&b", "/home/u/a|b", "/h/<x>", "/h/a^b", "/h/%PATH%", "/h/(x)", "/h/a\"b", "/h/a!b", "/h/a\nb"] {
            assert_eq!(super::open_command_line("Ubuntu", bad), None, "{bad}");
        }
        assert_eq!(super::open_command_line("Ub&untu", "/home/u/x"), None);
        assert!(super::open_command_line("Ubuntu", "/home/u/my proj").is_some());
    }

    #[test]
    fn open_command_line() {
        assert_eq!(
            super::open_command_line("Ubuntu", "/home/u/workspace/myapp").unwrap(),
            ["cmd.exe", "/c", "code", "--remote", "wsl+Ubuntu", "/home/u/workspace/myapp"]
        );
    }

    #[test]
    fn direct_spawn_args_need_no_escaping() {
        assert_eq!(
            direct_args("cli.js", "Ubuntu", "/home/u/a&b"),
            ["cli.js", "--remote", "wsl+Ubuntu", "/home/u/a&b"]
        );
    }

    #[test]
    fn code_shim_maps_to_code_exe() {
        assert_eq!(
            code_exe_from_shim(r"C:\Users\u\AppData\Local\Programs\Microsoft VS Code\bin\code.cmd").as_deref(),
            Some(r"C:\Users\u\AppData\Local\Programs\Microsoft VS Code\Code.exe")
        );
        assert_eq!(code_exe_from_shim(r"C:\tools\code.cmd"), None);
    }

    #[test]
    fn code_install_is_refreshed_when_missing_or_moved() {
        let inst = CodeInstall { exe: r"C:\VSCode\Code.exe".into(), cli_js: Some(r"C:\VSCode\a717861c07\cli.js".into()) };
        let all = |_: &str| true;
        assert!(needs_refresh(None, all), "nothing cached (installed later / stale PATH)");
        assert!(!needs_refresh(Some(&inst), all));
        // A VS Code update deletes the per-commit folder.
        assert!(needs_refresh(Some(&inst), |p: &str| !p.ends_with("cli.js")));
        assert!(needs_refresh(Some(&inst), |p: &str| !p.ends_with("Code.exe")));
        let no_cli = CodeInstall { cli_js: None, ..inst };
        assert!(!needs_refresh(Some(&no_cli), all));
    }

    #[test]
    fn explorer_folder_argument_is_one_quoted_token() {
        assert_eq!(
            explorer_folder_arg(r"\\wsl.localhost\Ubuntu\home\u\a,b").as_deref(),
            Some(r#""\\wsl.localhost\Ubuntu\home\u\a,b""#)
        );
        assert_eq!(explorer_folder_arg(r#"\\wsl.localhost\Ubuntu\x"y"#), None);
    }

    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_wsl_projects() {
        let d = crate::search::wsl::default_distro();
        println!("default distro: {d:?}");
        let Some(d) = d else { return };
        let running = crate::search::wsl::is_running(&d);
        println!("running: {running}");
        if !running {
            return;
        }
        println!("user: {:?}", crate::search::wsl::home_user(&d));
        let idx = SearchIndex::new();
        let c = collect(&RealWsl, &WinFs { distro: d }, "", &[], Cache::default(), now_secs());
        println!("roots: {:?}", c.roots);
        idx.set_source(Kind::Project, build_entries(&c, None));
        let all = idx.snapshot();
        println!("projects found: {}", all.len());
        for e in all.iter().take(10) {
            println!("  {} [{}] -> {}", e.title, e.subtitle, e.id);
        }
        println!("code exe: {:?}", code_install());
    }
}
