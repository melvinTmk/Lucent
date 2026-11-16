//! Global-hotkey strings ("Alt+Space") <-> Win32 modifiers + virtual key, and the
//! multi-hotkey registry (`HotkeyRegistry`, one Win32 id per `HotkeyId`). Constants are the
//! real Win32 `MOD_*` values.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

const VK_SPACE: u32 = 0x20;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    /// Combination of `MOD_*`.
    pub mods: u32,
    pub vk: u32,
}

/// French AZERTY (fr-FR) unshifted characters -> virtual key. Used to test the mapping and
/// as the fallback when the active layout has no key for the character.
const AZERTY: &[(char, u32)] = &[
    ('²', 0xDE), // VK_OEM_7
    ('&', 0x31),
    ('é', 0x32),
    ('"', 0x33),
    ('\'', 0x34),
    ('(', 0x35),
    ('-', 0x36),
    ('è', 0x37),
    ('_', 0x38),
    ('ç', 0x39),
    ('à', 0x30),
    (')', 0xDB), // VK_OEM_4
    ('=', 0xBB), // VK_OEM_PLUS
    ('^', 0xDD), // VK_OEM_6
    ('$', 0xBA), // VK_OEM_1
    ('ù', 0xC0), // VK_OEM_3
    ('*', 0xDC), // VK_OEM_5
    (',', 0xBC), // VK_OEM_COMMA
    (';', 0xBE), // VK_OEM_PERIOD
    (':', 0xBF), // VK_OEM_2
    ('!', 0xDF), // VK_OEM_8
    ('<', 0xE2), // VK_OEM_102
];

/// Pure AZERTY mapping (see `AZERTY`).
pub fn azerty_vk(c: char) -> Option<u32> {
    AZERTY.iter().find(|(ch, _)| *ch == c).map(|(_, vk)| *vk)
}

/// Virtual key from a `VkKeyScanW` result: None when the layout has no such key (-1) or
/// types it only with a shift state (high byte: Maj/Ctrl/Alt), since registering the bare
/// VK would answer to another character.
pub fn vk_from_scan(r: i16) -> Option<u32> {
    if r == -1 || (r as u16) >> 8 != 0 {
        return None;
    }
    Some((r as u16 & 0xFF) as u32)
}

/// Virtual key of a character on the active keyboard layout (`VkKeyScanW`), falling back
/// to the AZERTY table when the layout has no such key or needs a shift state for it.
fn layout_vk(c: char) -> Option<u32> {
    #[cfg(windows)]
    {
        let mut buf = [0u16; 2];
        if c.encode_utf16(&mut buf).len() == 1 {
            let r = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::VkKeyScanW(buf[0]) };
            if let Some(vk) = vk_from_scan(r) {
                return Some(vk);
            }
        }
    }
    azerty_vk(c)
}

fn parse_key(k: &str, char_vk: impl Fn(char) -> Option<u32>) -> Option<u32> {
    let up = k.to_uppercase();
    match up.as_str() {
        "SPACE" | "ESPACE" => return Some(VK_SPACE),
        "LEFT" | "GAUCHE" => return Some(VK_LEFT),
        "UP" | "HAUT" => return Some(VK_UP),
        "RIGHT" | "DROITE" => return Some(VK_RIGHT),
        "DOWN" | "BAS" => return Some(VK_DOWN),
        _ => {}
    }
    if let Some(n) = up.strip_prefix('F') {
        if let Ok(n @ 1..=24) = n.parse::<u32>() {
            return Some(0x70 + n - 1);
        }
    }
    let mut chars = k.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    if c.is_ascii_alphanumeric() {
        return Some(c.to_ascii_uppercase() as u32);
    }
    char_vk(c)
}

/// Case-insensitive, `+`-separated: modifiers (Ctrl/Control, Alt, Shift, Win) then one key:
/// A-Z, 0-9, Space/Espace, F1-F24, arrows (Left/Gauche, Up/Haut, Right/Droite, Down/Bas) or
/// one OEM character (`²`, `&`, `é`…, resolved on the active layout). At least one of
/// Alt/Ctrl/Win is required.
pub fn parse(s: &str) -> Result<Hotkey, String> {
    parse_with(s, layout_vk)
}

/// `parse` with an explicit character -> virtual-key mapping (tests use `azerty_vk`).
pub fn parse_with(s: &str, char_vk: impl Fn(char) -> Option<u32>) -> Result<Hotkey, String> {
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    let (key, mods_s) = parts.split_last().ok_or("raccourci vide")?;
    let mut mods = 0;
    for m in mods_s {
        mods |= match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => MOD_CONTROL,
            "alt" => MOD_ALT,
            "shift" => MOD_SHIFT,
            "win" => MOD_WIN,
            _ => return Err(format!("modificateur inconnu : {m}")),
        };
    }
    let vk = parse_key(key, char_vk).ok_or_else(|| format!("touche non prise en charge : {key}"))?;
    if mods & (MOD_ALT | MOD_CONTROL | MOD_WIN) == 0 {
        return Err("le raccourci doit contenir Alt, Ctrl ou Win".into());
    }
    Ok(Hotkey { mods, vk })
}

/// Canonical form: `Ctrl+Alt+Shift+Win+Key`, English key names (Space, Left…), OEM keys
/// shown as their AZERTY character.
pub fn format(h: &Hotkey) -> String {
    let mut out = String::new();
    for (bit, name) in [(MOD_CONTROL, "Ctrl"), (MOD_ALT, "Alt"), (MOD_SHIFT, "Shift"), (MOD_WIN, "Win")] {
        if h.mods & bit != 0 {
            out.push_str(name);
            out.push('+');
        }
    }
    match h.vk {
        VK_SPACE => out.push_str("Space"),
        VK_LEFT => out.push_str("Left"),
        VK_UP => out.push_str("Up"),
        VK_RIGHT => out.push_str("Right"),
        VK_DOWN => out.push_str("Down"),
        0x70..=0x87 => out.push_str(&format!("F{}", h.vk - 0x70 + 1)),
        v @ (0x30..=0x39 | 0x41..=0x5A) => out.push(char::from_u32(v).unwrap_or('?')),
        v => match AZERTY.iter().find(|(_, vk)| *vk == v) {
            Some((c, _)) => out.push(*c),
            None => out.push_str(&format!("VK{v:02X}")),
        },
    }
    out
}

pub const DEFAULT: &str = "Alt+Space";

/// Parses a stored combo; a hand-edited invalid one falls back to the default.
/// The bool is true when the fallback was used.
pub fn parse_or_default(s: &str) -> (Hotkey, bool) {
    match parse(s) {
        Ok(h) => (h, false),
        Err(_) => (parse(DEFAULT).expect("default hotkey parses"), true),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HotkeyStatus {
    pub combo: String,
    /// False when the OS refused the registration (combo taken by another app).
    pub ok: bool,
}

/// Where a hotkey is actually registered (Win32 `RegisterHotKey` in production).
pub trait Registrar {
    fn register(&mut self, id: i32, h: &Hotkey) -> bool;
    fn unregister(&mut self, id: i32);
}

/// Every global hotkey Lucent owns. The Win32 id is the position in `ALL` + 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HotkeyId {
    Search,
    Switcher,
    SwitcherBack,
    Overview,
    SnapLeft,
    SnapRight,
    SnapMax,
    SnapCenter,
    SnapThird1,
    SnapThird2,
    SnapThird3,
    MovePrevMonitor,
    MoveNextMonitor,
    MicMute,
    DisplaySwitch,
}

impl HotkeyId {
    pub const ALL: [HotkeyId; 15] = [
        HotkeyId::Search,
        HotkeyId::Switcher,
        HotkeyId::SwitcherBack,
        HotkeyId::Overview,
        HotkeyId::SnapLeft,
        HotkeyId::SnapRight,
        HotkeyId::SnapMax,
        HotkeyId::SnapCenter,
        HotkeyId::SnapThird1,
        HotkeyId::SnapThird2,
        HotkeyId::SnapThird3,
        HotkeyId::MovePrevMonitor,
        HotkeyId::MoveNextMonitor,
        HotkeyId::MicMute,
        HotkeyId::DisplaySwitch,
    ];

    pub fn win_id(self) -> i32 {
        self as i32 + 1
    }

    pub fn from_win_id(id: i32) -> Option<HotkeyId> {
        usize::try_from(id.checked_sub(1)?).ok().and_then(|i| Self::ALL.get(i).copied())
    }
}

pub type HotkeyMap = BTreeMap<HotkeyId, Hotkey>;

/// Fixed snapping / monitor-move combos (only the `shortcuts.snapping` toggle is a setting).
pub const SNAP_COMBOS: [(HotkeyId, &str); 9] = [
    (HotkeyId::SnapLeft, "Ctrl+Alt+Left"),
    (HotkeyId::SnapRight, "Ctrl+Alt+Right"),
    (HotkeyId::SnapMax, "Ctrl+Alt+Up"),
    (HotkeyId::SnapCenter, "Ctrl+Alt+Down"),
    (HotkeyId::SnapThird1, "Ctrl+Alt+D"),
    (HotkeyId::SnapThird2, "Ctrl+Alt+F"),
    (HotkeyId::SnapThird3, "Ctrl+Alt+G"),
    (HotkeyId::MovePrevMonitor, "Ctrl+Alt+Shift+Left"),
    (HotkeyId::MoveNextMonitor, "Ctrl+Alt+Shift+Right"),
];

/// Ids released while a game is focused (the game gets the keys): all but Search, which
/// stays registered and is gated by `game::service::blocks_hotkey`.
pub fn game_suspended_ids() -> Vec<HotkeyId> {
    HotkeyId::ALL.into_iter().filter(|id| *id != HotkeyId::Search).collect()
}

/// Combos wanted by `s`. Empty strings disable a shortcut; an invalid search combo falls
/// back to `DEFAULT`, other invalid ones are skipped. The second value lists the problems.
/// `SwitcherBack` = switcher + Shift (omitted when the switcher combo already has Shift).
pub fn desired(s: &crate::settings::Settings) -> (HotkeyMap, Vec<String>) {
    let mut m = HotkeyMap::new();
    let mut errs = Vec::new();
    let (search, fell_back) = parse_or_default(&s.search.hotkey);
    if fell_back {
        errs.push(format!("raccourci de recherche invalide {:?}, {DEFAULT} utilisé", s.search.hotkey));
    }
    m.insert(HotkeyId::Search, search);
    let mut opt = |id: HotkeyId, combo: &str| -> Option<Hotkey> {
        if combo.trim().is_empty() {
            return None;
        }
        match parse(combo) {
            Ok(h) => Some(h),
            Err(e) => {
                errs.push(format!("{id:?} : {e}"));
                None
            }
        }
    };
    let switcher = opt(HotkeyId::Switcher, &s.shortcuts.switcher);
    let overview = opt(HotkeyId::Overview, &s.shortcuts.overview);
    let mic = opt(HotkeyId::MicMute, &s.shortcuts.mic_mute);
    // Only when a shared monitor is configured.
    let display = if s.display_switch.monitor.trim().is_empty() {
        None
    } else {
        opt(HotkeyId::DisplaySwitch, &s.shortcuts.display_switch)
    };
    if let Some(h) = switcher {
        m.insert(HotkeyId::Switcher, h);
        if h.mods & MOD_SHIFT == 0 {
            m.insert(HotkeyId::SwitcherBack, Hotkey { mods: h.mods | MOD_SHIFT, vk: h.vk });
        }
    }
    if let Some(h) = overview {
        m.insert(HotkeyId::Overview, h);
    }
    if let Some(h) = mic {
        m.insert(HotkeyId::MicMute, h);
    }
    if let Some(h) = display {
        m.insert(HotkeyId::DisplaySwitch, h);
    }
    if s.shortcuts.snapping {
        for (id, combo) in SNAP_COMBOS {
            if let Ok(h) = parse(combo) {
                m.insert(id, h);
            }
        }
    }
    (m, errs)
}

/// Pure bookkeeping around a `Registrar` for every `HotkeyId`. `apply` only touches what
/// changed (changed/removed combos are released before anything is registered); ids that
/// failed are retried on the next `apply`/`resume`. Suspended ids are never registered and
/// are not failures.
pub struct HotkeyRegistry<R: Registrar> {
    reg: R,
    wanted: HotkeyMap,
    registered: HotkeyMap,
    failed: BTreeSet<HotkeyId>,
    suspended: BTreeSet<HotkeyId>,
}

impl<R: Registrar> HotkeyRegistry<R> {
    pub fn new(reg: R) -> Self {
        Self {
            reg,
            wanted: HotkeyMap::new(),
            registered: HotkeyMap::new(),
            failed: BTreeSet::new(),
            suspended: BTreeSet::new(),
        }
    }

    /// Makes `map` the wanted set. `Err` lists the ids the OS refused (taken by another app).
    pub fn apply(&mut self, map: HotkeyMap) -> Result<(), Vec<HotkeyId>> {
        let stale: Vec<HotkeyId> = self
            .registered
            .iter()
            .filter(|(id, h)| map.get(id) != Some(h))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.reg.unregister(id.win_id());
            self.registered.remove(&id);
        }
        self.wanted = map;
        self.sync()
    }

    fn sync(&mut self) -> Result<(), Vec<HotkeyId>> {
        self.failed.clear();
        for (id, h) in &self.wanted {
            if self.suspended.contains(id) || self.registered.contains_key(id) {
                continue;
            }
            if self.reg.register(id.win_id(), h) {
                self.registered.insert(*id, *h);
            } else {
                self.failed.insert(*id);
            }
        }
        if self.failed.is_empty() { Ok(()) } else { Err(self.failed.iter().copied().collect()) }
    }

    /// Releases `ids` until `resume`. Idempotent.
    pub fn suspend(&mut self, ids: &[HotkeyId]) {
        for id in ids {
            self.suspended.insert(*id);
            self.failed.remove(id);
            if self.registered.remove(id).is_some() {
                self.reg.unregister(id.win_id());
            }
        }
    }

    /// Lifts the suspension of `ids` and registers what is missing.
    pub fn resume(&mut self, ids: &[HotkeyId]) -> Result<(), Vec<HotkeyId>> {
        for id in ids {
            self.suspended.remove(id);
        }
        self.sync()
    }

    /// Game Mode: suspends (true) or resumes (false) `game_suspended_ids()`.
    pub fn set_game_focused(&mut self, focused: bool) -> Result<(), Vec<HotkeyId>> {
        let ids = game_suspended_ids();
        if focused {
            self.suspend(&ids);
            Ok(())
        } else {
            self.resume(&ids)
        }
    }

    /// Status of one id; an id not wanted reports an empty combo, ok.
    pub fn status(&self, id: HotkeyId) -> HotkeyStatus {
        match self.wanted.get(&id) {
            Some(h) => HotkeyStatus { combo: format(h), ok: !self.failed.contains(&id) },
            None => HotkeyStatus { combo: String::new(), ok: true },
        }
    }

    /// Status of every wanted id.
    pub fn statuses(&self) -> BTreeMap<HotkeyId, HotkeyStatus> {
        self.wanted.keys().map(|id| (*id, self.status(*id))).collect()
    }

    fn refused_message(&self, ids: &[HotkeyId]) -> String {
        let combos: Vec<String> = ids.iter().map(|id| self.status(*id).combo).collect();
        format!("raccourci indisponible : {}", combos.join(", "))
    }
}

/// Single pending-request slot: "latest wins". A newer `put` replaces (and returns) the older
/// entry; `cancel(seq)` removes only the caller's own entry, so a timed-out request can never
/// override a newer one.
pub struct Slot<T> {
    seq: u64,
    pending: Option<(u64, T)>,
}

impl<T> Slot<T> {
    pub const fn new() -> Self {
        Self { seq: 0, pending: None }
    }
    /// Returns this request's generation and the entry it replaced, if any.
    pub fn put(&mut self, v: T) -> (u64, Option<T>) {
        self.seq += 1;
        let old = self.pending.replace((self.seq, v));
        (self.seq, old.map(|(_, v)| v))
    }
    pub fn take(&mut self) -> Option<T> {
        self.pending.take().map(|(_, v)| v)
    }
    /// True when the entry of generation `seq` was still pending and is now removed.
    pub fn cancel(&mut self, seq: u64) -> bool {
        if matches!(self.pending, Some((g, _)) if g == seq) {
            self.pending = None;
            true
        } else {
            false
        }
    }
}

static STATUS: std::sync::Mutex<BTreeMap<HotkeyId, HotkeyStatus>> = std::sync::Mutex::new(BTreeMap::new());

type Handler = Arc<dyn Fn(HotkeyId) + Send + Sync>;
static HANDLERS: std::sync::Mutex<BTreeMap<HotkeyId, Handler>> = std::sync::Mutex::new(BTreeMap::new());

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

type StatusListener = Arc<dyn Fn() + Send + Sync>;
static GAME_STATUS_LISTENER: std::sync::Mutex<Option<StatusListener>> = std::sync::Mutex::new(None);

/// Called (hotkey thread) after a Game Mode suspend/resume changed the statuses, so the
/// app can broadcast `hotkeys://status`. Keep it short and non-blocking.
pub fn set_game_status_listener(f: impl Fn() + Send + Sync + 'static) {
    *lock(&GAME_STATUS_LISTENER) = Some(Arc::new(f));
}

/// Runs the Game Mode status listener, if any (outside the lock).
fn notify_game_status() {
    let f = lock(&GAME_STATUS_LISTENER).clone();
    if let Some(f) = f {
        f();
    }
}

/// Search hotkey status (what `get_hotkey_status` / `hotkey://status` report).
pub fn status() -> HotkeyStatus {
    lock(&STATUS).get(&HotkeyId::Search).cloned().unwrap_or(HotkeyStatus { combo: String::new(), ok: true })
}

/// Status of every registered-or-wanted id (`get_hotkey_statuses` / `hotkeys://status`).
pub fn statuses() -> BTreeMap<HotkeyId, HotkeyStatus> {
    lock(&STATUS).clone()
}

/// Sets the handler of `id` (replacing any previous one). Handlers run on the hotkey
/// thread: keep them short and non-blocking (hand work off to another thread/channel).
pub fn set_handler(id: HotkeyId, f: impl Fn(HotkeyId) + Send + Sync + 'static) {
    lock(&HANDLERS).insert(id, Arc::new(f));
}

/// Runs the handler of `id`; false when none is set.
pub fn dispatch(id: HotkeyId) -> bool {
    let h = lock(&HANDLERS).get(&id).cloned();
    match h {
        Some(f) => {
            f(id);
            true
        }
        None => false,
    }
}

/// Starts the hotkey thread (Windows only; no-op elsewhere). `on_press` runs on that thread
/// for every press: keep it short. Nothing is registered until the first `set_all`.
pub fn start(on_press: impl Fn(HotkeyId) + Send + 'static) {
    #[cfg(windows)]
    win::start(Box::new(on_press));
    #[cfg(not(windows))]
    let _ = on_press;
}

/// Makes `map` the registered set (diff-based); blocks until the hotkey thread replied.
/// `Err` names the refused combos (statuses are updated either way).
pub fn set_all(map: HotkeyMap) -> Result<(), String> {
    #[cfg(windows)]
    return win::set(map);
    #[cfg(not(windows))]
    {
        let _ = map;
        Err("raccourci global indisponible".into())
    }
}

/// Game Mode hook: a focused game releases every hotkey but Search; unfocused/exit
/// registers them again. Non-blocking, idempotent.
pub fn set_game_focused(focused: bool) {
    #[cfg(windows)]
    win::set_game_focused(focused);
    #[cfg(not(windows))]
    let _ = focused;
}

#[cfg(windows)]
mod win {
    use super::{HotkeyId, HotkeyMap, HotkeyRegistry, Registrar, STATUS, Slot, lock};
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::mpsc::{self, Sender};
    use std::sync::Mutex;
    use std::time::Duration;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
        PostMessageW, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
        WM_HOTKEY, WNDCLASSW,
    };
    use windows::core::w;

    type Reply = Sender<Result<(), String>>;

    static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
    static SLOT: Mutex<Slot<(HotkeyMap, Reply)>> = Mutex::new(Slot::new());
    /// Wanted Game Mode suspension; the hotkey thread reconciles on WM_APP.
    static GAME_FOCUSED: AtomicBool = AtomicBool::new(false);

    struct Win32Registrar {
        hwnd: HWND,
    }

    impl Registrar for Win32Registrar {
        fn register(&mut self, id: i32, h: &super::Hotkey) -> bool {
            let mods = HOT_KEY_MODIFIERS(h.mods) | MOD_NOREPEAT;
            let r = unsafe { RegisterHotKey(Some(self.hwnd), id, mods, h.vk) };
            if let Err(e) = &r {
                eprintln!("lucent: RegisterHotKey {} failed: {e}", super::format(h));
            }
            r.is_ok()
        }
        fn unregister(&mut self, id: i32) {
            unsafe {
                let _ = UnregisterHotKey(Some(self.hwnd), id);
            }
        }
    }

    thread_local! {
        static REGISTRY: RefCell<Option<HotkeyRegistry<Win32Registrar>>> = const { RefCell::new(None) };
        static ON_PRESS: RefCell<Option<Box<dyn Fn(HotkeyId)>>> = const { RefCell::new(None) };
        /// Game suspension currently applied to the registry.
        static APPLIED_GAME: Cell<bool> = const { Cell::new(false) };
    }

    fn drain_requests() {
        let request = lock(&SLOT).take();
        let mut game_changed = false;
        REGISTRY.with(|r| {
            let mut r = r.borrow_mut();
            let Some(reg) = r.as_mut() else { return };
            let focused = GAME_FOCUSED.load(Ordering::SeqCst);
            if APPLIED_GAME.with(|g| g.replace(focused)) != focused {
                game_changed = true;
                if let Err(ids) = reg.set_game_focused(focused) {
                    eprintln!("lucent: {}", reg.refused_message(&ids));
                }
            }
            if let Some((map, reply)) = request {
                let result = reg.apply(map).map_err(|ids| reg.refused_message(&ids));
                let _ = reply.send(result);
            }
            *lock(&STATUS) = reg.statuses();
        });
        if game_changed {
            super::notify_game_status();
        }
    }

    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_HOTKEY => {
                if let Some(id) = i32::try_from(wp.0).ok().and_then(HotkeyId::from_win_id) {
                    ON_PRESS.with(|p| {
                        if let Some(f) = p.borrow().as_ref() {
                            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(id))).is_err() {
                                eprintln!("lucent: hotkey callback panicked ({id:?})");
                            }
                        }
                    });
                }
                LRESULT(0)
            }
            WM_APP => {
                drain_requests();
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    pub fn start(on_press: Box<dyn Fn(HotkeyId) + Send>) {
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let spawned = std::thread::Builder::new().name("lucent-hotkey".into()).spawn(move || unsafe {
            let Ok(hmodule) = GetModuleHandleW(None) else { return };
            let class = w!("LucentHotkey");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: hmodule.into(),
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                eprintln!("lucent: RegisterClassW failed for hotkey window");
                return;
            }
            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("Lucent hotkey"),
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
                    eprintln!("lucent: CreateWindowExW failed for hotkey window: {e}");
                    return;
                }
            };
            REGISTRY.with(|s| *s.borrow_mut() = Some(HotkeyRegistry::new(Win32Registrar { hwnd })));
            ON_PRESS.with(|p| *p.borrow_mut() = Some(on_press));
            HWND_RAW.store(hwnd.0 as isize, Ordering::SeqCst);
            let _ = ready_tx.send(());
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            // Thread ending: release the combos.
            REGISTRY.with(|s| *s.borrow_mut() = None);
            HWND_RAW.store(0, Ordering::SeqCst);
        });
        match spawned {
            Ok(_) => {
                if ready_rx.recv_timeout(Duration::from_secs(5)).is_err() {
                    eprintln!("lucent: hotkey thread did not start");
                }
            }
            Err(e) => eprintln!("lucent: cannot spawn hotkey thread: {e}"),
        }
    }

    fn post() -> Result<(), String> {
        let raw = HWND_RAW.load(Ordering::SeqCst);
        if raw == 0 {
            return Err("service de raccourci non démarré".into());
        }
        unsafe { PostMessageW(Some(HWND(raw as *mut _)), WM_APP, WPARAM(0), LPARAM(0)) }
            .map_err(|e| format!("PostMessage a échoué : {e}"))
    }

    pub fn set_game_focused(focused: bool) {
        if GAME_FOCUSED.swap(focused, Ordering::SeqCst) != focused {
            if let Err(e) = post() {
                eprintln!("lucent: hotkey game suspension: {e}");
            }
        }
    }

    pub fn set(map: HotkeyMap) -> Result<(), String> {
        if HWND_RAW.load(Ordering::SeqCst) == 0 {
            return Err("service de raccourci non démarré".into());
        }
        let (tx, rx) = mpsc::channel();
        let (seq, replaced) = lock(&SLOT).put((map, tx));
        if let Some((_, old_reply)) = replaced {
            let _ = old_reply.send(Err("remplacé par un raccourci plus récent".into()));
        }
        let cancel = || {
            lock(&SLOT).cancel(seq);
        };
        if let Err(e) = post() {
            cancel();
            return Err(e);
        }
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(r) => r,
            Err(_) => {
                cancel();
                Err("le service de raccourci ne répond pas".to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips() {
        let h = parse("Alt+Space").unwrap();
        assert_eq!(h, Hotkey { mods: MOD_ALT, vk: 0x20 });
        assert_eq!(format(&h), "Alt+Space");
        let h = parse("ctrl+shift+K").unwrap();
        assert_eq!(h, Hotkey { mods: MOD_CONTROL | MOD_SHIFT, vk: 0x4B });
        assert_eq!(format(&h), "Ctrl+Shift+K");
        assert_eq!(parse("Win+F24").unwrap().vk, 0x87);
        assert_eq!(format(&parse("win+alt+f5").unwrap()), "Alt+Win+F5");
        assert_eq!(parse("Ctrl+7").unwrap().vk, 0x37);
    }

    #[test]
    fn parse_rejects_bare_or_shift_only() {
        for s in ["Space", "Shift+A", "Alt+", "Alt+Foo", "", "Alt+F25", "Alt+F0"] {
            assert!(parse(s).is_err(), "{s} should be rejected");
        }
    }

    #[derive(Default)]
    struct Fake {
        succeed: bool,
        log: Vec<String>,
    }
    impl Registrar for Fake {
        fn register(&mut self, id: i32, h: &Hotkey) -> bool {
            self.log.push(format!("register {id} {}", format(h)));
            self.succeed
        }
        fn unregister(&mut self, id: i32) {
            self.log.push(format!("unregister {id}"));
        }
    }

    fn search(s: &str) -> HotkeyMap {
        map(&[(HotkeyId::Search, s)])
    }

    #[test]
    fn register_failure_is_reported_not_fatal() {
        let mut svc = HotkeyRegistry::new(Fake { succeed: false, ..Default::default() });
        assert!(svc.apply(search("Alt+Space")).is_err());
        assert_eq!(svc.status(HotkeyId::Search), HotkeyStatus { combo: "Alt+Space".into(), ok: false });
        svc.reg.succeed = true;
        assert!(svc.apply(search("Alt+Space")).is_ok());
        assert_eq!(svc.status(HotkeyId::Search), HotkeyStatus { combo: "Alt+Space".into(), ok: true });
    }

    #[test]
    fn reapply_unregisters_old_first() {
        let mut svc = HotkeyRegistry::new(Fake { succeed: true, ..Default::default() });
        svc.apply(search("Alt+Space")).unwrap();
        svc.apply(search("Ctrl+Alt+K")).unwrap();
        assert_eq!(svc.reg.log, ["register 1 Alt+Space", "unregister 1", "register 1 Ctrl+Alt+K"]);
    }

    #[test]
    fn failed_apply_leaves_nothing_registered() {
        let mut svc = HotkeyRegistry::new(Fake { succeed: true, ..Default::default() });
        svc.apply(search("Alt+Space")).unwrap();
        svc.reg.succeed = false;
        assert!(svc.apply(search("Alt+J")).is_err());
        assert_eq!(svc.status(HotkeyId::Search), HotkeyStatus { combo: "Alt+J".into(), ok: false });
        // Old combo was unregistered, so a retry must not unregister again.
        svc.reg.log.clear();
        let _ = svc.apply(search("Alt+J"));
        assert_eq!(svc.reg.log, ["register 1 Alt+J"]);
    }

    #[test]
    fn slot_latest_wins_and_cancel_only_removes_own() {
        let mut slot = Slot::new();
        let (g1, old) = slot.put("A");
        assert!(old.is_none());
        let (g2, old) = slot.put("B");
        assert_eq!(old, Some("A"));
        assert!(!slot.cancel(g1), "stale request must not cancel the newer one");
        assert!(slot.cancel(g2));
        assert_eq!(slot.take(), None);
        let (_, _) = slot.put("C");
        assert_eq!(slot.take(), Some("C"));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn startup_falls_back_to_default_on_invalid() {
        assert_eq!(parse_or_default("Ctrl+K"), (parse("Ctrl+K").unwrap(), false));
        assert_eq!(parse_or_default("nonsense"), (parse(DEFAULT).unwrap(), true));
    }

    // ---- M6: extended keys ----

    #[test]
    fn game_status_listener_is_notified() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        notify_game_status();
        set_game_status_listener(|| {
            N.fetch_add(1, Ordering::SeqCst);
        });
        notify_game_status();
        assert_eq!(N.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn vk_scan_rejects_shifted_and_missing_keys() {
        assert_eq!(vk_from_scan(0x00DE), Some(0xDE), "² unshifted on AZERTY");
        assert_eq!(vk_from_scan(0x0131), None, "Maj+1 types the character: not the bare key");
        assert_eq!(vk_from_scan(0x0636), None, "AltGr");
        assert_eq!(vk_from_scan(-1), None);
    }

    #[test]
    fn parse_m6_defaults() {
        assert_eq!(parse_with("Alt+²", azerty_vk).unwrap(), Hotkey { mods: MOD_ALT, vk: 0xDE });
        assert_eq!(parse("Ctrl+Alt+Gauche").unwrap(), Hotkey { mods: MOD_CONTROL | MOD_ALT, vk: 0x25 });
        assert_eq!(parse("Ctrl+Alt+Shift+Droite").unwrap(), Hotkey { mods: MOD_CONTROL | MOD_ALT | MOD_SHIFT, vk: 0x27 });
        assert_eq!(parse("Ctrl+Alt+Espace").unwrap(), Hotkey { mods: MOD_CONTROL | MOD_ALT, vk: 0x20 });
        assert_eq!(parse("Ctrl+Alt+Haut").unwrap().vk, 0x26);
        assert_eq!(parse("Ctrl+Alt+bas").unwrap().vk, 0x28);
        for (s, vk) in [("Ctrl+Alt+Left", 0x25), ("Ctrl+Alt+Up", 0x26), ("Ctrl+Alt+Right", 0x27), ("Ctrl+Alt+Down", 0x28)] {
            assert_eq!(parse(s).unwrap().vk, vk, "{s}");
        }
        assert_eq!(parse("Ctrl+Alt+D").unwrap().vk, 0x44);
    }

    #[test]
    fn azerty_table_maps_oem_characters() {
        assert_eq!(azerty_vk('²'), Some(0xDE));
        assert_eq!(azerty_vk('&'), Some(0x31));
        assert_eq!(azerty_vk('é'), Some(0x32));
        assert_eq!(azerty_vk(')'), Some(0xDB));
        assert_eq!(azerty_vk('ù'), Some(0xC0));
        assert_eq!(azerty_vk('€'), None);
        assert_eq!(parse_with("Alt+&", azerty_vk).unwrap().vk, 0x31);
        assert!(parse_with("Alt+€", azerty_vk).is_err());
    }

    #[test]
    fn format_names_arrows_and_oem() {
        assert_eq!(format(&parse("ctrl+alt+gauche").unwrap()), "Ctrl+Alt+Left");
        assert_eq!(format(&parse("Ctrl+Alt+Espace").unwrap()), "Ctrl+Alt+Space");
        assert_eq!(format(&Hotkey { mods: MOD_ALT, vk: 0xDE }), "Alt+²");
        assert_eq!(format(&Hotkey { mods: MOD_ALT | MOD_SHIFT, vk: 0x28 }), "Alt+Shift+Down");
    }

    #[test]
    fn display_switch_hotkey_only_with_a_shared_monitor() {
        let mut s = crate::settings::Settings::default();
        assert!(!desired(&s).0.contains_key(&HotkeyId::DisplaySwitch));
        s.display_switch.monitor = "24E4".into();
        assert!(desired(&s).0.contains_key(&HotkeyId::DisplaySwitch));
        s.shortcuts.display_switch = String::new();
        assert!(!desired(&s).0.contains_key(&HotkeyId::DisplaySwitch));
    }

    #[test]
    fn hotkey_id_win_ids_round_trip() {
        for (i, id) in HotkeyId::ALL.iter().enumerate() {
            assert_eq!(id.win_id(), i as i32 + 1);
            assert_eq!(HotkeyId::from_win_id(id.win_id()), Some(*id));
        }
        assert_eq!(HotkeyId::from_win_id(0), None);
        assert_eq!(HotkeyId::from_win_id(16), None);
        assert_eq!(HotkeyId::from_win_id(-3), None);
        assert_eq!(serde_json::to_string(&HotkeyId::SnapThird1).unwrap(), "\"snapThird1\"");
        assert_eq!(serde_json::to_string(&HotkeyId::MovePrevMonitor).unwrap(), "\"movePrevMonitor\"");
    }

    // ---- M6: registry ----

    /// Fake registrar refusing the combos listed in `taken`.
    #[derive(Default)]
    struct Picky {
        taken: Vec<String>,
        log: Vec<String>,
    }
    impl Registrar for Picky {
        fn register(&mut self, id: i32, h: &Hotkey) -> bool {
            self.log.push(format!("register {id} {}", format(h)));
            !self.taken.contains(&format(h))
        }
        fn unregister(&mut self, id: i32) {
            self.log.push(format!("unregister {id}"));
        }
    }

    fn map(pairs: &[(HotkeyId, &str)]) -> HotkeyMap {
        pairs.iter().map(|(id, s)| (*id, parse(s).unwrap())).collect()
    }

    #[test]
    fn registry_reports_partial_failure_per_id() {
        let mut r = HotkeyRegistry::new(Picky { taken: vec!["Ctrl+Alt+M".into()], ..Default::default() });
        let m = map(&[(HotkeyId::Search, "Alt+Space"), (HotkeyId::MicMute, "Ctrl+Alt+M"), (HotkeyId::SnapLeft, "Ctrl+Alt+Left")]);
        assert_eq!(r.apply(m), Err(vec![HotkeyId::MicMute]));
        assert_eq!(r.status(HotkeyId::Search), HotkeyStatus { combo: "Alt+Space".into(), ok: true });
        assert_eq!(r.status(HotkeyId::MicMute), HotkeyStatus { combo: "Ctrl+Alt+M".into(), ok: false });
        assert_eq!(r.status(HotkeyId::SnapLeft), HotkeyStatus { combo: "Ctrl+Alt+Left".into(), ok: true });
        assert_eq!(r.status(HotkeyId::Overview), HotkeyStatus { combo: String::new(), ok: true });
        assert_eq!(r.statuses().len(), 3);
    }

    #[test]
    fn registry_reapply_only_touches_changes_and_retries_failures() {
        let mut r = HotkeyRegistry::new(Picky { taken: vec!["Ctrl+Alt+M".into()], ..Default::default() });
        let _ = r.apply(map(&[(HotkeyId::Search, "Alt+Space"), (HotkeyId::MicMute, "Ctrl+Alt+M"), (HotkeyId::SnapLeft, "Ctrl+Alt+Left")]));
        r.reg.log.clear();
        r.reg.taken.clear();
        // Search changes, SnapLeft removed, MicMute unchanged but failed: retried.
        assert_eq!(r.apply(map(&[(HotkeyId::Search, "Ctrl+K"), (HotkeyId::MicMute, "Ctrl+Alt+M")])), Ok(()));
        assert_eq!(r.reg.log, ["unregister 1", "unregister 5", "register 1 Ctrl+K", "register 14 Ctrl+Alt+M"]);
        r.reg.log.clear();
        assert_eq!(r.apply(map(&[(HotkeyId::Search, "Ctrl+K"), (HotkeyId::MicMute, "Ctrl+Alt+M")])), Ok(()));
        assert!(r.reg.log.is_empty(), "unchanged map must not touch the OS: {:?}", r.reg.log);
    }

    #[test]
    fn registry_suspend_and_resume() {
        let mut r = HotkeyRegistry::new(Picky::default());
        let _ = r.apply(map(&[(HotkeyId::Search, "Alt+Space"), (HotkeyId::Switcher, "Alt+F1"), (HotkeyId::SnapLeft, "Ctrl+Alt+Left")]));
        r.reg.log.clear();
        assert_eq!(r.set_game_focused(true), Ok(()));
        assert_eq!(r.reg.log, ["unregister 2", "unregister 5"]);
        assert_eq!(r.set_game_focused(true), Ok(()));
        assert_eq!(r.reg.log.len(), 2, "suspend is idempotent");
        // Suspended ids are not failures.
        assert!(r.status(HotkeyId::Switcher).ok);
        // A settings change while suspended does not register the suspended ids.
        r.reg.log.clear();
        let _ = r.apply(map(&[(HotkeyId::Search, "Alt+Space"), (HotkeyId::Switcher, "Alt+F2"), (HotkeyId::SnapLeft, "Ctrl+Alt+Left")]));
        assert!(r.reg.log.is_empty(), "{:?}", r.reg.log);
        assert_eq!(r.status(HotkeyId::Switcher).combo, "Alt+F2");
        r.reg.taken = vec!["Alt+F2".into()];
        assert_eq!(r.set_game_focused(false), Err(vec![HotkeyId::Switcher]));
        assert_eq!(r.reg.log, ["register 2 Alt+F2", "register 5 Ctrl+Alt+Left"]);
        assert!(!r.status(HotkeyId::Switcher).ok);
        // Explicit suspend/resume of a subset.
        r.reg.log.clear();
        r.suspend(&[HotkeyId::Search]);
        assert_eq!(r.reg.log, ["unregister 1"]);
        r.reg.log.clear();
        r.reg.taken.clear();
        assert_eq!(r.resume(&[HotkeyId::Search]), Ok(()));
        assert_eq!(r.reg.log, ["register 1 Alt+Space", "register 2 Alt+F2"]);
    }

    #[test]
    fn dispatch_runs_the_handler_of_the_id() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static HITS: AtomicU32 = AtomicU32::new(0);
        assert!(!dispatch(HotkeyId::SnapThird3));
        set_handler(HotkeyId::SnapThird3, |id| {
            assert_eq!(id, HotkeyId::SnapThird3);
            HITS.fetch_add(1, Ordering::SeqCst);
        });
        assert!(dispatch(HotkeyId::SnapThird3));
        assert_eq!(HITS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn game_suspension_spares_search_only() {
        let ids = game_suspended_ids();
        assert_eq!(ids.len(), HotkeyId::ALL.len() - 1);
        assert!(!ids.contains(&HotkeyId::Search));
    }

    #[test]
    fn desired_map_from_default_settings() {
        let (m, errs) = desired(&crate::settings::Settings::default());
        assert!(errs.is_empty(), "{errs:?}");
        let f = |id| m.get(&id).map(format);
        assert_eq!(f(HotkeyId::Search).as_deref(), Some("Alt+Space"));
        assert_eq!(f(HotkeyId::Switcher).as_deref(), Some("Alt+²"));
        assert_eq!(f(HotkeyId::SwitcherBack).as_deref(), Some("Alt+Shift+²"));
        assert_eq!(f(HotkeyId::Overview).as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(f(HotkeyId::SnapLeft).as_deref(), Some("Ctrl+Alt+Left"));
        assert_eq!(f(HotkeyId::SnapRight).as_deref(), Some("Ctrl+Alt+Right"));
        assert_eq!(f(HotkeyId::SnapMax).as_deref(), Some("Ctrl+Alt+Up"));
        assert_eq!(f(HotkeyId::SnapCenter).as_deref(), Some("Ctrl+Alt+Down"));
        assert_eq!(f(HotkeyId::SnapThird1).as_deref(), Some("Ctrl+Alt+D"));
        assert_eq!(f(HotkeyId::SnapThird2).as_deref(), Some("Ctrl+Alt+F"));
        assert_eq!(f(HotkeyId::SnapThird3).as_deref(), Some("Ctrl+Alt+G"));
        assert_eq!(f(HotkeyId::MovePrevMonitor).as_deref(), Some("Ctrl+Alt+Shift+Left"));
        assert_eq!(f(HotkeyId::MoveNextMonitor).as_deref(), Some("Ctrl+Alt+Shift+Right"));
        assert_eq!(f(HotkeyId::MicMute).as_deref(), Some("Ctrl+Alt+M"));
        assert_eq!(m.len(), 14);
    }

    #[test]
    fn desired_map_honours_toggles_and_empty_strings() {
        let mut s = crate::settings::Settings::default();
        s.shortcuts.snapping = false;
        s.shortcuts.mic_mute = String::new();
        s.shortcuts.switcher = "Alt+Shift+F1".into();
        s.shortcuts.overview = "nonsense".into();
        s.search.hotkey = "bad".into();
        let (m, errs) = desired(&s);
        assert_eq!(m.keys().copied().collect::<Vec<_>>(), [HotkeyId::Search, HotkeyId::Switcher]);
        assert_eq!(format(&m[&HotkeyId::Search]), DEFAULT, "invalid search combo falls back");
        assert_eq!(errs.len(), 2, "{errs:?}");
    }
}
