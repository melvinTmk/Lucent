//! Detects launcher libraries (Steam, Epic, Riot, EA) and classifies executables.
//! Only reads files and the registry; never touches game processes (Vanguard).
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Steam,
    Epic,
    Riot,
    Ea,
    Manual,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Class {
    Game(Source),
    NotGame,
    Unknown,
}

#[derive(Debug, Default, Clone)]
pub struct Libraries {
    /// Lowercased dirs with a trailing `\`.
    pub roots: Vec<(String, Source)>,
    /// Subset of `roots` that are containers of game folders (Steam `common`, EA, the Riot
    /// fallback). Other roots are a single game's own install dir.
    pub containers: Vec<String>,
    /// Launcher install dirs (lowercased, trailing `\`): anything under them is not a game,
    /// unless a more specific library root contains it (Steam games live under Steam's dir).
    pub launcher_dirs: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize)]
pub struct DetectedGame {
    pub name: String,
    /// For library entries, the game's folder.
    pub exe: String,
    pub source: Source,
}

/// Lowercase, backslashes, trailing `\`.
pub fn normalize_dir(p: &str) -> String {
    let mut s = p.trim().replace('/', "\\").to_lowercase();
    if !s.ends_with('\\') {
        s.push('\\');
    }
    s
}

/// Every `"path"` value of a libraryfolders.vdf, unescaped.
pub fn steam_library_paths(vdf: &str) -> Vec<String> {
    vdf.lines()
        .filter_map(|line| {
            let mut parts = line.split('"').skip(1).step_by(2);
            if parts.next()? != "path" {
                return None;
            }
            let v = parts.next()?;
            (!v.is_empty()).then(|| v.replace("\\\\", "\\"))
        })
        .collect()
}

/// `InstallLocation` of an Epic `.item` manifest.
pub fn epic_install_location(item_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(item_json).ok()?;
    let s = v.get("InstallLocation")?.as_str()?;
    (!s.is_empty()).then(|| s.to_string())
}

/// `product_install_full_path` (or `product_install_root`) of a Riot product settings yaml,
/// normalised.
pub fn riot_install_path(product_settings_yaml: &str) -> Option<String> {
    for key in ["product_install_full_path", "product_install_root"] {
        for line in product_settings_yaml.lines() {
            let Some(rest) = line.trim().strip_prefix(key) else { continue };
            let Some(val) = rest.trim_start().strip_prefix(':') else { continue };
            let val = val.trim().trim_matches(|c| c == '"' || c == '\'');
            if !val.is_empty() {
                return Some(normalize_dir(val));
            }
        }
    }
    None
}

/// `associated_client` keys of RiotClientInstalls.json, normalised.
pub fn riot_installs_json_roots(json: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    v.get("associated_client")
        .and_then(|m| m.as_object())
        .map(|m| m.keys().map(|k| normalize_dir(k)).collect())
        .unwrap_or_default()
}

/// Dirs of the Riot Client executables (`rc_default`, `rc_live`), normalised.
pub fn riot_client_dirs(json: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    ["rc_default", "rc_live"]
        .iter()
        .filter_map(|k| v.get(*k)?.as_str())
        .filter_map(|p| {
            let p = p.replace('/', "\\");
            let (dir, _) = p.rsplit_once('\\')?;
            Some(normalize_dir(dir))
        })
        .collect()
}

const NOT_GAME_FILES: &[&str] = &[
    "steam.exe",
    "steamwebhelper.exe",
    "epicgameslauncher.exe",
    "riotclientservices.exe",
    "riotclientux.exe",
    "riotclientuxrender.exe",
    "vgc.exe",
    "vgtray.exe",
    "eadesktop.exe",
    "crashreportclient.exe",
    "unrealcefsubprocess.exe",
];

fn file_name(exe_lower: &str) -> &str {
    exe_lower.rsplit(['\\', '/']).next().unwrap_or(exe_lower)
}

fn is_builtin_not_game(exe_lower: &str) -> bool {
    let f = file_name(exe_lower);
    NOT_GAME_FILES.contains(&f)
        || (f.starts_with("easyanticheat") && f.ends_with(".exe"))
        || (f.starts_with("beservice") && f.ends_with(".exe"))
}

fn norm_path(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

fn matches_any(exe: &str, list: &[String]) -> bool {
    list.iter().any(|p| {
        let p = norm_path(p);
        let p = p.trim();
        if p.is_empty() {
            return false;
        }
        let dir = if p.ends_with('\\') { p.to_string() } else { format!("{p}\\") };
        exe == p.trim_end_matches('\\') || exe.starts_with(&dir)
    })
}

pub fn classify(exe_lower: &str, libs: &Libraries, include: &[String], exclude: &[String]) -> Class {
    let exe = norm_path(exe_lower);
    if matches_any(&exe, exclude) {
        return Class::NotGame;
    }
    if matches_any(&exe, include) {
        return Class::Game(Source::Manual);
    }
    if is_builtin_not_game(&exe) {
        return Class::NotGame;
    }
    let root = libs
        .roots
        .iter()
        .filter(|(d, _)| exe.starts_with(d.as_str()))
        .max_by_key(|(d, _)| d.len());
    let launcher = libs
        .launcher_dirs
        .iter()
        .filter(|d| exe.starts_with(d.as_str()))
        .map(|d| d.len())
        .max();
    match (root, launcher) {
        (_, Some(l)) if root.is_none_or(|(d, _)| l >= d.len()) => Class::NotGame,
        (Some((_, s)), _) => Class::Game(*s),
        _ => Class::Unknown,
    }
}

/// Real-cased name of `segment` inside `parent` (roots are stored lowercased).
fn real_name(parent: &str, segment: &str) -> String {
    if let Ok(rd) = std::fs::read_dir(parent) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.to_lowercase() == segment {
                return n;
            }
        }
    }
    segment.to_string()
}

/// Installed games: one entry per single-game root (Epic InstallLocation, Riot product dir:
/// named after the product folder, skipping a trailing `live`/`pbe`), first-level folders
/// under container roots (Steam `common`, EA, Riot fallback; launcher dirs skipped), plus
/// manual includes.
pub fn list_detected_games(libs: &Libraries, include: &[String]) -> Vec<DetectedGame> {
    let mut out = Vec::new();
    for (root, source) in &libs.roots {
        if libs.containers.contains(root) {
            let Ok(rd) = std::fs::read_dir(root.as_str()) else { continue };
            for e in rd.flatten() {
                if !e.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let name = e.file_name().to_string_lossy().to_string();
                let dir = normalize_dir(&format!("{root}{name}"));
                if libs.launcher_dirs.contains(&dir) {
                    continue;
                }
                out.push(DetectedGame { name, exe: dir, source: *source });
            }
        } else {
            let mut segs: Vec<&str> = root.split('\\').filter(|x| !x.is_empty()).collect();
            while segs.len() > 1 && matches!(segs.last(), Some(&"live") | Some(&"pbe")) {
                segs.pop();
            }
            let Some(seg) = segs.pop() else { continue };
            let parent = if segs.is_empty() { String::new() } else { segs.join("\\") + "\\" };
            out.push(DetectedGame { name: real_name(&parent, seg), exe: root.clone(), source: *source });
        }
    }
    for p in include {
        let name = std::path::Path::new(p)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| p.clone());
        out.push(DetectedGame { name, exe: p.clone(), source: Source::Manual });
    }
    out
}

/// Reads Steam, Epic, Riot and EA libraries from this machine. Missing files are skipped.
#[cfg(windows)]
pub fn discover() -> Libraries {
    use std::fs;
    let mut libs = Libraries::default();

    // Steam
    if let Some(sp) = win::steam_path() {
        let steam = normalize_dir(&sp);
        let vdf = fs::read_to_string(format!(r"{}steamapps\libraryfolders.vdf", steam)).unwrap_or_default();
        let mut paths = steam_library_paths(&vdf);
        if paths.is_empty() {
            paths.push(sp.replace('/', "\\"));
        }
        for p in paths {
            let r = format!(r"{}steamapps\common\", normalize_dir(&p));
            libs.containers.push(r.clone());
            libs.roots.push((r, Source::Steam));
        }
        libs.launcher_dirs.push(steam);
    }

    // Epic
    let manifests = r"C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests";
    if let Ok(rd) = fs::read_dir(manifests) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("item")) {
                if let Some(loc) = fs::read_to_string(e.path()).ok().and_then(|s| epic_install_location(&s)) {
                    libs.roots.push((normalize_dir(&loc), Source::Epic));
                }
            }
        }
    }
    libs.launcher_dirs.push(normalize_dir(r"C:\Program Files (x86)\Epic Games\Launcher"));

    // Riot: RiotClientInstalls.json, then yaml metadata, then C:\Riot Games\
    let before = libs.roots.len();
    if let Ok(json) = fs::read_to_string(r"C:\ProgramData\Riot Games\RiotClientInstalls.json") {
        for r in riot_installs_json_roots(&json) {
            libs.roots.push((r, Source::Riot));
        }
        libs.launcher_dirs.extend(riot_client_dirs(&json));
    }
    if libs.roots.len() == before {
        if let Ok(rd) = fs::read_dir(r"C:\ProgramData\Riot Games\Metadata") {
            for e in rd.flatten() {
                let Ok(inner) = fs::read_dir(e.path()) else { continue };
                for f in inner.flatten() {
                    if f.file_name().to_string_lossy().ends_with(".product_settings.yaml") {
                        if let Some(p) = fs::read_to_string(f.path()).ok().and_then(|s| riot_install_path(&s)) {
                            libs.roots.push((p, Source::Riot));
                        }
                    }
                }
            }
        }
    }
    if libs.roots.len() == before {
        let r = normalize_dir(r"C:\Riot Games");
        libs.containers.push(r.clone());
        libs.roots.push((r, Source::Riot));
    }
    libs.launcher_dirs.push(normalize_dir(r"C:\Riot Games\Riot Client"));

    // EA
    libs.launcher_dirs.push(normalize_dir(r"C:\Program Files\Electronic Arts\EA Desktop"));
    for p in [r"C:\Program Files\EA Games", r"C:\Program Files (x86)\EA Games"] {
        if std::path::Path::new(p).is_dir() {
            let r = normalize_dir(p);
            libs.containers.push(r.clone());
            libs.roots.push((r, Source::Ea));
        }
    }
    let mut seen = std::collections::HashSet::new();
    libs.roots.retain(|(r, _)| seen.insert(r.clone()));
    libs.containers.sort();
    libs.containers.dedup();
    libs.launcher_dirs.sort();
    libs.launcher_dirs.dedup();
    libs
}

#[cfg(not(windows))]
pub fn discover() -> Libraries {
    Libraries::default()
}

#[cfg(windows)]
mod win {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::w;

    /// `HKCU\Software\Valve\Steam` `SteamPath` (forward slashes as stored).
    pub fn steam_path() -> Option<String> {
        unsafe {
            let sub = w!(r"Software\Valve\Steam");
            let value = w!("SteamPath");
            let mut len = 0u32;
            if RegGetValueW(HKEY_CURRENT_USER, sub, value, RRF_RT_REG_SZ, None, None, Some(&mut len)).0 != 0
                || len == 0
            {
                return None;
            }
            let mut buf = vec![0u16; (len as usize).div_ceil(2)];
            let r = RegGetValueW(
                HKEY_CURRENT_USER,
                sub,
                value,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut _),
                Some(&mut len),
            );
            if r.0 != 0 {
                return None;
            }
            let n = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
            let s = String::from_utf16_lossy(&buf[..n]);
            (!s.is_empty()).then_some(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RIOT_JSON: &str = r#"{"associated_client": {"C:/Riot Games/VALORANT/live/": "C:/Riot Games/Riot Client/RiotClientServices.exe"}, "patchlines": {}, "rc_default": "C:/riot games/riot client/RiotClientServices.exe", "rc_live": "C:/riot games/riot client/RiotClientServices.exe"}"#;

    fn libs() -> Libraries {
        let mut l = Libraries::default();
        l.roots.push((r"c:\riot games\valorant\live\".into(), Source::Riot));
        l.roots.push((r"c:\program files (x86)\steam\steamapps\common\".into(), Source::Steam));
        l.launcher_dirs = vec![
            r"c:\riot games\riot client\".into(),
            r"c:\program files (x86)\steam\".into(),
        ];
        l
    }

    #[test]
    fn steam_vdf_two_libraries() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(
            steam_library_paths(vdf),
            vec![r"C:\Program Files (x86)\Steam".to_string(), r"D:\SteamLibrary".to_string()]
        );
    }

    #[test]
    fn epic_item_install_location() {
        let j = r#"{"DisplayName":"X","InstallLocation":"D:\\Epic\\Fortnite","Other":1}"#;
        assert_eq!(epic_install_location(j), Some(r"D:\Epic\Fortnite".to_string()));
        assert_eq!(epic_install_location("not json"), None);
    }

    #[test]
    fn riot_yaml_path_normalised() {
        let y = "product_id: \"valorant\"\nproduct_install_full_path: \"C:/Riot Games/VALORANT/live\"\n";
        assert_eq!(riot_install_path(y), Some(r"c:\riot games\valorant\live\".to_string()));
    }

    #[test]
    fn riot_installs_json_sample() {
        assert_eq!(riot_installs_json_roots(RIOT_JSON), vec![r"c:\riot games\valorant\live\".to_string()]);
        assert_eq!(riot_client_dirs(RIOT_JSON), vec![r"c:\riot games\riot client\".to_string(), r"c:\riot games\riot client\".to_string()]);
        assert!(riot_installs_json_roots("garbage").is_empty());
    }

    #[test]
    fn classify_cases() {
        let l = libs();
        let none: Vec<String> = vec![];
        let val = r"c:\riot games\valorant\live\shootergame\binaries\win64\valorant-win64-shipping.exe";
        assert_eq!(classify(val, &l, &none, &none), Class::Game(Source::Riot));
        assert_eq!(classify(r"c:\riot games\vanguard\vgc.exe", &l, &none, &none), Class::NotGame);
        assert_eq!(classify(r"c:\riot games\riot client\foo.exe", &l, &none, &none), Class::NotGame);
        assert_eq!(
            classify(r"c:\program files (x86)\steam\steamapps\common\hl3\hl3.exe", &l, &none, &none),
            Class::Game(Source::Steam)
        );
        assert_eq!(classify(r"c:\program files (x86)\steam\steam.exe", &l, &none, &none), Class::NotGame);
        assert_eq!(classify(r"c:\x\easyanticheat_eos.exe", &l, &none, &none), Class::NotGame);
        let brave = r"c:\program files\bravesoftware\brave-browser\application\brave.exe";
        assert_eq!(classify(brave, &l, &none, &none), Class::Unknown);
        assert_eq!(classify(val, &l, &none, &[val.to_string()]), Class::NotGame);
        assert_eq!(classify(brave, &l, &[brave.to_string()], &none), Class::Game(Source::Manual));
        assert_eq!(classify(brave, &l, &[r"C:\Program Files\BraveSoftware\".to_string()], &none), Class::Game(Source::Manual));
    }

    #[test]
    fn dir_filters_match_children_without_trailing_slash() {
        let l = libs();
        let none: Vec<String> = vec![];
        let exe = r"c:\games\foo\bin\foo.exe";
        assert_eq!(classify(exe, &l, &[r"C:\Games\Foo".to_string()], &none), Class::Game(Source::Manual));
        assert_eq!(classify(exe, &l, &none, &[r"C:\Games\Foo".to_string()]), Class::NotGame);
        assert_eq!(classify(r"c:\games\foobar\x.exe", &l, &[r"C:\Games\Foo".to_string()], &none), Class::Unknown);
    }

    #[test]
    fn list_detected_games_shapes() {
        let base = std::env::temp_dir().join(format!("lucent-game-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for p in [
            r"steam\steamapps\common\Half-Life",
            r"epic\Fortnite\Engine",
            r"riot\VALORANT\live\ShooterGame",
            r"riotfb\Riot Client",
            r"riotfb\LeagueOfLegends",
        ] {
            std::fs::create_dir_all(base.join(p)).unwrap();
        }
        let d = |p: &str| normalize_dir(base.join(p).to_str().unwrap());
        let mut l = Libraries::default();
        let steam = d(r"steam\steamapps\common");
        let rfb = d("riotfb");
        l.roots = vec![
            (steam.clone(), Source::Steam),
            (d(r"epic\Fortnite"), Source::Epic),
            (d(r"riot\VALORANT\live"), Source::Riot),
            (rfb.clone(), Source::Riot),
        ];
        l.containers = vec![steam, rfb];
        l.launcher_dirs = vec![d(r"riotfb\Riot Client")];
        let got = list_detected_games(&l, &[r"C:\Games\Foo\foo.exe".to_string()]);
        let mut names: Vec<(&str, Source)> = got.iter().map(|g| (g.name.as_str(), g.source)).collect();
        assert_eq!(names.len(), 5);
        names.sort_by_key(|n| n.0.to_string());
        assert_eq!(
            names,
            vec![
                ("Fortnite", Source::Epic),
                ("Half-Life", Source::Steam),
                ("LeagueOfLegends", Source::Riot),
                ("VALORANT", Source::Riot),
                ("foo", Source::Manual),
            ]
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn source_serializes_lowercase_for_the_front() {
        let g = DetectedGame { name: "VALORANT".into(), exe: r"c:\riot games\valorant\live\".into(), source: Source::Riot };
        let v = serde_json::to_value(&g).unwrap();
        assert_eq!(v["source"], "riot");
    }
}
