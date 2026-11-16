//! Game session service: Windows glue around the pure `session::Session`. One worker
//! thread owns the session; foreground changes (wintrack hook), prompt answers, settings
//! and library refreshes reach it as messages. Emits `game://state` on every change and
//! `game://ask` for prompts.
//!
//! Vanguard: a game process is only ever opened with PROCESS_QUERY_LIMITED_INFORMATION
//! (image path); its exit is detected by polling a Toolhelp process snapshot, never by
//! waiting on a handle.
use super::library::{self, DetectedGame, Libraries};
use super::session::GameStateDto;
use crate::settings::GameModeSettings;
use std::sync::mpsc::Sender;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use tauri::AppHandle;

/// Process-list poll period while a game is active.
pub const POLL: Duration = Duration::from_secs(2);
/// Library rediscovery period.
pub const LIBRARY_REFRESH: Duration = Duration::from_secs(10 * 60);

pub enum Msg {
    /// New foreground window (HWND).
    Foreground(isize),
    Answer { exe: String, is_game: bool },
    Settings(GameModeSettings),
    Libraries(Libraries),
}

#[derive(serde::Serialize, Clone)]
struct AskPayload {
    exe: String,
    name: String,
}

static TX: OnceLock<Sender<Msg>> = OnceLock::new();
static STATE: Mutex<GameStateDto> = Mutex::new(GameStateDto { active: false, name: None, monitor_id: None, focused: false });
static ACTIVE_EXE: Mutex<Option<String>> = Mutex::new(None);
static LIBS: Mutex<Option<Libraries>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sends to the worker; a no-op before `start` (and off Windows).
pub fn send(m: Msg) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(m);
    }
}

/// Last published game state.
pub fn state() -> GameStateDto {
    lock(&STATE).clone()
}

/// A game session is running (cheap: no clone).
pub fn is_active() -> bool {
    lock(&STATE).active
}

/// The exe path of the currently active game session, if any.
pub fn active_exe() -> Option<String> {
    lock(&ACTIVE_EXE).clone()
}

/// The search hotkey is ignored while the game is focused and the cursor is on its
/// monitor (a stray Alt+Space must not pop a window over the game).
pub fn blocks_hotkey() -> bool {
    let s = state();
    s.active && s.focused && cursor_monitor_id().is_some_and(|m| s.owns(&m))
}

#[cfg(windows)]
fn cursor_monitor_id() -> Option<String> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut pt = POINT::default();
    unsafe { GetCursorPos(&mut pt) }.ok()?;
    crate::monitors::win::enumerate().into_iter().find(|m| m.rect.contains(pt.x, pt.y)).map(|m| m.id)
}

#[cfg(not(windows))]
fn cursor_monitor_id() -> Option<String> {
    None
}

/// Installed games. Blocking: discovers the libraries if the refresher has not yet.
pub fn detected_games(include: &[String]) -> Vec<DetectedGame> {
    let cached = lock(&LIBS).clone();
    let libs = cached.unwrap_or_else(|| {
        let l = library::discover();
        *lock(&LIBS) = Some(l.clone());
        l
    });
    library::list_detected_games(&libs, include)
}

/// Starts the worker, the library refresher and the foreground subscription. Needs
/// `wintrack::win::watch` running for foreground events.
#[cfg(windows)]
pub fn start(app: &AppHandle, gm: GameModeSettings) {
    win::start(app, gm);
}

#[cfg(not(windows))]
pub fn start(app: &AppHandle, gm: GameModeSettings) {
    let _ = (app, gm);
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::game::library::Class;
    use crate::game::power;
    use crate::game::session::{Effect, Input, Session};
    use crate::monitors::{self, Rect};
    use crate::settings::{AppId, Settings};
    use std::sync::mpsc::{RecvTimeoutError, channel};
    use std::time::Instant;
    use tauri::{Emitter, Listener};
    use windows::Win32::Foundation::{CloseHandle, HWND};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromWindow};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    pub fn start(app: &AppHandle, gm: GameModeSettings) {
        let (tx, rx) = channel::<Msg>();
        if TX.set(tx).is_err() {
            return; // already started
        }
        let worker_app = app.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("lucent-game".into())
            .spawn(move || Worker::new(worker_app, gm).run(rx))
        {
            eprintln!("lucent: cannot spawn game session thread: {e}");
            return;
        }
        if let Err(e) = std::thread::Builder::new().name("lucent-game-libs".into()).spawn(|| loop {
            let l = library::discover();
            *lock(&LIBS) = Some(l.clone());
            send(Msg::Libraries(l));
            std::thread::sleep(LIBRARY_REFRESH);
        }) {
            eprintln!("lucent: cannot spawn game library thread: {e}");
        }
        crate::wintrack::win::on_foreground(|h| send(Msg::Foreground(h)));
        app.listen_any("settings://changed", |event| match serde_json::from_str::<Settings>(event.payload()) {
            Ok(s) => send(Msg::Settings(s.game_mode)),
            Err(e) => eprintln!("lucent: bad settings://changed payload: {e}"),
        });
    }

    struct Worker {
        app: AppHandle,
        session: Session,
        gm: GameModeSettings,
        libs: Option<Libraries>,
        own_pid: u32,
        /// Single entry: the exe of the last (hwnd, pid) resolved, so polling the same
        /// foreground window never reopens the game process.
        exe_cache: Option<(isize, u32, String)>,
        /// Plan to put back while game mode holds High performance.
        power_prev: Option<power::Guid>,
    }

    impl Worker {
        fn new(app: AppHandle, gm: GameModeSettings) -> Self {
            let session = Session::new(gm.enabled, |exe: &str| crate::actions::display_name(&AppId::Exe(exe.to_string())));
            Self { app, session, gm, libs: None, own_pid: std::process::id(), exe_cache: None, power_prev: None }
        }

        fn run(mut self, rx: std::sync::mpsc::Receiver<Msg>) {
            let e = self.current_foreground();
            self.publish(e);
            let mut next_poll = Instant::now() + POLL;
            loop {
                let was_active = self.session.is_active();
                let msg = if was_active {
                    match rx.recv_timeout(next_poll.saturating_duration_since(Instant::now())) {
                        Ok(m) => Some(m),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                } else {
                    match rx.recv() {
                        Ok(m) => Some(m),
                        Err(_) => return,
                    }
                };
                let effects = match msg {
                    None => {
                        next_poll = Instant::now() + POLL;
                        self.poll()
                    }
                    Some(Msg::Foreground(h)) => self.foreground(h),
                    Some(Msg::Answer { exe, is_game }) => self.session.step(Input::Answer { exe, is_game }),
                    // Both change how the current foreground classifies (a game already in
                    // the foreground at startup, game mode re-enabled, exe included): re-check it.
                    Some(Msg::Settings(gm)) => {
                        let mut e = self.session.set_enabled(gm.enabled);
                        self.gm = gm;
                        if !self.gm.power_plan {
                            self.restore_power();
                        }
                        e.extend(self.current_foreground());
                        e
                    }
                    Some(Msg::Libraries(l)) => {
                        self.libs = Some(l);
                        self.current_foreground()
                    }
                };
                if !was_active && self.session.is_active() {
                    next_poll = Instant::now() + POLL;
                }
                self.publish(effects);
            }
        }

        /// Exit check, then a foreground re-check: catches monitor moves (no event) and
        /// Lucent's own windows (the hook skips its own process).
        fn poll(&mut self) -> Vec<Effect> {
            let mut out = match process_ids() {
                Some(pids) => self.session.step(Input::Running(pids)),
                None => Vec::new(),
            };
            if self.session.is_active() {
                out.extend(self.current_foreground());
            }
            out
        }

        fn current_foreground(&mut self) -> Vec<Effect> {
            let fg = unsafe { GetForegroundWindow() };
            self.foreground(fg.0 as isize)
        }

        fn foreground(&mut self, hwnd: isize) -> Vec<Effect> {
            match self.foreground_input(hwnd) {
                Some(i) => self.session.step(i),
                None => Vec::new(),
            }
        }

        fn foreground_input(&mut self, hwnd: isize) -> Option<Input> {
            if hwnd == 0 {
                return None;
            }
            let h = HWND(hwnd as *mut _);
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
            if pid == 0 {
                return None;
            }
            let (monitor_id, mrect) = monitor_of(h)?;
            if pid == self.own_pid {
                return Some(Input::Foreground { pid, exe: String::new(), monitor_id, fullscreen: false, class: Class::NotGame });
            }
            let exe = match &self.exe_cache {
                Some((ch, cp, e)) if *ch == hwnd && *cp == pid => e.clone(),
                _ => {
                    let e = crate::wintrack::win::exe_of_pid(pid).map(|p| p.to_lowercase()).unwrap_or_default();
                    self.exe_cache = Some((hwnd, pid, e.clone()));
                    e
                }
            };
            let is_fg = unsafe { GetForegroundWindow() }.0 as isize == hwnd;
            let mut fullscreen = is_fg && monitors::fullscreen_foreground_rect().is_some_and(|r: Rect| r.covers(&mrect));
            let class = if exe.is_empty() {
                Class::Unknown
            } else {
                let default = Libraries::default();
                library::classify(&exe, self.libs.as_ref().unwrap_or(&default), &self.gm.include, &self.gm.exclude)
            };
            // Before the first discovery every library game looks unknown: never ask then.
            if self.libs.is_none() && class == Class::Unknown {
                fullscreen = false;
            }
            Some(Input::Foreground { pid, exe, monitor_id, fullscreen, class })
        }

        fn restore_power(&mut self) {
            if let Some(prev) = self.power_prev.take() {
                power::restore(prev);
            }
        }

        fn publish(&mut self, effects: Vec<Effect>) {
            for e in effects {
                match e {
                    Effect::Ask { exe, name } => {
                        if let Err(err) = self.app.emit("game://ask", AskPayload { exe, name }) {
                            eprintln!("lucent: cannot emit game://ask: {err}");
                        }
                    }
                    Effect::Enter { .. } if self.gm.power_plan && self.power_prev.is_none() => {
                        self.power_prev = power::enter();
                    }
                    Effect::Exit => self.restore_power(),
                    _ => {}
                }
            }
            let now = self.session.state();
            *lock(&ACTIVE_EXE) = self.session.active_exe().map(str::to_string);
            {
                let mut cur = lock(&STATE);
                if *cur == now {
                    return;
                }
                *cur = now.clone();
            }
            crate::dock_host::send(crate::dock_host::Msg::Game(now.clone()));
            crate::hotkey::set_game_focused(now.active && now.focused);
            crate::valorant::watch::on_game_state(&self.app, now.active);
            if let Err(e) = self.app.emit("game://state", now) {
                eprintln!("lucent: cannot emit game://state: {e}");
            }
        }
    }

    /// Device name and rect of the monitor showing most of `h`.
    fn monitor_of(h: HWND) -> Option<(String, Rect)> {
        unsafe {
            let m = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFOEXW::default();
            mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            if !GetMonitorInfoW(m, &mut mi.monitorInfo as *mut MONITORINFO).as_bool() {
                return None;
            }
            let len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
            let r = mi.monitorInfo.rcMonitor;
            Some((String::from_utf16_lossy(&mi.szDevice[..len]), Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }))
        }
    }

    /// Pids of every running process (Toolhelp snapshot: no process is opened). `None`
    /// when the snapshot fails, so a failure never ends a session.
    fn process_ids() -> Option<Vec<u32>> {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
            let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut out = Vec::new();
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    out.push(e.th32ProcessID);
                    if Process32NextW(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
            Some(out).filter(|v| !v.is_empty())
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn process_snapshot_contains_this_process() {
            let pids = super::process_ids().expect("snapshot");
            assert!(pids.contains(&std::process::id()));
        }
    }
}
