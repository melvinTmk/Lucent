//! One Lucent per session. Two instances would each own a `Shell_TrayWnd`, fight over
//! the taskbar and stack two docks per monitor. Helper modes (`--watchdog`,
//! `--spawn-watchdog`, `--restore-taskbar`) never come here (see `cli::Mode`).
#![cfg(windows)]
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, PostMessageW, RegisterWindowMessageW,
};
use windows::core::w;

/// Takes the session-wide `Local\Lucent` mutex. `false` when another instance holds it.
/// The handle is deliberately leaked: the mutex is held until the process ends.
pub fn acquire() -> bool {
    unsafe {
        match CreateMutexW(None, false, w!("Local\\Lucent")) {
            // The handle is valid in both cases; ERROR_ALREADY_EXISTS tells who was first.
            Ok(_) => GetLastError() != ERROR_ALREADY_EXISTS,
            // Cannot create it at all: better run than refuse to start.
            Err(e) => {
                eprintln!("lucent: cannot create single-instance mutex: {e}");
                true
            }
        }
    }
}

/// Registered message the running instance answers by opening its settings window.
pub fn open_settings_msg() -> u32 {
    unsafe { RegisterWindowMessageW(w!("Lucent.OpenSettings")) }
}

/// Asks the running instance (through its shell listener window) to open Réglages.
pub fn signal_running_instance() {
    unsafe {
        let Ok(hwnd) = FindWindowW(w!("LucentShellListener"), None) else {
            eprintln!("lucent: already running, but its window was not found");
            return;
        };
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        // Let it bring the settings window to the front.
        if pid != 0 {
            let _ = AllowSetForegroundWindow(pid);
        }
        let _ = PostMessageW(Some(hwnd), open_settings_msg(), WPARAM(0), LPARAM(0));
    }
}
