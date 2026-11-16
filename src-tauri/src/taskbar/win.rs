//! Real Win32 taskbar access. Never used from unit tests (they use a fake).
use super::ShellTaskbar;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    ABM_GETSTATE, ABM_SETSTATE, ABS_ALWAYSONTOP, ABS_AUTOHIDE, APPBARDATA, SHAppBarMessage,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowExW, GetMessageW, GetShellWindow, GetWindowThreadProcessId,
    MSG, RegisterClassW, RegisterWindowMessageW, SW_HIDE, SW_SHOWNA, ShowWindow,
    TranslateMessage, WM_DISPLAYCHANGE, WM_ENDSESSION, WM_SETTINGCHANGE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{BOOL, w};

/// Primary taskbar (`Shell_TrayWnd`) plus every `Shell_SecondaryTrayWnd`.
/// Holds no handles: windows are looked up on every call, so it survives explorer restarts.
pub struct Win32Taskbar;

fn appbar_data() -> APPBARDATA {
    APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, ..Default::default() }
}

/// Explorer's primary taskbar. Every `Shell_TrayWnd` lookup goes through here: while the
/// taskbar is hidden, Lucent's tray host owns a window of the same class in front of
/// Explorer's, which a plain `FindWindowW` would return. Only the shell's own window is
/// accepted (owner of `GetShellWindow()`, else a process named `explorer.exe`).
pub fn explorer_tray() -> Option<HWND> {
    let mut candidates = Vec::new();
    let mut prev: Option<HWND> = None;
    let shell_pid = unsafe {
        while let Ok(h) = FindWindowExW(None, prev, w!("Shell_TrayWnd"), None) {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            candidates.push((h.0 as isize, pid));
            prev = Some(h);
        }
        let shell = GetShellWindow();
        let mut pid = 0u32;
        if !shell.0.is_null() {
            GetWindowThreadProcessId(shell, Some(&mut pid));
        }
        (pid != 0).then_some(pid)
    };
    let is_explorer = |pid: u32| {
        crate::wintrack::win::exe_of_pid(pid)
            .is_some_and(|p| p.to_lowercase().ends_with("\\explorer.exe"))
    };
    super::pick_explorer_tray(&candidates, shell_pid, std::process::id(), is_explorer).map(|h| HWND(h as *mut _))
}

fn taskbar_windows() -> Vec<HWND> {
    let mut out = Vec::new();
    unsafe {
        if let Some(h) = explorer_tray() {
            out.push(h);
        }
        let mut prev: Option<HWND> = None;
        while let Ok(h) = FindWindowExW(None, prev, w!("Shell_SecondaryTrayWnd"), None) {
            out.push(h);
            prev = Some(h);
        }
    }
    out
}

impl ShellTaskbar for Win32Taskbar {
    fn available(&self) -> bool {
        explorer_tray().is_some()
    }

    fn autohide(&self) -> bool {
        let mut abd = appbar_data();
        let state = unsafe { SHAppBarMessage(ABM_GETSTATE, &mut abd) } as u32;
        state & ABS_AUTOHIDE != 0
    }

    /// Through the AppBar API so the reserved work area is freed (maximized windows
    /// fill the screen) and given back on restore.
    fn set_autohide(&mut self, on: bool) {
        let mut abd = appbar_data();
        if let Some(h) = explorer_tray() {
            abd.hWnd = h;
        }
        abd.lParam = LPARAM((if on { ABS_AUTOHIDE } else { ABS_ALWAYSONTOP }) as isize);
        unsafe { SHAppBarMessage(ABM_SETSTATE, &mut abd) };
    }

    fn any_visible(&self) -> bool {
        taskbar_windows().into_iter().any(|h| unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(h) }.as_bool())
    }

    fn set_visible(&mut self, visible: bool) {
        let cmd = if visible { SW_SHOWNA } else { SW_HIDE };
        for h in taskbar_windows() {
            unsafe {
                let _ = ShowWindow(h, cmd);
            }
        }
    }
}

// ---- Console close / Ctrl+C -------------------------------------------------------

static CONSOLE_CB: OnceLock<fn()> = OnceLock::new();

unsafe extern "system" fn console_handler(_ctrl_type: u32) -> BOOL {
    if let Some(cb) = CONSOLE_CB.get() {
        cb();
    }
    BOOL(0) // not handled: let the default handler terminate the process
}

/// Calls `cb` on Ctrl+C, Ctrl+Break, console close, logoff and shutdown console events.
pub fn install_console_ctrl_handler(cb: fn()) {
    if CONSOLE_CB.set(cb).is_ok() {
        unsafe {
            let _ = SetConsoleCtrlHandler(Some(console_handler), true);
        }
    }
}

// ---- Shell listener: TaskbarCreated + session end --------------------------------

struct ShellCallbacks {
    on_taskbar_created: fn(),
    on_session_end: fn(),
}

static SHELL_CB: OnceLock<ShellCallbacks> = OnceLock::new();
static DISPLAY_CB: OnceLock<fn()> = OnceLock::new();

/// Calls `cb` on `WM_DISPLAYCHANGE` (monitor plugged/unplugged, resolution or layout
/// change), received by the shell listener window. May be set before or after
/// `spawn_shell_listener`.
pub fn on_display_change(cb: fn()) {
    let _ = DISPLAY_CB.set(cb);
}
static SETTING_CB: OnceLock<fn()> = OnceLock::new();

/// Calls `cb` on `WM_SETTINGCHANGE` (wallpaper, theme…), received by the shell listener
/// window. May be set before or after `spawn_shell_listener`.
pub fn on_setting_change(cb: fn()) {
    let _ = SETTING_CB.set(cb);
}
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static OPEN_SETTINGS_CB: OnceLock<fn()> = OnceLock::new();
static OPEN_SETTINGS: AtomicU32 = AtomicU32::new(0);

/// Calls `cb` when a second launch asks this instance to open its settings
/// (`single_instance::signal_running_instance`). Set before `spawn_shell_listener`.
pub fn on_open_settings(cb: fn()) {
    let _ = OPEN_SETTINGS_CB.set(cb);
}

unsafe extern "system" fn shell_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let created = TASKBAR_CREATED.load(Ordering::Relaxed);
    let open = OPEN_SETTINGS.load(Ordering::Relaxed);
    if open != 0 && msg == open {
        if let Some(cb) = OPEN_SETTINGS_CB.get() {
            cb();
        }
        return LRESULT(0);
    }
    // Breadcrumbs for the log: crashes and freezes were seen around these transitions.
    const WM_POWERBROADCAST: u32 = 0x0218;
    match msg {
        WM_POWERBROADCAST => crate::diag::event(format!("power broadcast {:#x}", wp.0)),
        WM_ENDSESSION => crate::diag::event(format!("session end ({}, {:#x})", wp.0, lp.0)),
        _ => {}
    }
    if let (WM_DISPLAYCHANGE, Some(cb)) = (msg, DISPLAY_CB.get()) {
        cb();
    }
    if let (WM_SETTINGCHANGE, Some(cb)) = (msg, SETTING_CB.get()) {
        cb();
    }
    if let Some(cb) = SHELL_CB.get() {
        if created != 0 && msg == created {
            (cb.on_taskbar_created)();
            return LRESULT(0);
        }
        if msg == WM_ENDSESSION && wp.0 != 0 {
            (cb.on_session_end)();
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Spawns a thread owning a hidden, never-shown top-level window that receives the
/// `TaskbarCreated` broadcast (explorer restarted) and `WM_ENDSESSION`.
/// A message-only (`HWND_MESSAGE`) window would not work: broadcasts skip them.
pub fn spawn_shell_listener(on_taskbar_created: fn(), on_session_end: fn()) {
    if SHELL_CB.set(ShellCallbacks { on_taskbar_created, on_session_end }).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("lucent-shell-listener".into())
        .spawn(|| unsafe {
            TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
            OPEN_SETTINGS.store(crate::single_instance::open_settings_msg(), Ordering::Relaxed);
            let Ok(hmodule) = GetModuleHandleW(None) else { return };
            let class = w!("LucentShellListener");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(shell_wnd_proc),
                hInstance: hmodule.into(),
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                eprintln!("lucent: RegisterClassW failed for shell listener");
                return;
            }
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class,
                w!("Lucent shell listener"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hmodule.into()),
                None,
            );
            if hwnd.is_err() {
                eprintln!("lucent: CreateWindowExW failed for shell listener");
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .expect("spawn shell listener thread");
}
