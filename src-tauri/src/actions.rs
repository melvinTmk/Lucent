//! App actions: launch, focus, quit, display names. Win32 adapter over the dock model.
use crate::dock_model::WindowRef;
use crate::settings::AppId;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn file_stem(path: &str) -> String {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => name.to_string(),
    }
}

fn fallback_name(app: &AppId) -> String {
    match app {
        AppId::Exe(p) => file_stem(p),
        AppId::Aumid(a) => a.split(['_', '!']).next().unwrap_or(a).rsplit('.').next().unwrap_or(a).to_string(),
    }
}

/// FileDescription / AppX DisplayName, falling back to the file stem. Memoized.
pub fn display_name(app: &AppId) -> String {
    static CACHE: OnceLock<Mutex<HashMap<AppId, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(n) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(app) {
        return n.clone();
    }
    #[cfg(windows)]
    let found = win::display_name(app);
    #[cfg(not(windows))]
    let found: Option<String> = None;
    let name = found.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| fallback_name(app));
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(app.clone(), name.clone());
    name
}

pub fn launch(app: &AppId) -> Result<(), String> {
    #[cfg(windows)]
    return win::launch(app);
    #[cfg(not(windows))]
    Err(format!("launch unsupported: {app:?}"))
}

/// `launch` on a short-lived worker thread: ShellExecute can block for seconds (slow app,
/// AppsFolder resolution, network shortcut, COM pumping), and callers are often on the main
/// thread, where any block freezes every Lucent window. Errors are only logged.
pub fn launch_detached(app: AppId) {
    let spawned = std::thread::Builder::new().name("lucent-launch".into()).spawn(move || {
        #[cfg(windows)]
        win::com_sta();
        if let Err(e) = launch(&app) {
            eprintln!("lucent: {e}");
        }
    });
    if let Err(e) = spawned {
        eprintln!("lucent: cannot spawn launch thread: {e}");
    }
}

pub fn focus(hwnd: isize) {
    #[cfg(windows)]
    win::focus(hwnd);
    #[cfg(not(windows))]
    let _ = hwnd;
}

pub fn quit(windows: &[WindowRef]) {
    for w in windows {
        #[cfg(windows)]
        win::close(w.hwnd);
        #[cfg(not(windows))]
        let _ = w;
    }
}

/// Force-quits the processes owning `windows` (the taskbar's "End task"). Explicit user
/// action from the context menu only. Lucent itself is never killed, and a UWP app
/// hosted in `ApplicationFrameHost` is closed instead: killing the frame host would take
/// every other UWP window down with it.
pub fn end_task(windows: &[WindowRef]) {
    #[cfg(windows)]
    {
        let owners: Vec<(isize, u32)> = windows.iter().map(|w| (w.hwnd, win::window_pid(w.hwnd))).collect();
        for pid in pids_to_kill(&owners, std::process::id()) {
            if win::is_frame_host_pid(pid) {
                owners.iter().filter(|(_, p)| *p == pid).for_each(|(h, _)| win::close(*h));
            } else if let Err(e) = win::terminate(pid) {
                eprintln!("lucent: cannot end task {pid}: {e}");
            }
        }
    }
    #[cfg(not(windows))]
    let _ = windows;
}

/// Distinct owning pids, in window order, minus unknown (0) and our own.
pub(crate) fn pids_to_kill(owners: &[(isize, u32)], own_pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    for &(_, pid) in owners {
        if pid != 0 && pid != own_pid && !out.contains(&pid) {
            out.push(pid);
        }
    }
    out
}

/// True when `hwnd` belongs to this process (Lucent must never close itself).
pub fn is_own_window(hwnd: isize) -> bool {
    #[cfg(windows)]
    return win::window_pid(hwnd) == std::process::id();
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        false
    }
}

pub fn open_task_manager() {
    launch_detached(AppId::Exe("taskmgr.exe".into()));
}

#[cfg(windows)]
mod win {
    use crate::settings::AppId;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsIconic, PostMessageW, SW_RESTORE, SW_SHOWNORMAL, SetForegroundWindow, ShowWindowAsync, WM_CLOSE,
    };
    use windows::core::{HSTRING, PCWSTR, w};

    /// ShellExecute wants an STA on the calling thread. Thread-local; failures are harmless.
    pub fn com_sta() {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    }

    pub fn window_pid(hwnd: isize) -> u32 {
        let mut pid = 0u32;
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid)) };
        pid
    }

    fn exe_of(pid: u32) -> Option<String> {
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        };
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
            let _ = windows::Win32::Foundation::CloseHandle(h);
            ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    pub fn is_frame_host_pid(pid: u32) -> bool {
        exe_of(pid).is_some_and(|p| crate::wintrack::is_frame_host(&p))
    }

    pub fn terminate(pid: u32) -> Result<(), String> {
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, false, pid).map_err(|e| e.to_string())?;
            let r = TerminateProcess(h, 1).map_err(|e| e.to_string());
            let _ = windows::Win32::Foundation::CloseHandle(h);
            r
        }
    }

    pub fn launch(app: &AppId) -> Result<(), String> {
        let target = match app {
            // Windows refuses to start an exe inside WindowsApps by path: use its AUMID.
            AppId::Exe(p) if crate::wintrack::is_windowsapps_path(p) => {
                match crate::search::apps::aumid_for_packaged_exe(p) {
                    Some(a) => format!("shell:AppsFolder\\{a}"),
                    None => p.clone(),
                }
            }
            AppId::Exe(p) => p.clone(),
            AppId::Aumid(a) => format!("shell:AppsFolder\\{a}"),
        };
        let target = HSTRING::from(target);
        // ShellExecuteW returns an HINSTANCE; values > 32 mean success.
        let r = unsafe { ShellExecuteW(None, w!("open"), &target, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
        if r.0 as isize > 32 { Ok(()) } else { Err(format!("ShellExecute failed ({}) for {app:?}", r.0 as isize)) }
    }

    pub fn focus(hwnd: isize) {
        let h = HWND(hwnd as *mut _);
        unsafe {
            // Async: a synchronous ShowWindow on a hung app's window blocks the caller
            // (often our main thread) until that app answers.
            if IsIconic(h).as_bool() {
                let _ = ShowWindowAsync(h, SW_RESTORE);
            }
            let _ = SetForegroundWindow(h);
        }
    }

    pub fn close(hwnd: isize) {
        let _ = unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }

    pub fn display_name(app: &AppId) -> Option<String> {
        match app {
            AppId::Exe(p) => file_description(p),
            AppId::Aumid(a) => appx_display_name(a),
        }
    }

    fn query_string(block: &[u8], sub: &str) -> Option<String> {
        let sub = HSTRING::from(sub);
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        unsafe {
            if !VerQueryValueW(block.as_ptr() as *const _, &sub, &mut ptr, &mut len).as_bool() || len == 0 {
                return None;
            }
            let s = std::slice::from_raw_parts(ptr as *const u16, len as usize);
            let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
            Some(String::from_utf16_lossy(&s[..end]))
        }
    }

    fn file_description(path: &str) -> Option<String> {
        let wide = HSTRING::from(path);
        unsafe {
            let size = GetFileVersionInfoSizeW(&wide, None);
            if size == 0 {
                return None;
            }
            let mut block = vec![0u8; size as usize];
            GetFileVersionInfoW(&wide, None, size, block.as_mut_ptr() as *mut _).ok()?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            let tr = HSTRING::from("\\VarFileInfo\\Translation");
            let mut langs = vec![];
            if VerQueryValueW(block.as_ptr() as *const _, &tr, &mut ptr, &mut len).as_bool() && len >= 4 {
                let p = ptr as *const u16;
                langs.push((*p, *p.add(1)));
            }
            langs.extend([(0x040C, 0x04B0), (0x0409, 0x04B0), (0x0409, 0x04E4)]);
            langs.into_iter().find_map(|(lang, cp)| {
                query_string(&block, &format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"))
                    .filter(|s| !s.trim().is_empty())
            })
        }
    }

    fn appx_display_name(aumid: &str) -> Option<String> {
        use windows::Management::Deployment::PackageManager;
        let family = aumid.split('!').next()?;
        let pm = PackageManager::new().ok()?;
        let pkgs = pm.FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(family)).ok()?;
        pkgs.into_iter().find_map(|p| p.DisplayName().ok().map(|n| n.to_string()).filter(|n| !n.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pids_to_kill_dedups_and_spares_self_and_unknown() {
        let owners = [(1, 40), (2, 40), (3, 0), (4, 7), (5, 99)];
        assert_eq!(pids_to_kill(&owners, 7), vec![40, 99]);
    }

    #[test]
    fn file_stem_handles_paths() {
        assert_eq!(file_stem(r"c:\windows\explorer.exe"), "explorer");
        assert_eq!(file_stem("noext"), "noext");
    }

    #[test]
    fn aumid_fallback_uses_package_name() {
        assert_eq!(fallback_name(&AppId::Aumid("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into())), "WindowsCalculator");
    }

    /// `cargo test actions -- --ignored --nocapture`
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn live_display_names() {
        let e = display_name(&AppId::Exe(r"c:\windows\explorer.exe".into()));
        let c = display_name(&AppId::Aumid("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()));
        println!("{e} / {c}");
        assert!(!e.is_empty() && !c.is_empty());
    }
}
