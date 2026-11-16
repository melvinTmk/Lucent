//! Files dropped on a dock icon open with that app. Pure helpers (argument quoting,
//! z-order pick) plus the Win32 launch adapter (`ShellExecuteW` args / `ActivateForFile`).
use crate::settings::AppId;

/// Builds the command line for an exe: each path double-quoted, space separated.
/// Refuses an empty list, an empty path and any path containing `"` (it cannot be quoted
/// reliably and would allow argument injection). Backslashes right before the closing
/// quote are doubled so a trailing `\` cannot escape it.
pub fn quote_args(paths: &[String]) -> Result<String, String> {
    if paths.is_empty() {
        return Err("no paths".into());
    }
    let mut out = String::new();
    for p in paths {
        if p.is_empty() || p.contains('\0') {
            return Err("empty or invalid path".into());
        }
        if p.contains('"') {
            return Err(format!("path contains a quote: {p}"));
        }
        if !out.is_empty() {
            out.push(' ');
        }
        let trailing = p.chars().rev().take_while(|&c| c == '\\').count();
        out.push('"');
        out.push_str(p);
        out.push_str(&"\\".repeat(trailing));
        out.push('"');
    }
    Ok(out)
}

/// Absolute Windows path: `X:\…` / `X:/…` or UNC `\\server\share…` (never relative,
/// never a switch like `/c` or `-x`).
pub fn is_absolute_path(p: &str) -> bool {
    let b = p.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    drive || (p.starts_with(r"\\") && p.len() > 2)
}

/// Keeps only the dropped paths that are absolute and exist (`exists`): the page cannot
/// smuggle switches or relative names into the launched app's command line.
pub fn keep_existing_absolute(paths: &[String], exists: impl Fn(&str) -> bool) -> Vec<String> {
    paths.iter().filter(|p| is_absolute_path(p) && exists(p)).cloned().collect()
}

/// The window of `candidates` that is highest in `z_order` (topmost first): the app's most
/// recently used window. Falls back to the first candidate when none is in `z_order`.
pub fn pick_topmost(z_order: &[isize], candidates: &[isize]) -> Option<isize> {
    z_order.iter().copied().find(|h| candidates.contains(h)).or_else(|| candidates.first().copied())
}

/// Spring-loading: brings the most recent window of the hovered app to the front.
pub fn spring(hwnds: &[isize]) {
    #[cfg(windows)]
    {
        let z = win::z_order();
        if let Some(h) = pick_topmost(&z, hwnds) {
            crate::actions::focus(h);
        }
    }
    #[cfg(not(windows))]
    let _ = hwnds;
}

/// Opens `paths` with `app`. Blocking (COM): call from a worker thread.
pub fn open(app: &AppId, paths: &[String]) -> Result<(), String> {
    let paths = keep_existing_absolute(paths, |p| std::path::Path::new(p).exists());
    if paths.is_empty() {
        return Err("no existing absolute path".into());
    }
    #[cfg(windows)]
    return win::open(app, &paths);
    #[cfg(not(windows))]
    {
        let _ = (app, paths);
        Err("unsupported".into())
    }
}

#[cfg(windows)]
mod win {
    use super::quote_args;
    use crate::settings::AppId;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
    use windows::Win32::UI::Shell::{
        ApplicationActivationManager, IApplicationActivationManager, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
        ShellExecuteW,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, SW_SHOWNORMAL};
    use windows::core::{BOOL, HSTRING, PCWSTR, w};

    pub fn z_order() -> Vec<isize> {
        unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
            unsafe { (*(l.0 as *mut Vec<isize>)).push(h.0 as isize) };
            BOOL(1)
        }
        let mut v: Vec<isize> = Vec::new();
        let _ = unsafe { EnumWindows(Some(cb), LPARAM(&mut v as *mut _ as isize)) };
        v
    }

    pub fn open(app: &AppId, paths: &[String]) -> Result<(), String> {
        let aumid = match app {
            AppId::Aumid(a) => Some(a.clone()),
            AppId::Exe(p) if crate::wintrack::is_windowsapps_path(p) => crate::search::apps::aumid_for_packaged_exe(p),
            AppId::Exe(_) => None,
        };
        if let Some(a) = aumid {
            return activate_for_files(&a, paths);
        }
        let AppId::Exe(exe) = app else { return Err("no executable".into()) };
        let args = HSTRING::from(quote_args(paths)?);
        let exe = HSTRING::from(exe.as_str());
        let r = unsafe { ShellExecuteW(None, w!("open"), &exe, &args, PCWSTR::null(), SW_SHOWNORMAL) };
        if r.0 as isize > 32 { Ok(()) } else { Err(format!("ShellExecute failed ({}) for {app:?}", r.0 as isize)) }
    }

    fn activate_for_files(aumid: &str, paths: &[String]) -> Result<(), String> {
        if paths.is_empty() {
            return Err("no paths".into());
        }
        unsafe {
            // S_FALSE / RPC_E_CHANGED_MODE are fine: COM is already usable on this thread.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
            for p in paths {
                let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
                if SHParseDisplayName(&HSTRING::from(p.as_str()), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
                    pidls.push(pidl);
                }
            }
            let result = (|| -> Result<(), String> {
                if pidls.is_empty() {
                    return Err("no valid paths".into());
                }
                let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
                let array = SHCreateShellItemArrayFromIDLists(&raw).map_err(|e| e.to_string())?;
                let mgr: IApplicationActivationManager =
                    CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER).map_err(|e| e.to_string())?;
                mgr.ActivateForFile(&HSTRING::from(aumid), &array, w!("open")).map(|_| ()).map_err(|e| e.to_string())
            })();
            for p in pidls {
                CoTaskMemFree(Some(p as *const _));
            }
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn quotes_each_path_even_with_spaces() {
        assert_eq!(quote_args(&s(&[r"C:\a b\c.txt", r"D:\x.png"])).unwrap(), r#""C:\a b\c.txt" "D:\x.png""#);
    }

    #[test]
    fn refuses_a_quote_empty_list_or_empty_path() {
        assert!(quote_args(&s(&[r#"C:\a"b.txt"#])).is_err());
        assert!(quote_args(&s(&[r"C:\ok.txt", r#"x" /c calc"#])).is_err());
        assert!(quote_args(&[]).is_err());
        assert!(quote_args(&s(&[""])).is_err());
    }

    #[test]
    fn trailing_backslash_cannot_escape_the_closing_quote() {
        assert_eq!(quote_args(&s(&[r"C:\dir\"])).unwrap(), r#""C:\dir\\""#);
    }

    #[test]
    fn keeps_only_existing_absolute_paths() {
        let all = s(&[r"C:\a.txt", r"rel\b.txt", "/c", "-x", r"\\srv\share\c.txt", "D:/d.png", r"C:\gone.txt", "C:", r"\\"]);
        let kept = keep_existing_absolute(&all, |p| p != r"C:\gone.txt");
        assert_eq!(kept, s(&[r"C:\a.txt", r"\\srv\share\c.txt", "D:/d.png"]));
        assert!(keep_existing_absolute(&all, |_| false).is_empty());
    }

    #[test]
    fn topmost_follows_z_order() {
        assert_eq!(pick_topmost(&[9, 5, 3, 1], &[1, 5]), Some(5));
        assert_eq!(pick_topmost(&[9], &[4, 2]), Some(4));
        assert_eq!(pick_topmost(&[9], &[]), None);
    }
}
