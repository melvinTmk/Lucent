//! Power menu (lock, sleep, sign out, restart, shut down): the Windows taskbar is hidden, so
//! the Start menu's power button is gone. Entry points (Control Center, dock context menu,
//! search commands) all end up in `perform`, behind a `Backend` so tests never touch the real
//! power APIs.
//!
//! Restart, shut down and sign out are *not* forced: apps may ask to save. The taskbar and the
//! power plan are restored by the normal session-end path (`WM_ENDSESSION` ->
//! `taskbar_glue::restore_for_exit`, see `taskbar::win::spawn_shell_listener`).

/// French strings of the native menu and confirmation boxes.
pub mod text {
    pub const SUBMENU: &str = "Alimentation";
    pub const LOCK: &str = "Verrouiller";
    pub const SLEEP: &str = "Veille";
    pub const SIGN_OUT: &str = "Se déconnecter";
    pub const RESTART: &str = "Redémarrer";
    pub const SHUTDOWN: &str = "Éteindre";
    pub const CONFIRM_RESTART: &str = "Redémarrer maintenant ? Les applications ouvertes vont se fermer.";
    pub const CONFIRM_SHUTDOWN: &str = "Éteindre maintenant ? Les applications ouvertes vont se fermer.";
    pub const CONFIRM_SIGN_OUT: &str = "Se déconnecter maintenant ? Les applications ouvertes vont se fermer.";
    pub const SUB_RUN: &str = "Entrée : exécuter";
    pub const SUB_ASK: &str = "Entrée : demander confirmation";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    Lock,
    Sleep,
    SignOut,
    Restart,
    Shutdown,
}

pub const ALL: [PowerAction; 5] =
    [PowerAction::Lock, PowerAction::Sleep, PowerAction::SignOut, PowerAction::Restart, PowerAction::Shutdown];

impl PowerAction {
    /// The wire name (`power_action` command, menu and search ids).
    pub fn id(self) -> &'static str {
        match self {
            Self::Lock => "lock",
            Self::Sleep => "sleep",
            Self::SignOut => "signout",
            Self::Restart => "restart",
            Self::Shutdown => "shutdown",
        }
    }

    /// Exactly one of the five wire names; anything else (case, spaces included) is rejected.
    pub fn parse(s: &str) -> Option<Self> {
        ALL.into_iter().find(|a| a.id() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lock => text::LOCK,
            Self::Sleep => text::SLEEP,
            Self::SignOut => text::SIGN_OUT,
            Self::Restart => text::RESTART,
            Self::Shutdown => text::SHUTDOWN,
        }
    }

    /// Needs a confirmation: unsaved work may be lost.
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::SignOut | Self::Restart | Self::Shutdown)
    }

    pub fn confirm_text(self) -> Option<&'static str> {
        match self {
            Self::Restart => Some(text::CONFIRM_RESTART),
            Self::Shutdown => Some(text::CONFIRM_SHUTDOWN),
            Self::SignOut => Some(text::CONFIRM_SIGN_OUT),
            _ => None,
        }
    }
}

/// Performs the real thing; replaced by a recorder in tests.
pub trait Backend {
    fn execute(&self, action: PowerAction) -> Result<(), String>;
}

/// The `power_action` command body: parses (rejecting unknown names), then executes.
pub fn run_named(name: &str, backend: &dyn Backend) -> Result<(), String> {
    let action = PowerAction::parse(name).ok_or_else(|| format!("unknown power action {name:?}"))?;
    backend.execute(action)
}

/// Native confirmation flow (dock menu): destructive actions run only when `ask` says yes.
pub fn perform_confirmed(action: PowerAction, ask: &dyn Fn(PowerAction) -> bool, backend: &dyn Backend) -> Result<(), String> {
    if action.is_destructive() && !ask(action) {
        return Ok(());
    }
    backend.execute(action)
}

// ---------- native menu ----------

pub const MENU_PREFIX: &str = "power:";

pub struct MenuSpec {
    pub id: String,
    pub label: &'static str,
}

/// Entries of the « Alimentation » submenu, in order.
pub fn menu_spec() -> Vec<MenuSpec> {
    ALL.into_iter().map(|a| MenuSpec { id: format!("{MENU_PREFIX}{}", a.id()), label: a.label() }).collect()
}

pub fn action_from_menu_id(id: &str) -> Option<PowerAction> {
    id.strip_prefix(MENU_PREFIX).and_then(PowerAction::parse)
}

// ---------- search commands ----------

/// Lowercase, accents removed.
fn fold(s: &str) -> String {
    s.trim().chars().flat_map(char::to_lowercase).map(crate::search::rank::fold).collect()
}

const KEYWORDS: [(PowerAction, &[&str]); 5] = [
    (PowerAction::Shutdown, &["eteindre", "arreter", "shutdown", "shut down"]),
    (PowerAction::Restart, &["redemarrer", "redemarrage", "restart", "reboot"]),
    (PowerAction::Sleep, &["veille", "mettre en veille", "sleep"]),
    (PowerAction::Lock, &["verrouiller", "verrouillage", "lock"]),
    (PowerAction::SignOut, &["deconnexion", "deconnecter", "se deconnecter", "sign out", "signout", "logout", "log out"]),
];

/// Actions whose keyword starts with the (accent-insensitive) query of at least 3 chars.
pub fn match_query(q: &str) -> Vec<PowerAction> {
    let f = fold(q);
    if f.chars().count() < 3 {
        return Vec::new();
    }
    KEYWORDS.iter().filter(|(_, kws)| kws.iter().any(|k| k.starts_with(&f))).map(|(a, _)| *a).collect()
}

// ---------- Windows ----------

#[cfg(windows)]
pub mod win {
    use super::{Backend, PowerAction};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED, HANDLE, LUID};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES,
        TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Power::SetSuspendState;
    use windows::Win32::System::Shutdown::{
        ExitWindowsEx, EWX_LOGOFF, EWX_POWEROFF, EWX_REBOOT, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER, SHUTDOWN_REASON,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDOK, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND, MB_TOPMOST,
    };

    pub struct Real;

    fn enable_shutdown_privilege() -> Result<(), String> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)
                .map_err(|e| format!("OpenProcessToken: {e}"))?;
            let r = (|| {
                let mut luid = LUID::default();
                LookupPrivilegeValueW(PCWSTR::null(), w!("SeShutdownPrivilege"), &mut luid)
                    .map_err(|e| format!("LookupPrivilegeValue: {e}"))?;
                let tp = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
                };
                AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None)
                    .map_err(|e| format!("AdjustTokenPrivileges: {e}"))?;
                // Success is reported even when the privilege was not granted.
                if GetLastError() == ERROR_NOT_ALL_ASSIGNED {
                    return Err("shutdown privilege not held".to_string());
                }
                Ok(())
            })();
            let _ = CloseHandle(token);
            r
        }
    }

    fn exit_windows(flags: windows::Win32::System::Shutdown::EXIT_WINDOWS_FLAGS) -> Result<(), String> {
        let reason = SHUTDOWN_REASON(SHTDN_REASON_MAJOR_OTHER.0 | SHTDN_REASON_FLAG_PLANNED.0);
        // Not EWX_FORCE: applications may ask to save their work.
        unsafe { ExitWindowsEx(flags, reason) }.map_err(|e| format!("ExitWindowsEx: {e}"))
    }

    impl Backend for Real {
        fn execute(&self, action: PowerAction) -> Result<(), String> {
            match action {
                PowerAction::Lock => {
                    crate::corners::win::lock();
                    Ok(())
                }
                PowerAction::Sleep => {
                    // Blocks until the PC wakes up: never call on a UI thread.
                    if !unsafe { SetSuspendState(false, false, false) } {
                        return Err("SetSuspendState failed".into());
                    }
                    Ok(())
                }
                PowerAction::SignOut => exit_windows(EWX_LOGOFF),
                PowerAction::Restart => {
                    enable_shutdown_privilege()?;
                    exit_windows(EWX_REBOOT)
                }
                PowerAction::Shutdown => {
                    enable_shutdown_privilege()?;
                    exit_windows(EWX_POWEROFF)
                }
            }
        }
    }

    /// Runs `action` on its own thread (sleep blocks; native boxes are modal).
    pub fn spawn(action: PowerAction, confirm_first: bool) {
        let spawned = std::thread::Builder::new().name("lucent-power".into()).spawn(move || {
            let ask = |a: PowerAction| confirm_box(a);
            let r = if confirm_first {
                super::perform_confirmed(action, &ask, &Real)
            } else {
                Real.execute(action)
            };
            if let Err(e) = r {
                eprintln!("lucent: power action {} failed: {e}", action.id());
            }
        });
        if let Err(e) = spawned {
            eprintln!("lucent: cannot spawn power thread: {e}");
        }
    }

    /// Native OK/Cancel warning box (French system buttons follow the user's UI language).
    fn confirm_box(action: PowerAction) -> bool {
        let Some(msg) = action.confirm_text() else { return true };
        let text: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
        let title: Vec<u16> = action.label().encode_utf16().chain(std::iter::once(0)).collect();
        let r = unsafe {
            MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OKCANCEL | MB_ICONWARNING | MB_TOPMOST | MB_SETFOREGROUND,
            )
        };
        r == IDOK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Rec(RefCell<Vec<PowerAction>>);
    impl Backend for Rec {
        fn execute(&self, a: PowerAction) -> Result<(), String> {
            self.0.borrow_mut().push(a);
            Ok(())
        }
    }

    #[test]
    fn parses_the_five_names_and_rejects_the_rest() {
        for a in ALL {
            assert_eq!(PowerAction::parse(a.id()), Some(a));
        }
        for bad in ["", "Lock", "LOCK", " lock", "hibernate", "shutdown ", "lock;restart", "power:lock"] {
            assert_eq!(PowerAction::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn run_named_rejects_unknown_without_executing() {
        let r = Rec::default();
        assert!(run_named("hibernate", &r).is_err());
        assert!(r.0.borrow().is_empty());
        run_named("sleep", &r).unwrap();
        assert_eq!(*r.0.borrow(), [PowerAction::Sleep]);
    }

    #[test]
    fn only_restart_shutdown_signout_are_destructive() {
        let d: Vec<_> = ALL.into_iter().filter(|a| a.is_destructive()).collect();
        assert_eq!(d, [PowerAction::SignOut, PowerAction::Restart, PowerAction::Shutdown]);
        for a in ALL {
            assert_eq!(a.confirm_text().is_some(), a.is_destructive());
        }
        assert_eq!(PowerAction::Restart.confirm_text(), Some("Redémarrer maintenant ? Les applications ouvertes vont se fermer."));
    }

    #[test]
    fn native_confirmation_gates_destructive_actions() {
        let r = Rec::default();
        perform_confirmed(PowerAction::Shutdown, &|_| false, &r).unwrap();
        assert!(r.0.borrow().is_empty(), "cancel does nothing");
        perform_confirmed(PowerAction::Shutdown, &|_| true, &r).unwrap();
        perform_confirmed(PowerAction::Lock, &|_| false, &r).unwrap(); // never asked
        assert_eq!(*r.0.borrow(), [PowerAction::Shutdown, PowerAction::Lock]);
    }

    #[test]
    fn menu_has_five_roundtrippable_entries() {
        let spec = menu_spec();
        assert_eq!(spec.len(), 5);
        let labels: Vec<_> = spec.iter().map(|m| m.label).collect();
        assert_eq!(labels, ["Verrouiller", "Veille", "Se déconnecter", "Redémarrer", "Éteindre"]);
        for (m, a) in spec.iter().zip(ALL) {
            assert_eq!(action_from_menu_id(&m.id), Some(a));
        }
        assert_eq!(action_from_menu_id("power:nope"), None);
        assert_eq!(action_from_menu_id("dock:lock"), None);
    }

    #[test]
    fn search_matches_accent_insensitively() {
        assert_eq!(match_query("éteindre"), [PowerAction::Shutdown]);
        assert_eq!(match_query("ETEINDRE"), [PowerAction::Shutdown]);
        assert_eq!(match_query("arrêter"), [PowerAction::Shutdown]);
        assert_eq!(match_query("redémarrer"), [PowerAction::Restart]);
        assert_eq!(match_query("redem"), [PowerAction::Restart]);
        assert_eq!(match_query("veille"), [PowerAction::Sleep]);
        assert_eq!(match_query("verrouiller"), [PowerAction::Lock]);
        assert_eq!(match_query("déconnexion"), [PowerAction::SignOut]);
        assert_eq!(match_query("se déconnecter"), [PowerAction::SignOut]);
        assert!(match_query("ve").is_empty(), "too short");
        assert!(match_query("discord").is_empty());
        assert!(match_query("").is_empty());
    }
}
