//! Watchdog: a second `lucent.exe --watchdog <pid>` process that restores the taskbar
//! if the main process dies without doing it (killed from Task Manager, crash, ...).
use super::Marker;

/// Restore only if the parent is gone *and* the marker says the taskbar is still hidden.
/// A clean exit restores and deletes the marker first, so the watchdog then does nothing.
pub fn should_restore(parent_alive: bool, marker: Option<Marker>) -> bool {
    !parent_alive && marker.is_some_and(|m| m.hidden)
}

/// The app died abnormally (fail-fast, access violation, panic abort...): its exit code is
/// an NTSTATUS error or a custom exception code (`0xC.......`, `0xE.......`). A clean quit
/// exits with 0; "End task" and the installer's kill use `TerminateProcess(.., 1)`.
pub fn is_crash_exit(code: u32) -> bool {
    code & 0xC000_0000 == 0xC000_0000
}

/// Relaunch window: at most `MAX_RELAUNCHES` within `RELAUNCH_WINDOW_SECS`, so a crash
/// right at startup cannot loop forever.
pub const MAX_RELAUNCHES: usize = 3;
pub const RELAUNCH_WINDOW_SECS: u64 = 600;

/// Keeps the relaunch times still inside the window and says whether one more is allowed.
pub fn relaunch_allowed(history: &[u64], now: u64) -> (bool, Vec<u64>) {
    let recent: Vec<u64> = history.iter().copied().filter(|&t| now.saturating_sub(t) < RELAUNCH_WINDOW_SECS && t <= now).collect();
    (recent.len() < MAX_RELAUNCHES, recent)
}

/// Every app re-adds its tray icon (to Explorer, once Lucent's tray window is gone).
#[cfg(windows)]
pub fn broadcast_taskbar_created() {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, RegisterWindowMessageW, SendNotifyMessageW};
    unsafe {
        let msg = RegisterWindowMessageW(windows::core::w!("TaskbarCreated"));
        let _ = SendNotifyMessageW(HWND_BROADCAST, msg, WPARAM(0), LPARAM(0));
    }
}

/// Blocks until `parent_pid` exits, then restores the taskbar if needed, then exits.
#[cfg(windows)]
pub fn run_watchdog(parent_pid: u32) -> ! {
    use super::win::Win32Taskbar;
    use super::{TaskbarGuard, marker_path, read_marker};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        WaitForSingleObject,
    };

    crate::diag::init("watchdog", false);
    let mut exit_code: Option<u32> = None;
    unsafe {
        // If OpenProcess fails the parent is already gone: fall through and check.
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, false, parent_pid) {
            WaitForSingleObject(h, INFINITE);
            let mut code = 0u32;
            if GetExitCodeProcess(h, &mut code).is_ok() {
                exit_code = Some(code);
            }
            let _ = CloseHandle(h);
        }
    }
    crate::diag::event(format!("watchdog: app {parent_pid} exited with {:?}", exit_code.map(|c| format!("{c:#010x}"))));
    let path = marker_path();
    if should_restore(false, read_marker(&path)) {
        let mut guard = TaskbarGuard::new(Win32Taskbar, path.clone());
        guard.recover_on_startup();
        // Explorer down: the restore stays pending (marker kept). Wait up to 60 s for
        // the shell to come back; otherwise next startup recovers from the marker.
        for _ in 0..120 {
            if !path.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            guard.on_explorer_restarted();
        }
        // The dead app's tray window is gone: make every app re-add its icon to Explorer.
        if !path.exists() {
            broadcast_taskbar_created();
        }
    }
    if exit_code.is_some_and(is_crash_exit) {
        relaunch_after_crash();
    }
    std::process::exit(0)
}

/// Starts the app again after a crash, unless Windows is shutting down, the exe is gone
/// (uninstall/upgrade) or it already crashed too often lately.
#[cfg(windows)]
fn relaunch_after_crash() {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SHUTTINGDOWN};
    if unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) } != 0 {
        crate::diag::event("watchdog: session ending, no relaunch");
        return;
    }
    if !std::env::current_exe().is_ok_and(|e| e.exists()) {
        return;
    }
    let file = crate::paths::cache_dir().join("crash-relaunches.json");
    let history: Vec<u64> = std::fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (allowed, mut recent) = relaunch_allowed(&history, now);
    if !allowed {
        crate::diag::event(format!("watchdog: {} crashes in {RELAUNCH_WINDOW_SECS} s, giving up", recent.len()));
        return;
    }
    recent.push(now);
    let _ = std::fs::write(&file, serde_json::to_string(&recent).unwrap_or_default());
    // Let whatever killed it settle (display/DWM reset, sleep transition).
    std::thread::sleep(std::time::Duration::from_secs(3));
    match spawn_detached(&[crate::cli::RELAUNCH_FLAG]) {
        Ok(_) => crate::diag::event("watchdog: app relaunched after crash"),
        Err(e) => crate::diag::event(format!("watchdog: relaunch failed: {e}")),
    }
}

/// `<current exe> <args>`, detached, without a console window, out of our job if allowed.
#[cfg(windows)]
pub fn spawn_detached(args: &[&str]) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;

    let exe = std::env::current_exe()?;
    let spawn = |flags: u32| {
        Command::new(&exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    // Try to leave our job object (a kill-on-close job, e.g. a dev runner, would kill
    // the watchdog with us); fall back when the job forbids breakaway.
    // Under `cargo run` / `tauri dev` the job usually forbids breakaway, so the fallback
    // watchdog dies with that job: test the watchdog with the exe launched directly.
    match spawn(base | CREATE_BREAKAWAY_FROM_JOB) {
        Ok(c) => Ok(c),
        Err(e) => {
            eprintln!("lucent: job breakaway refused ({e}), spawning {args:?} inside the job");
            spawn(base)
        }
    }
}

/// App side: starts `<exe> --spawn-watchdog <our pid>` and waits (bounded) for it to report
/// that the watchdog is running. The middle process exits at once, so the watchdog's
/// parent is dead: Task Manager does not group it under Lucent, and "End task" on
/// Lucent (which ends the app's process tree) leaves it alive to restore the taskbar.
#[cfg(windows)]
pub fn spawn_watchdog() -> std::io::Result<()> {
    let pid = std::process::id().to_string();
    let mut middle = spawn_detached(&["--spawn-watchdog", &pid])?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(status) = middle.try_wait()? {
            return if status.success() {
                eprintln!("lucent: taskbar watchdog spawned");
                Ok(())
            } else {
                Err(std::io::Error::other(format!("watchdog spawner failed: {status}")))
            };
        }
        if std::time::Instant::now() >= deadline {
            let _ = middle.kill();
            return Err(std::io::Error::other("watchdog spawner timed out"));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// `--spawn-watchdog <pid>` mode: start `<exe> --watchdog <pid>` and return; the caller
/// exits right away (exit code 0 = watchdog started).
#[cfg(windows)]
pub fn spawn_watchdog_for(parent_pid: u32) -> std::io::Result<()> {
    spawn_detached(&["--watchdog", &parent_pid.to_string()]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_codes() {
        assert!(is_crash_exit(0xE046_4645)); // CoreMessaging fail-fast
        assert!(is_crash_exit(0xC000_0409)); // fast fail / panic abort
        assert!(is_crash_exit(0xC000_0005)); // access violation
        assert!(!is_crash_exit(0)); // clean quit
        assert!(!is_crash_exit(1)); // End task / installer kill
        assert!(!is_crash_exit(0x4000_0000));
    }

    #[test]
    fn relaunch_budget() {
        let now = 10_000;
        assert_eq!(relaunch_allowed(&[], now), (true, vec![]));
        assert_eq!(relaunch_allowed(&[now - 10, now - 20], now), (true, vec![now - 10, now - 20]));
        assert!(!relaunch_allowed(&[now - 10, now - 20, now - 30], now).0);
        // Old entries fall out of the window; future ones (clock change) are dropped.
        assert_eq!(relaunch_allowed(&[now - 700, now - 10, now + 50], now), (true, vec![now - 10]));
    }
}
