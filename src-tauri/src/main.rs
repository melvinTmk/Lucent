// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use lucent_lib::cli::{parse_args, Mode};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        // `lucent.exe --watchdog <pid>`: taskbar watchdog mode (spawned by the app itself).
        #[cfg(windows)]
        Mode::Watchdog(pid) => lucent_lib::taskbar::watchdog::run_watchdog(pid),
        // `lucent.exe --spawn-watchdog <pid>`: start the watchdog, exit at once.
        #[cfg(windows)]
        Mode::SpawnWatchdog(pid) => {
            let ok = lucent_lib::taskbar::watchdog::spawn_watchdog_for(pid).is_ok();
            std::process::exit(if ok { 0 } else { 1 });
        }
        // `lucent.exe --restore-taskbar`: used by the uninstaller.
        #[cfg(windows)]
        Mode::RestoreTaskbar => lucent_lib::restore_taskbar_and_exit(),
        _ => lucent_lib::run(),
    }
}
