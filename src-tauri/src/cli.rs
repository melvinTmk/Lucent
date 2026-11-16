//! Command-line modes of `lucent.exe`.

/// Run-key launch at sign-in (`autostart::run_value`).
pub const AUTOSTART_FLAG: &str = "--autostart";
/// Relaunch by the watchdog after a crash.
pub const RELAUNCH_FLAG: &str = "--after-crash";

/// Launched by the system or by our watchdog rather than by the user: such a launch must
/// stay silent when an instance already runs (no Réglages window popping up).
pub fn is_unattended<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter().any(|a| a.as_ref() == AUTOSTART_FLAG || a.as_ref() == RELAUNCH_FLAG)
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Mode {
    /// `--watchdog <pid>`: taskbar watchdog (spawned by the app itself).
    Watchdog(u32),
    /// `--spawn-watchdog <pid>`: short-lived middle process that starts the watchdog for
    /// `<pid>` and exits at once, so the watchdog has no live parent (Task Manager does
    /// not group it under the app, and "End task" on the app does not kill it).
    SpawnWatchdog(u32),
    /// `--restore-taskbar`: restore the taskbar from the marker, then exit (used by the uninstaller).
    RestoreTaskbar,
    /// Normal app start (also `--autostart`).
    App,
}

impl Mode {
    /// Only the app itself is single-instance; helper modes must run alongside it.
    pub fn takes_instance_lock(self) -> bool {
        self == Mode::App
    }
}

/// `args` excludes the program name. A malformed `--watchdog`/`--spawn-watchdog` falls
/// back to `App` (which then meets the single-instance lock).
pub fn parse_args<S: AsRef<str>>(args: &[S]) -> Mode {
    match args.first().map(|s| s.as_ref()) {
        Some("--restore-taskbar") => Mode::RestoreTaskbar,
        Some("--watchdog") => match args.get(1).and_then(|s| s.as_ref().parse::<u32>().ok()) {
            Some(pid) => Mode::Watchdog(pid),
            None => Mode::App,
        },
        Some("--spawn-watchdog") => match args.get(1).and_then(|s| s.as_ref().parse::<u32>().ok()) {
            Some(pid) => Mode::SpawnWatchdog(pid),
            None => Mode::App,
        },
        _ => Mode::App,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unattended_launches() {
        assert!(is_unattended(&["--autostart"]));
        assert!(is_unattended(&["--after-crash"]));
        assert!(!is_unattended::<&str>(&[]));
        assert_eq!(parse_args(&["--after-crash"]), Mode::App);
    }

    #[test]
    fn restore_flag() {
        assert_eq!(parse_args(&["--restore-taskbar"]), Mode::RestoreTaskbar);
    }
    #[test]
    fn watchdog_with_pid() {
        assert_eq!(parse_args(&["--watchdog", "1234"]), Mode::Watchdog(1234));
    }
    #[test]
    fn watchdog_bad_pid_is_app() {
        assert_eq!(parse_args(&["--watchdog", "x"]), Mode::App);
        assert_eq!(parse_args(&["--watchdog"]), Mode::App);
    }
    #[test]
    fn spawn_watchdog_with_pid() {
        assert_eq!(parse_args(&["--spawn-watchdog", "77"]), Mode::SpawnWatchdog(77));
        assert_eq!(parse_args(&["--spawn-watchdog", "x"]), Mode::App);
        assert_eq!(parse_args(&["--spawn-watchdog"]), Mode::App);
    }
    #[test]
    fn only_the_app_takes_the_single_instance_lock() {
        assert!(Mode::App.takes_instance_lock());
        assert!(!Mode::Watchdog(1).takes_instance_lock());
        assert!(!Mode::SpawnWatchdog(1).takes_instance_lock());
        assert!(!Mode::RestoreTaskbar.takes_instance_lock());
    }
    #[test]
    fn default_and_autostart() {
        assert_eq!(parse_args::<&str>(&[]), Mode::App);
        assert_eq!(parse_args(&["--autostart"]), Mode::App);
    }
}
