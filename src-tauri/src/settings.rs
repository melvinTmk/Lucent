use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub schema_version: u32,
    pub dock: DockSettings,
    pub animations: AnimationSettings,
    pub general: GeneralSettings,
    pub search: SearchSettings,
    pub game_mode: GameModeSettings,
    pub valorant: ValorantSettings,
    pub shortcuts: ShortcutSettings,
    pub hot_corners: HotCorners,
    /// Control Center panel (clicking the dock's system island).
    pub control_center: bool,
    /// Quick Look preview (Shift+Espace on a search result).
    pub quick_look: bool,
    /// Monitor shared with another PC (input switch over DDC/CI).
    pub display_switch: DisplaySwitchSettings,
}

/// Shared monitor: its EDID name ("" = feature off) and the VCP 0x60 input codes, which
/// are model-specific (see `system::display_input`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct DisplaySwitchSettings {
    pub monitor: String,
    /// Input showing this PC.
    pub here: u8,
    /// Input showing the other PC.
    pub other: u8,
    /// Name of the other PC in the dock card ("PC portable").
    pub other_label: String,
    /// While it shows the other PC, take the monitor off this PC's desktop (windows move
    /// to the remaining monitors, the cursor cannot get lost on it).
    pub detach: bool,
}

impl Default for DisplaySwitchSettings {
    fn default() -> Self {
        Self { monitor: String::new(), here: 1, other: 3, other_label: "Autre PC".into(), detach: true }
    }
}

/// M6 global shortcuts (see `hotkey::parse`; "" disables one). Snapping combos are fixed
/// (`hotkey::SNAP_COMBOS`): only the toggle is a setting.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct ShortcutSettings {
    /// App switcher; Shift added = backwards.
    pub switcher: String,
    pub overview: String,
    pub snapping: bool,
    pub mic_mute: String,
    /// Shared monitor: this PC ⇄ the other PC.
    pub display_switch: String,
    /// Window snapping combos (used while `snapping` is on; "" disables one).
    pub snap: SnapShortcuts,
}

/// One combo per snapping action. Defaults: `hotkey::SNAP_COMBOS`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct SnapShortcuts {
    pub left: String,
    pub right: String,
    pub max: String,
    pub center: String,
    pub third1: String,
    pub third2: String,
    pub third3: String,
    pub prev_monitor: String,
    pub next_monitor: String,
}

impl Default for SnapShortcuts {
    fn default() -> Self {
        Self {
            left: "Ctrl+Alt+Left".into(),
            right: "Ctrl+Alt+Right".into(),
            max: "Ctrl+Alt+Up".into(),
            center: "Ctrl+Alt+Down".into(),
            third1: "Ctrl+Alt+D".into(),
            third2: "Ctrl+Alt+F".into(),
            third3: "Ctrl+Alt+G".into(),
            prev_monitor: "Ctrl+Alt+Shift+Left".into(),
            next_monitor: "Ctrl+Alt+Shift+Right".into(),
        }
    }
}

impl SnapShortcuts {
    /// (hotkey id, combo) for every snapping action.
    pub fn entries(&self) -> [(crate::hotkey::HotkeyId, &str); 9] {
        use crate::hotkey::HotkeyId::*;
        [
            (SnapLeft, &self.left),
            (SnapRight, &self.right),
            (SnapMax, &self.max),
            (SnapCenter, &self.center),
            (SnapThird1, &self.third1),
            (SnapThird2, &self.third2),
            (SnapThird3, &self.third3),
            (MovePrevMonitor, &self.prev_monitor),
            (MoveNextMonitor, &self.next_monitor),
        ]
    }
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        Self {
            switcher: "Alt+²".into(),
            overview: "Ctrl+Alt+Space".into(),
            snapping: true,
            mic_mute: "Ctrl+Alt+M".into(),
            display_switch: "Ctrl+Alt+O".into(),
            snap: SnapShortcuts::default(),
        }
    }
}

/// Valid hot-corner actions (`HotCorners` fields).
pub const CORNER_ACTIONS: [&str; 6] = ["none", "overview", "desktop", "lock", "search", "switcher"];

/// Action per screen corner, one of `CORNER_ACTIONS`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct HotCorners {
    pub top_left: String,
    pub top_right: String,
    pub bottom_left: String,
    pub bottom_right: String,
}

impl Default for HotCorners {
    fn default() -> Self {
        Self {
            top_left: "overview".into(),
            top_right: "none".into(),
            bottom_left: "none".into(),
            bottom_right: "none".into(),
        }
    }
}

fn clamp_corner(v: &mut String) {
    let a = v.trim().to_ascii_lowercase();
    *v = if CORNER_ACTIONS.contains(&a.as_str()) { a } else { "none".into() };
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct DockSettings {
    pub icon_size: u32,
    pub magnification: bool,
    pub magnification_scale: f32,
    pub position: DockPosition,
    pub reveal_delay_ms: u32,
    pub hide_delay_ms: u32,
    pub monitors: MonitorSelection,
    pub glass: GlassMode,
    pub pinned: Vec<PinnedApp>,
    pub show_clock: bool,
    /// Media island (current system media session) in the dock.
    pub show_media: bool,
    /// Never auto-hide (except over a fullscreen app or a focused game); the dock then
    /// reserves its strip of the screen like a taskbar.
    pub always_visible: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum DockPosition {
    Bottom,
    Left,
    Right,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MonitorSelection {
    All,
    Primary,
    List(Vec<String>),
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum GlassMode {
    Auto,
    Light,
    Dark,
}

/// Identity of an application: lowercased full exe path, or a packaged-app AUMID.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum AppId {
    Exe(String),
    Aumid(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PinnedApp {
    pub app: AppId,
    pub name: String,
    pub is_game: bool,
}

impl Default for PinnedApp {
    fn default() -> Self {
        Self { app: AppId::Exe(String::new()), name: String::new(), is_game: false }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AnimationSettings {
    pub speed: f32,
    pub style: AnimStyle,
    pub card_duration_ms: u32,
    pub reduce_motion: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum AnimStyle {
    Spring,
    Sober,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct GeneralSettings {
    pub launch_at_startup: bool,
    pub hide_windows_taskbar: bool,
    pub language: String,
    /// Look for a new version on GitHub (at startup, then every few hours).
    pub check_updates: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchSettings {
    /// Global hotkey, e.g. "Alt+Space" (see `hotkey::parse`).
    pub hotkey: String,
    /// Empty = auto-detect.
    pub project_roots: Vec<String>,
    /// Empty = default WSL distribution.
    pub wsl_distro: String,
    pub clipboard_history: bool,
    /// `%s` is replaced by the query.
    pub web_url: String,
    /// "" = auto (follows the glass tone), else "#rrggbb".
    pub text_color: String,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            hotkey: "Alt+Space".into(),
            project_roots: Vec::new(),
            wsl_distro: String::new(),
            clipboard_history: true,
            web_url: "https://www.google.com/search?q=%s".into(),
            text_color: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct GameModeSettings {
    pub enabled: bool,
    /// Lowercased exe paths always treated as games.
    pub include: Vec<String>,
    /// Lowercased exe paths never treated as games.
    pub exclude: Vec<String>,
    pub power_plan: bool,
    pub monitor_pill: bool,
    pub ping_target: String,
}

impl Default for GameModeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            include: Vec::new(),
            exclude: Vec::new(),
            power_plan: true,
            monitor_pill: true,
            ping_target: "1.1.1.1".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct ValorantSettings {
    pub riot_id: String,
    /// One of eu, na, ap, kr, latam, br.
    pub region: String,
    pub api_key: String,
    pub post_match: bool,
    /// Keep the last match's card in the dock at all times.
    pub pin_card: bool,
}

impl Default for ValorantSettings {
    fn default() -> Self {
        Self { riot_id: String::new(), region: "eu".into(), api_key: String::new(), post_match: true, pin_card: false }
    }
}

fn normalise_paths(v: &mut Vec<String>) {
    let mut out: Vec<String> = Vec::with_capacity(v.len());
    for s in v.iter() {
        let s = s.trim().to_lowercase();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    }
    *v = out;
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            dock: DockSettings::default(),
            animations: AnimationSettings::default(),
            general: GeneralSettings::default(),
            search: SearchSettings::default(),
            game_mode: GameModeSettings::default(),
            valorant: ValorantSettings::default(),
            shortcuts: ShortcutSettings::default(),
            hot_corners: HotCorners::default(),
            control_center: true,
            quick_look: true,
            display_switch: DisplaySwitchSettings::default(),
        }
    }
}

impl Default for DockSettings {
    fn default() -> Self {
        Self {
            icon_size: 36,
            magnification: true,
            magnification_scale: 1.6,
            position: DockPosition::Bottom,
            reveal_delay_ms: 150,
            hide_delay_ms: 400,
            monitors: MonitorSelection::All,
            glass: GlassMode::Auto,
            pinned: vec![PinnedApp {
                app: AppId::Exe(r"c:\windows\explorer.exe".into()),
                name: "Explorateur de fichiers".into(),
                is_game: false,
            }],
            show_clock: true,
            show_media: true,
            always_visible: false,
        }
    }
}

impl Default for AnimationSettings {
    fn default() -> Self {
        Self { speed: 1.0, style: AnimStyle::Spring, card_duration_ms: 4000, reduce_motion: false }
    }
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self { launch_at_startup: true, hide_windows_taskbar: true, language: "fr".into(), check_updates: true }
    }
}

pub struct SettingsStore {
    current: Mutex<Settings>,
    path: PathBuf,
}

pub fn settings_path() -> PathBuf {
    crate::paths::config_dir().join("settings.json")
}

/// Reads settings; never fails. Missing file -> defaults. Corrupt file -> copied to
/// `<name>.bak` and defaults returned. Missing fields are filled with defaults.
pub fn load_from(path: &Path) -> Settings {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Settings::default(),
    };
    match serde_json::from_str::<Settings>(&text) {
        Ok(s) => s.clamped(),
        Err(_) => {
            let _ = fs::write(path.with_extension("json.bak"), &text);
            Settings::default()
        }
    }
}

/// Atomic save: write `<name>.tmp` then rename over the target.
pub fn save_to(path: &Path, s: &Settings) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(s).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)
}

impl Settings {
    pub fn clamped(mut self) -> Settings {
        self.dock.icon_size = self.dock.icon_size.clamp(32, 96);
        self.dock.magnification_scale = self.dock.magnification_scale.clamp(1.0, 2.5);
        self.animations.speed = self.animations.speed.clamp(0.5, 2.0);
        let c = &self.search.text_color;
        let valid = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
        self.search.text_color = if valid { c.to_ascii_lowercase() } else { String::new() };
        normalise_paths(&mut self.game_mode.include);
        normalise_paths(&mut self.game_mode.exclude);
        let ping = self.game_mode.ping_target.trim();
        self.game_mode.ping_target = if ping.is_empty() { "1.1.1.1".into() } else { ping.into() };
        self.valorant.riot_id = self.valorant.riot_id.trim().into();
        if !["eu", "na", "ap", "kr", "latam", "br"].contains(&self.valorant.region.as_str()) {
            self.valorant.region = "eu".into();
        }
        let hc = &mut self.hot_corners;
        for v in [&mut hc.top_left, &mut hc.top_right, &mut hc.bottom_left, &mut hc.bottom_right] {
            clamp_corner(v);
        }
        self
    }
}

/// Pins of packaged exes (`resolve` gives their AUMID) become AUMID pins, keeping name
/// and game flag; a pin that would duplicate an existing one is dropped. `None` when
/// nothing changed. `resolve` is only called for `AppId::Exe` pins.
pub fn migrate_pins(pins: &[PinnedApp], resolve: impl Fn(&str) -> Option<String>) -> Option<Vec<PinnedApp>> {
    let mut changed = false;
    let mut out: Vec<PinnedApp> = Vec::with_capacity(pins.len());
    for p in pins {
        let mut p = p.clone();
        if let AppId::Exe(path) = &p.app {
            if let Some(aumid) = resolve(path) {
                p.app = AppId::Aumid(aumid);
                changed = true;
            }
        }
        if out.iter().any(|o| o.app == p.app) {
            changed = true;
            continue;
        }
        out.push(p);
    }
    changed.then_some(out)
}

impl SettingsStore {
    pub fn new(s: Settings, path: PathBuf) -> Self {
        Self { current: Mutex::new(s), path }
    }

    pub fn get(&self) -> Settings {
        self.current.lock().unwrap().clone()
    }

    /// Clamp, persist and store, all under the lock so concurrent saves are serialized
    /// (the file and the in-memory value always agree). Returns the stored value.
    pub fn apply(&self, s: Settings) -> Result<Settings, String> {
        crate::hotkey::parse(&s.search.hotkey)
            .map_err(|e| format!("raccourci de recherche invalide : {e}"))?;
        let snap = s.shortcuts.snap.entries().map(|(_, c)| c.to_string());
        let main = [&s.shortcuts.switcher, &s.shortcuts.overview, &s.shortcuts.mic_mute, &s.shortcuts.display_switch];
        for combo in main.into_iter().chain(snap.iter()) {
            if !combo.trim().is_empty() {
                crate::hotkey::parse(combo).map_err(|e| format!("raccourci invalide « {combo} » : {e}"))?;
            }
        }
        // The switcher closes on the Alt release: a combo without Alt would pick at once.
        if !s.shortcuts.switcher.trim().is_empty()
            && crate::hotkey::parse(&s.shortcuts.switcher).is_ok_and(|h| h.mods & crate::hotkey::MOD_ALT == 0)
        {
            return Err("Le changeur d'apps doit utiliser Alt".into());
        }
        let s = s.clamped();
        let mut cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
        save_to(&self.path, &s).map_err(|e| e.to_string())?;
        *cur = s.clone();
        Ok(s)
    }

    /// `apply` then broadcast `settings://changed`.
    pub fn update(&self, app: &AppHandle, s: Settings) -> Result<(), String> {
        let s = self.apply(s)?;
        app.emit("settings://changed", s).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);
    /// A fresh, empty directory per call. Windows reuses process ids and these dirs are never
    /// cleaned, so pid + counter alone could hand a test a stale `settings.json` from an
    /// earlier run (`apply_rejects_bad_hotkey` then saw the file "exist"): add a timestamp
    /// and wipe whatever is there.
    fn tmp() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let d = std::env::temp_dir().join(format!(
            "lucent-settings-test-{}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst),
            nanos
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test] fn tmp_dirs_are_unique_and_empty() {
        let (a, b) = (tmp(), tmp());
        assert_ne!(a, b);
        for d in [a, b] {
            assert_eq!(fs::read_dir(&d).unwrap().count(), 0);
        }
    }

    #[test] fn missing_file_returns_defaults() { assert_eq!(load_from(&tmp().join("nope.json")), Settings::default()); }
    #[test] fn defaults_match_spec() {
        let s = Settings::default();
        assert_eq!(s.dock.icon_size, 36); assert!(s.dock.magnification); assert_eq!(s.dock.magnification_scale, 1.6);
        assert_eq!(s.dock.glass, GlassMode::Auto); assert_eq!(s.animations.card_duration_ms, 4000);
        assert_eq!(s.general.language, "fr"); assert!(s.general.hide_windows_taskbar);
        assert_eq!(s.dock.pinned.len(), 1); assert_eq!(s.dock.pinned[0].app, AppId::Exe(r"c:\windows\explorer.exe".into()));
    }
    #[test] fn missing_search_section_gets_defaults() {
        let p = tmp().join("settings.json");
        fs::write(&p, r#"{"schemaVersion":1,"dock":{"iconSize":64},"general":{"language":"fr"}}"#).unwrap();
        let s = load_from(&p);
        assert_eq!(s.search.hotkey, "Alt+Space");
        assert!(s.search.project_roots.is_empty());
        assert_eq!(s.search.wsl_distro, "");
        assert!(s.search.clipboard_history);
        assert_eq!(s.search.web_url, "https://www.google.com/search?q=%s");
    }
    #[test] fn apply_rejects_bad_hotkey() {
        let p = tmp().join("settings.json");
        let store = SettingsStore::new(Settings::default(), p.clone());
        let mut s = Settings::default(); s.search.hotkey = "Shift+A".into();
        assert!(store.apply(s).is_err());
        assert!(!p.exists());
        let mut s = Settings::default(); s.search.hotkey = "Ctrl+Shift+K".into();
        assert!(store.apply(s).is_ok());
    }
    #[test] fn corrupt_file_backs_up_and_returns_defaults() {
        let p = tmp().join("settings.json"); fs::write(&p, "{not json").unwrap();
        assert_eq!(load_from(&p), Settings::default());
        assert_eq!(fs::read_to_string(p.with_extension("json.bak")).unwrap(), "{not json");
    }
    fn pin(app: AppId, name: &str, is_game: bool) -> PinnedApp { PinnedApp { app, name: name.into(), is_game } }
    #[test] fn migrate_pins_converts_resolved_exes() {
        let pins = vec![
            pin(AppId::Exe(r"c:\windows\explorer.exe".into()), "Explorer", false),
            pin(AppId::Exe(r"c:\program files\windowsapps\s_1_x64__p\spotify.exe".into()), "Spotify", true),
            pin(AppId::Exe(r"c:\program files\windowsapps\gone_1_x64__p\gone.exe".into()), "Gone", false),
        ];
        let out = migrate_pins(&pins, |p| p.contains("spotify").then(|| "S_p!Spotify".to_string())).unwrap();
        assert_eq!(out, vec![
            pins[0].clone(),
            pin(AppId::Aumid("S_p!Spotify".into()), "Spotify", true),
            pins[2].clone(),
        ]);
    }
    #[test] fn migrate_pins_unchanged_is_none_and_never_resolves_aumids() {
        let pins = vec![pin(AppId::Aumid("A!App".into()), "A", false), pin(AppId::Exe("c:\\x.exe".into()), "X", false)];
        assert_eq!(migrate_pins(&pins, |p| { assert_eq!(p, "c:\\x.exe"); None }), None);
    }
    #[test] fn migrate_pins_drops_duplicates() {
        let pins = vec![
            pin(AppId::Aumid("S_p!Spotify".into()), "Spotify", false),
            pin(AppId::Exe("c:\\s.exe".into()), "Spotify old", false),
        ];
        assert_eq!(migrate_pins(&pins, |_| Some("S_p!Spotify".into())), Some(vec![pins[0].clone()]));
    }
    #[test] fn show_media_defaults_to_true() { assert!(Settings::default().dock.show_media); }
    #[test] fn text_color_defaults_to_auto() { assert_eq!(Settings::default().search.text_color, ""); }
    #[test] fn clamped_normalises_text_color() {
        for (raw, want) in [("#FFAA00", "#ffaa00"), ("#12", ""), ("red", ""), ("javascript:x", ""), ("", ""), ("#ffffff", "#ffffff")] {
            let mut s = Settings::default(); s.search.text_color = raw.into();
            assert_eq!(s.clamped().search.text_color, want, "{raw}");
        }
    }
    #[test] fn m2_settings_without_text_color_load() {
        let p = tmp().join("settings.json"); fs::write(&p, r#"{"search":{"hotkey":"Alt+Space"}}"#).unwrap();
        assert_eq!(load_from(&p).search.text_color, "");
    }
    #[test] fn game_and_valorant_defaults() {
        let s = Settings::default();
        assert!(s.game_mode.enabled && s.game_mode.power_plan && s.game_mode.monitor_pill);
        assert!(s.game_mode.include.is_empty() && s.game_mode.exclude.is_empty());
        assert_eq!(s.game_mode.ping_target, "1.1.1.1");
        assert_eq!(s.valorant.riot_id, ""); assert_eq!(s.valorant.region, "eu");
        assert_eq!(s.valorant.api_key, ""); assert!(s.valorant.post_match);
    }
    #[test] fn clamped_normalises_game_and_valorant() {
        let mut s = Settings::default();
        s.game_mode.include = vec![" C:\\Games\\A.exe ".into(), "c:\\games\\a.exe".into(), "".into(), "  ".into(), "D:\\B.EXE".into()];
        s.game_mode.exclude = vec!["X.EXE".into(), "x.exe".into()];
        s.game_mode.ping_target = "   ".into();
        s.valorant.region = "XX".into(); s.valorant.riot_id = "  Name#EUW  ".into();
        let c = s.clamped();
        assert_eq!(c.game_mode.include, vec!["c:\\games\\a.exe", "d:\\b.exe"]);
        assert_eq!(c.game_mode.exclude, vec!["x.exe"]);
        assert_eq!(c.game_mode.ping_target, "1.1.1.1");
        assert_eq!(c.valorant.region, "eu"); assert_eq!(c.valorant.riot_id, "Name#EUW");
        let mut s = Settings::default(); s.game_mode.ping_target = " 8.8.8.8 ".into();
        for r in ["eu", "na", "ap", "kr", "latam", "br"] { s.valorant.region = r.into(); assert_eq!(s.clone().clamped().valorant.region, r); }
        assert_eq!(s.clamped().game_mode.ping_target, "8.8.8.8");
    }
    #[test] fn m3_settings_without_game_mode_load() {
        let p = tmp().join("settings.json"); fs::write(&p, r##"{"search":{"hotkey":"Alt+Space","textColor":"#ffffff"}}"##).unwrap();
        let s = load_from(&p);
        assert_eq!(s.game_mode, GameModeSettings::default()); assert_eq!(s.valorant, ValorantSettings::default());
        assert_eq!(s.search.text_color, "#ffffff");
    }
    #[test] fn partial_file_fills_missing_fields() {
        let p = tmp().join("settings.json"); fs::write(&p, r#"{"dock":{"iconSize":64}}"#).unwrap();
        let s = load_from(&p); assert_eq!(s.dock.icon_size, 64); assert_eq!(s.dock.hide_delay_ms, 400);
    }
    #[test] fn clamped_bounds_values() {
        let mut s = Settings::default(); s.dock.icon_size = 500; s.animations.speed = 0.0;
        let c = s.clamped(); assert_eq!(c.dock.icon_size, 96); assert_eq!(c.animations.speed, 0.5);
    }
    #[test] fn save_then_load_roundtrips() {
        let p = tmp().join("settings.json");
        let mut s = Settings::default();
        s.dock.icon_size = 72;
        s.dock.position = DockPosition::Left;
        s.dock.monitors = MonitorSelection::List(vec!["a".into(), "b".into()]);
        s.dock.pinned.push(PinnedApp { app: AppId::Aumid("X!App".into()), name: "X".into(), is_game: true });
        s.animations.style = AnimStyle::Sober;
        save_to(&p, &s).unwrap();
        assert!(!p.with_extension("json.tmp").exists());
        assert_eq!(load_from(&p), s);
    }
    #[test] fn concurrent_applies_leave_file_and_memory_consistent() {
        let p = tmp().join("settings.json");
        let store = std::sync::Arc::new(SettingsStore::new(Settings::default(), p.clone()));
        let hs: Vec<_> = (0..16u32).map(|i| {
            let st = store.clone();
            std::thread::spawn(move || {
                for j in 0..20u32 {
                    let mut s = Settings::default();
                    s.dock.icon_size = 32 + (i * 20 + j) % 60;
                    st.apply(s).unwrap();
                }
            })
        }).collect();
        for h in hs { h.join().unwrap(); }
        assert_eq!(load_from(&p), store.get());
    }

    #[test] fn m6_defaults() {
        let s = Settings::default();
        assert_eq!(s.shortcuts.switcher, "Alt+²"); assert_eq!(s.shortcuts.overview, "Ctrl+Alt+Space");
        assert!(s.shortcuts.snapping); assert_eq!(s.shortcuts.mic_mute, "Ctrl+Alt+M");
        assert_eq!(s.hot_corners.top_left, "overview");
        assert_eq!(s.hot_corners.top_right, "none"); assert_eq!(s.hot_corners.bottom_left, "none"); assert_eq!(s.hot_corners.bottom_right, "none");
        assert!(s.control_center); assert!(s.quick_look);
    }
    #[test] fn clamped_validates_hot_corners() {
        let mut s = Settings::default();
        s.hot_corners.top_left = "explode".into(); s.hot_corners.top_right = " Lock ".into();
        s.hot_corners.bottom_left = "desktop".into(); s.hot_corners.bottom_right = "".into();
        let c = s.clamped();
        assert_eq!(c.hot_corners.top_left, "none"); assert_eq!(c.hot_corners.top_right, "lock");
        assert_eq!(c.hot_corners.bottom_left, "desktop"); assert_eq!(c.hot_corners.bottom_right, "none");
        for a in CORNER_ACTIONS { let mut s = Settings::default(); s.hot_corners.top_left = a.into(); assert_eq!(s.clamped().hot_corners.top_left, a); }
    }
    #[test] fn m5_settings_without_m6_sections_load() {
        let p = tmp().join("settings.json");
        fs::write(&p, r##"{"schemaVersion":1,"dock":{"iconSize":40,"showMedia":false},"search":{"hotkey":"Ctrl+K"},"gameMode":{"enabled":false},"valorant":{"region":"na"}}"##).unwrap();
        let s = load_from(&p);
        assert_eq!(s.dock.icon_size, 40); assert_eq!(s.search.hotkey, "Ctrl+K"); assert!(!s.game_mode.enabled);
        assert_eq!(s.shortcuts, ShortcutSettings::default()); assert_eq!(s.hot_corners, HotCorners::default());
        assert!(s.control_center && s.quick_look);
        let p = tmp().join("settings.json");
        fs::write(&p, r#"{"shortcuts":{"micMute":""},"hotCorners":{"bottomRight":"search"},"quickLook":false}"#).unwrap();
        let s = load_from(&p);
        assert_eq!(s.shortcuts.mic_mute, ""); assert_eq!(s.shortcuts.switcher, "Alt+²");
        assert_eq!(s.hot_corners.bottom_right, "search"); assert_eq!(s.hot_corners.top_left, "overview");
        assert!(!s.quick_look && s.control_center);
    }
    #[test] fn apply_rejects_bad_shortcut_but_allows_empty() {
        let store = SettingsStore::new(Settings::default(), tmp().join("settings.json"));
        let mut s = Settings::default(); s.shortcuts.overview = "Shift+A".into();
        assert!(store.apply(s).is_err());
        let mut s = Settings::default(); s.shortcuts.mic_mute = String::new(); s.shortcuts.switcher = String::new();
        assert!(store.apply(s).is_ok());
    }
    #[test] fn apply_rejects_switcher_without_alt() {
        let store = SettingsStore::new(Settings::default(), tmp().join("settings.json"));
        let mut s = Settings::default(); s.shortcuts.switcher = "Ctrl+Q".into();
        assert_eq!(store.apply(s).unwrap_err(), "Le changeur d'apps doit utiliser Alt");
        let mut s = Settings::default(); s.shortcuts.switcher = "Ctrl+Alt+Q".into();
        assert!(store.apply(s).is_ok());
    }
}
