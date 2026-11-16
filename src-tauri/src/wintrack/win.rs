//! Win32 adapter: enumeration and change watching. No logic beyond data gathering.
use super::{
    AumidCache, DEBOUNCE_MAX_WAIT, DEBOUNCE_QUIET, RawWindow, debounce_timeout, is_dock_window, is_frame_host,
    is_windowsapps_path,
};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Instant;
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HWND, LPARAM, PROPERTYKEY, TRUE};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PWSTR};

// PKEY_AppUserModel_ID = {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, 5
const PKEY_APPUSERMODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

pub(crate) fn exe_of_pid(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// AUMID of a packaged process (documented query, no memory read). Packaged Win32 apps
/// (Desktop Bridge: Spotify, …) set none on their windows.
fn aumid_of_pid(pid: u32) -> Option<String> {
    use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        // APPLICATION_USER_MODEL_ID_MAX_LENGTH is 130 (including the terminator).
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        let r = GetApplicationUserModelId(h, &mut len, Some(PWSTR(buf.as_mut_ptr())));
        let _ = CloseHandle(h);
        if r != ERROR_SUCCESS {
            return None;
        }
        // `len` counts the terminating NUL.
        let n = (len as usize).min(buf.len());
        let end = buf[..n].iter().position(|&c| c == 0).unwrap_or(n);
        Some(String::from_utf16_lossy(&buf[..end])).filter(|s| !s.is_empty())
    }
}

fn aumid_of_hwnd(hwnd: HWND) -> Option<String> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
        let v = store.GetValue(&PKEY_APPUSERMODEL_ID).ok()?;
        let s = windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc(&v).ok()?;
        let out = s.to_string().ok().filter(|s| !s.is_empty());
        windows::Win32::System::Com::CoTaskMemFree(Some(s.0 as *const _));
        out
    }
}

fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// AUMID of the `Windows.UI.Core.CoreWindow` child hosted by an ApplicationFrameHost frame.
fn uwp_core_window_aumid(frame: HWND) -> Option<String> {
    unsafe extern "system" fn cb(child: HWND, lp: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lp.0 as *mut Option<HWND>) };
        if class_of(child) == "Windows.UI.Core.CoreWindow" {
            *out = Some(child);
            return BOOL(0);
        }
        TRUE
    }
    let mut found: Option<HWND> = None;
    unsafe {
        let _ = EnumChildWindows(Some(frame), Some(cb), LPARAM(&mut found as *mut _ as isize));
    }
    let core = found?;
    aumid_of_hwnd(core)
}

/// Cheap fields only: no process open, no property store, no child enumeration.
fn describe_cheap(hwnd: HWND) -> RawWindow {
    unsafe {
        let visible = IsWindowVisible(hwnd).as_bool();
        let mut w = RawWindow {
            hwnd: hwnd.0 as isize,
            pid: 0,
            title: String::new(),
            class: String::new(),
            visible,
            cloaked: false,
            has_owner: false,
            tool_window: false,
            app_window: false,
            exe_path: None,
            aumid: None,
        };
        if !visible {
            return w; // fails the filter; skip the rest
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut _,
            std::mem::size_of::<u32>() as u32,
        );
        w.cloaked = cloaked != 0;
        if w.cloaked {
            return w;
        }
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, &mut buf);
            w.title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        }
        w.pid = pid_of(hwnd);
        w.class = class_of(hwnd);
        w.has_owner = GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.0.is_null());
        w.tool_window = ex & WS_EX_TOOLWINDOW.0 != 0;
        w.app_window = ex & WS_EX_APPWINDOW.0 != 0;
        w
    }
}

static AUMIDS: LazyLock<Mutex<AumidCache>> = LazyLock::new(Default::default);

fn aumids() -> MutexGuard<'static, AumidCache> {
    AUMIDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Expensive identity lookups, only for windows that passed the filter.
/// A minimised UWP app's `CoreWindow` leaves its frame: fall back to the AUMID remembered
/// for that frame, then to the frame's own (see `AumidCache`). A packaged Win32 window
/// without an AUMID gets its process's.
fn resolve_identity(w: &mut RawWindow) {
    let hwnd = HWND(w.hwnd as *mut _);
    w.exe_path = exe_of_pid(w.pid);
    w.aumid = if w.exe_path.as_deref().is_some_and(is_frame_host) {
        let core = uwp_core_window_aumid(hwnd);
        aumids().resolve(w.hwnd, core, || aumid_of_hwnd(hwnd))
    } else {
        aumid_of_hwnd(hwnd).or_else(|| {
            // Packaged Win32 app without a window AUMID: ask its process.
            w.exe_path.as_deref().filter(|p| is_windowsapps_path(p)).and_then(|_| aumid_of_pid(w.pid))
        })
    };
}

/// Dock-eligible top-level windows, front to back, with identity resolved.
pub fn enumerate(own_pid: u32) -> Vec<RawWindow> {
    unsafe extern "system" fn cb(hwnd: HWND, lp: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lp.0 as *mut Vec<HWND>) };
        out.push(hwnd);
        TRUE
    }
    let mut hwnds: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut hwnds as *mut _ as isize));
    }
    let alive: Vec<isize> = hwnds.iter().map(|h| h.0 as isize).collect();
    aumids().retain_alive(&alive);
    hwnds
        .into_iter()
        .map(describe_cheap)
        .filter(|w| is_dock_window(w, own_pid))
        .map(|mut w| {
            resolve_identity(&mut w);
            w
        })
        .collect()
}

static TX: std::sync::OnceLock<Sender<()>> = std::sync::OnceLock::new();

type ForegroundCallback = Box<dyn Fn(isize) + Send + 'static>;
static FOREGROUND: Mutex<Vec<ForegroundCallback>> = Mutex::new(Vec::new());

/// Calls `cb` with the new foreground window on each `EVENT_SYSTEM_FOREGROUND` seen by the
/// `watch` hook (other processes only). Runs on the hook thread: `cb` must not block.
pub fn on_foreground(cb: impl Fn(isize) + Send + 'static) {
    FOREGROUND.lock().unwrap_or_else(|e| e.into_inner()).push(Box::new(cb));
}

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if hwnd.0.is_null() || id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 {
        return;
    }
    // Top-level windows only. A window already gone has no root (null): let it through,
    // its disappearance matters.
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if !root.0.is_null() && root != hwnd {
        return;
    }
    if event == EVENT_SYSTEM_FOREGROUND {
        for cb in FOREGROUND.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            cb(hwnd.0 as isize);
        }
    }
    if let Some(tx) = TX.get() {
        let _ = tx.send(());
    }
}

/// Spawns the hook thread (own message loop) and a debounce thread. `on_windows`
/// receives a fresh enumeration after each burst of events (50 ms debounce), plus once at start.
pub fn watch(own_pid: u32, on_windows: impl Fn(Vec<RawWindow>) + Send + 'static) {
    let (tx, rx): (Sender<()>, Receiver<()>) = channel();
    if TX.set(tx.clone()).is_err() {
        return; // already watching
    }
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        // Discrete events only: never LOCATIONCHANGE and friends.
        let ranges = [
            (EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE),
            (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        ];
        let hooks: Vec<HWINEVENTHOOK> = ranges
            .iter()
            .map(|&(lo, hi)| SetWinEventHook(lo, hi, None, Some(on_event), 0, 0, flags))
            .collect();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        for h in hooks {
            let _ = UnhookWinEvent(h);
        }
    });
    std::thread::spawn(move || {
        // COM is needed for the property store lookups.
        unsafe { let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); }
        on_windows(enumerate(own_pid));
        while rx.recv().is_ok() {
            let first = Instant::now();
            // Quiet period, but never later than DEBOUNCE_MAX_WAIT after the first event.
            while let Some(t) = debounce_timeout(first.elapsed(), DEBOUNCE_QUIET, DEBOUNCE_MAX_WAIT) {
                match rx.recv_timeout(t) {
                    Ok(()) => {}
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            on_windows(enumerate(own_pid));
        }
    });
}
