//! Launch at startup: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Lucent`.
use std::path::Path;

const VALUE_NAME: &str = "Lucent";

/// The Run value: the quoted exe path followed by `--autostart`.
pub fn run_value(exe: &Path) -> String {
    format!("\"{}\" --autostart", exe.display())
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Action {
    Write,
    Delete,
    Skip,
}

/// Debug builds never write the Run key (a dev exe must not be registered for login);
/// deleting is always allowed.
pub fn autostart_action(on: bool, debug: bool) -> Action {
    match (on, debug) {
        (true, false) => Action::Write,
        (true, true) => Action::Skip,
        (false, _) => Action::Delete,
    }
}

/// Last applied value (-1 = never applied), so saves that do not change it skip the registry.
static LAST: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(-1);

/// Applies `on` once per change (and once at startup). Errors are logged.
pub fn set_autostart(on: bool) {
    use std::sync::atomic::Ordering;
    if LAST.swap(on as i8, Ordering::SeqCst) == on as i8 {
        return;
    }
    match autostart_action(on, cfg!(debug_assertions)) {
        Action::Skip => eprintln!("lucent: debug build, not registering autostart"),
        #[cfg(windows)]
        Action::Write => win::set(true),
        #[cfg(windows)]
        Action::Delete => win::set(false),
        #[cfg(not(windows))]
        _ => {}
    }
}

#[cfg(windows)]
mod win {
    use super::{run_value, VALUE_NAME};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
        RegOpenKeyExW, RegSetValueExW,
    };
    use windows::core::{PCWSTR, w};

    pub fn set(on: bool) {
        unsafe {
            let mut key = HKEY::default();
            let st = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                w!(r"Software\Microsoft\Windows\CurrentVersion\Run"),
                None,
                KEY_SET_VALUE,
                &mut key,
            );
            if st != ERROR_SUCCESS {
                eprintln!("lucent: cannot open Run key: {st:?}");
                return;
            }
            // The former name's entry would start a second dock (Glassdock) at sign-in.
            let legacy: Vec<u16> = "Glassdock".encode_utf16().chain(Some(0)).collect();
            let _ = RegDeleteValueW(key, PCWSTR(legacy.as_ptr()));
            let name: Vec<u16> = VALUE_NAME.encode_utf16().chain(Some(0)).collect();
            if on {
                match std::env::current_exe() {
                    Ok(exe) => {
                        let v: Vec<u16> = run_value(&exe).encode_utf16().chain(Some(0)).collect();
                        let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2);
                        let st = RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes));
                        if st != ERROR_SUCCESS {
                            eprintln!("lucent: cannot write Run value: {st:?}");
                        }
                    }
                    Err(e) => eprintln!("lucent: current_exe failed: {e}"),
                }
            } else {
                let st = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
                if st != ERROR_SUCCESS && st != ERROR_FILE_NOT_FOUND {
                    eprintln!("lucent: cannot delete Run value: {st:?}");
                }
            }
            let _ = RegCloseKey(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_builds_never_write_but_may_delete() {
        assert_eq!(autostart_action(true, false), Action::Write);
        assert_eq!(autostart_action(true, true), Action::Skip);
        assert_eq!(autostart_action(false, true), Action::Delete);
        assert_eq!(autostart_action(false, false), Action::Delete);
    }

    #[test]
    fn run_value_quotes_path_and_flags_autostart() {
        assert_eq!(
            run_value(Path::new(r"C:\A B\lucent.exe")),
            "\"C:\\A B\\lucent.exe\" --autostart"
        );
    }
}
