//! Win32 shell around `HostCore`: our `Shell_TrayWnd` (+ `TrayNotifyWnd` child) on its own
//! thread, HICON → PNG, Windows 11 "promoted" state, and clicks sent to icon owners.
//!
//! Threading rules (deadlock-free by construction):
//! - the host thread never takes the taskbar guard lock: the guard calls `SHAppBarMessage`,
//!   which lands on our window (`dwData == 0`) and must always be answered;
//! - forwarding to Explorer uses `SMTO_BLOCK`, so no other message re-enters the window
//!   procedure while `CORE` is locked;
//! - `TaskbarCreated` is broadcast from a short-lived thread: a notify message sent from the
//!   host thread to its own window would run the window procedure re-entrantly.
//! - other taskbar-directed messages (`WM_COMMAND`, `WM_USER + n`) are relayed to Explorer's
//!   tray outside any lock; `WM_CLOSE` from anyone is ignored (we stop on `WM_APP_STOP`).
use super::{
    COPYDATA_APPBAR, COPYDATA_NOTIFY, Forwarder, HostCore, Lifecycle, NotifySetting, PRUNE_MS, Route, WM_APP_STOP,
    expand_known_folder, icon_rgba, is_promoted, parse_guid, route, route_stop,
};
use crate::monitors::Rect;
use crate::tray::input::{TrayClick, callback_messages};
use crate::tray::registry::TrayIconView;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP,
    ReleaseDC,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_ROUTINE_FLAGS, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CopyIcon, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow, DispatchMessageW,
    FindWindowExW, GetIconInfo, GetMessageW, GetWindowRect, GetWindowThreadProcessId, HICON, HWND_BROADCAST,
    HWND_TOPMOST, ICONINFO, InSendMessage, IsWindow, KillTimer, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SMTO_ABORTIFHUNG, SMTO_BLOCK, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SendMessageTimeoutW,
    SendNotifyMessageW, SetTimer, SetWindowPos, TranslateMessage, WINDOW_EX_STYLE, WM_COPYDATA, WNDCLASSW, WS_CHILD, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{GUID, PCWSTR, PWSTR, w};

const PRUNE_TIMER: usize = 1;
const EMIT_TIMER: usize = 2;
const FORWARD_TIMEOUT_MS: u32 = 1000;
/// Relaying other taskbar-directed messages: short, the sender is waiting on us.
const RELAY_TIMEOUT_MS: u32 = 500;
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// Tray PNG size before the monitor scale (shown at a smaller CSS size: stays sharp).
const ICON_PX: f32 = 32.0;
/// Half of the on-screen tray icon (CSS px), for `Shell_NotifyIconGetRect` answers.
const ICON_HALF_CSS: i32 = 11;
/// At most one "we were displaced" rebroadcast per this period (all apps re-add on it).
const DISPLACED_REBROADCAST_MS: u64 = 30_000;

static APP: OnceLock<AppHandle> = OnceLock::new();
static CORE: Mutex<Option<HostCore<Win32Forwarder>>> = Mutex::new(None);
static HOST: Mutex<HostSlot> = Mutex::new(HostSlot { life: Lifecycle::new(), hwnd: 0, done: None });
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// `wParam` of the `WM_COPYDATA` being handled (the sender's window), forwarded as is.
static FORWARD_WPARAM: AtomicUsize = AtomicUsize::new(0);
/// Largest monitor scale, as `f32` bits.
static ICON_SCALE: AtomicU32 = AtomicU32::new(0x3F80_0000); // 1.0
static LAST_DISPLACED_MS: AtomicU64 = AtomicU64::new(0);
/// Set by `stop()` right before it posts `WM_APP_STOP`: the only stop request honoured.
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Called (from the dying host thread) when the host ended without `stop()`.
static ON_UNEXPECTED_EXIT: OnceLock<fn()> = OnceLock::new();

/// The running host, if any. `life` is freed by `stop()` or by the host thread itself when
/// it ends for any reason, so `start()` always works again afterwards.
struct HostSlot {
    life: Lifecycle,
    hwnd: isize,
    done: Option<Receiver<()>>,
}

fn host() -> MutexGuard<'static, HostSlot> {
    HOST.lock().unwrap_or_else(|e| e.into_inner())
}

fn core() -> MutexGuard<'static, Option<HostCore<Win32Forwarder>>> {
    CORE.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Handles in `NOTIFYICONDATA` are 32-bit; widen them the way `LongToHandle` does.
fn hwnd_of(h: u32) -> HWND {
    HWND(h as i32 as isize as *mut _)
}

fn taskbar_created_msg() -> u32 {
    let m = TASKBAR_CREATED.load(Ordering::Relaxed);
    if m != 0 {
        return m;
    }
    let m = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    TASKBAR_CREATED.store(m, Ordering::Relaxed);
    m
}

pub struct Win32Forwarder;

impl Forwarder for Win32Forwarder {
    fn forward(&self, dw: usize, bytes: &[u8]) -> isize {
        forward_copydata(dw, bytes)
    }
    fn broadcast_taskbar_created(&self) {
        broadcast_taskbar_created();
    }
}

fn forward_copydata(dw: usize, bytes: &[u8]) -> isize {
    let Some(tray) = crate::taskbar::win::explorer_tray() else { return 0 };
    let cds = COPYDATASTRUCT { dwData: dw, cbData: bytes.len() as u32, lpData: bytes.as_ptr() as *mut _ };
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(
            tray,
            WM_COPYDATA,
            WPARAM(FORWARD_WPARAM.load(Ordering::Relaxed)),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            FORWARD_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    if ok.0 == 0 { 0 } else { result as isize }
}

fn broadcast_taskbar_created() {
    let msg = taskbar_created_msg();
    let _ = std::thread::Builder::new().name("lucent-tray-broadcast".into()).spawn(move || unsafe {
        let _ = SendNotifyMessageW(HWND_BROADCAST, msg, WPARAM(0), LPARAM(0));
    });
}

// ---- Lifecycle ------------------------------------------------------------------------

/// Lets the host emit `tray://icons`. Call once at setup, before `start`.
pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// `cb` runs when the host thread ends without `stop()` (the owner decides whether to
/// restart it: only while the taskbar is still hidden). Must not call `start()` inline.
pub fn on_unexpected_exit(cb: fn()) {
    let _ = ON_UNEXPECTED_EXIT.set(cb);
}

/// Creates our `Shell_TrayWnd` (no-op if running) and pulls every icon to it.
/// Only call while Lucent keeps the Windows taskbar hidden.
pub fn start() {
    let mut slot = host();
    let Some(generation) = slot.life.begin() else { return };
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("lucent-tray-host".into())
        .spawn(move || run(generation, ready_tx, done_tx));
    if let Err(e) = spawned {
        eprintln!("lucent: cannot spawn tray host thread: {e}");
        slot.life.exited(generation);
        return;
    }
    match ready_rx.recv_timeout(STOP_TIMEOUT) {
        Ok(Some(hwnd)) => {
            slot.hwnd = hwnd;
            slot.done = Some(done_rx);
        }
        _ => {
            slot.life.exited(generation);
            eprintln!("lucent: tray host window not created, icons stay in Explorer");
        }
    }
}

/// Destroys our window and gives the icons back to Explorer (no-op if not running).
/// Waits (bounded) for the host thread so a following taskbar restore talks to Explorer.
pub fn stop() {
    let (hwnd, done) = {
        let mut slot = host();
        if slot.life.take().is_none() {
            return;
        }
        (slot.hwnd, slot.done.take())
    };
    STOP_REQUESTED.store(true, Ordering::SeqCst);
    unsafe {
        // Private message, honoured only with STOP_REQUESTED (WM_CLOSE is ignored).
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_APP_STOP, WPARAM(0), LPARAM(0));
    }
    if let Some(done) = done {
        let _ = done.recv_timeout(STOP_TIMEOUT);
    }
}

/// Current visible icons (empty when the host is not running).
pub fn views() -> Vec<TrayIconView> {
    core().as_ref().map(|c| c.views()).unwrap_or_default()
}

fn emit(views: Vec<TrayIconView>) {
    if let Some(app) = APP.get() {
        if let Err(e) = app.emit("tray://icons", views) {
            eprintln!("lucent: cannot emit tray://icons: {e}");
        }
    }
}

fn register_class(name: PCWSTR, proc_: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) {
    unsafe {
        let Ok(hmodule) = GetModuleHandleW(None) else { return };
        let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: hmodule.into(), lpszClassName: name, ..Default::default() };
        // Fails with ERROR_CLASS_ALREADY_EXISTS after a stop/start: fine.
        RegisterClassW(&wc);
    }
}

fn run(generation: u64, ready: Sender<Option<isize>>, done: Sender<()>) {
    taskbar_created_msg();
    refresh_scale();
    *core() = Some(HostCore::new(Win32Forwarder));
    register_class(w!("Shell_TrayWnd"), tray_proc);
    register_class(w!("TrayNotifyWnd"), child_proc);
    let hwnd = unsafe {
        let hinst = GetModuleHandleW(None).ok().map(|h| h.into());
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("Shell_TrayWnd"),
            None,
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            hinst,
            None,
        ) else {
            *core() = None;
            let _ = ready.send(None);
            host().life.exited(generation);
            return;
        };
        let _ = CreateWindowExW(WINDOW_EX_STYLE(0), w!("TrayNotifyWnd"), None, WS_CHILD, 0, 0, 0, 0, Some(hwnd), None, hinst, None);
        place(hwnd);
        SetTimer(Some(hwnd), PRUNE_TIMER, PRUNE_MS as u32, None);
        hwnd
    };
    if ready.send(Some(hwnd.0 as isize)).is_err() {
        // `start()` gave up waiting: nobody owns this window, take it down again.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        *core() = None;
        host().life.exited(generation);
        return;
    }
    if let Some(c) = core().as_mut() {
        c.start(now_ms());
    }
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // Window gone: Explorer is the tray again. Drop our state, empty the dock island,
    // then make every app re-add its icon (now to Explorer).
    *core() = None;
    crate::icons::retain_pngs("tray-", |_| false);
    emit(Vec::new());
    broadcast_taskbar_created();
    // Ended for any reason (stop, or unexpectedly): `start()` must work again.
    let unexpected = host().life.exited(generation);
    let _ = done.send(());
    if unexpected {
        eprintln!("lucent: tray host ended unexpectedly");
        if let Some(cb) = ON_UNEXPECTED_EXIT.get() {
            cb();
        }
    }
}

/// Topmost (ahead of Explorer's tray for `FindWindow`), and the same rect as Explorer's
/// tray so `GetWindowRect(FindWindow("Shell_TrayWnd"))` callers still get the taskbar's
/// geometry. The window is never shown (no `WS_VISIBLE`, no `SWP_SHOWWINDOW`).
fn place(hwnd: HWND) {
    let mut r = RECT::default();
    let explorer = crate::taskbar::win::explorer_tray().filter(|&t| unsafe { GetWindowRect(t, &mut r) }.is_ok());
    unsafe {
        let _ = match explorer {
            Some(_) => SetWindowPos(hwnd, Some(HWND_TOPMOST), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOACTIVATE),
            None => SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE),
        };
    }
}

/// Relays a taskbar-directed message to Explorer's tray, outside any lock. Sent messages
/// wait (bounded) for Explorer's answer; posted ones are posted on.
fn relay(msg: u32, wp: WPARAM, lp: LPARAM, sent: bool) -> isize {
    let Some(tray) = crate::taskbar::win::explorer_tray() else { return 0 };
    unsafe {
        if sent {
            let mut result = 0usize;
            let ok = SendMessageTimeoutW(tray, msg, wp, lp, SMTO_ABORTIFHUNG | SMTO_BLOCK, RELAY_TIMEOUT_MS, Some(&mut result));
            if ok.0 == 0 { 0 } else { result as isize }
        } else {
            let _ = PostMessageW(Some(tray), msg, wp, lp);
            0
        }
    }
}

/// `FindWindow("Shell_TrayWnd")` (what `Shell_NotifyIcon` uses) returns the first window
/// of that class in z-order: true when that is not ours (Explorer raised its taskbar).
fn displaced(own: HWND) -> bool {
    unsafe { FindWindowExW(None, None, w!("Shell_TrayWnd"), None) }.is_ok_and(|first| first != own)
}

fn refresh_scale() {
    let s = crate::monitors::win::enumerate().iter().map(|m| m.scale).fold(1.0f32, f32::max);
    ICON_SCALE.store(s.to_bits(), Ordering::Relaxed);
}

// ---- Window procedures -----------------------------------------------------------------

unsafe extern "system" fn child_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn tray_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let sent = unsafe { InSendMessage() }.as_bool();
    let r = match route(msg, TASKBAR_CREATED.load(Ordering::Relaxed), sent) {
        Route::Stop => route_stop(STOP_REQUESTED.swap(false, Ordering::SeqCst)),
        r => r,
    };
    match r {
        Route::CopyData => LRESULT(unsafe { on_copydata(hwnd, wp, lp) }),
        Route::Timer => {
            on_timer(hwnd, wp.0);
            LRESULT(0)
        }
        Route::Stop => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        Route::Quit => {
            unsafe { PostQuitMessage(0) };
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        Route::Ignore => LRESULT(0),
        Route::TaskbarCreated => {
            // Explorer restarted (its new taskbar is in front of us) or our own broadcast
            // coming back. Raise first so the apps re-adding find our window.
            place(hwnd);
            refresh_scale();
            if let Some(c) = core().as_mut() {
                c.on_taskbar_created(now_ms());
            }
            LRESULT(0)
        }
        Route::Forward => LRESULT(relay(msg, wp, lp, sent)),
        Route::Default => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe fn on_copydata(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> isize {
    let cds = lp.0 as *const COPYDATASTRUCT;
    if cds.is_null() {
        return 0;
    }
    let cds = unsafe { &*cds };
    let bytes: &[u8] = if cds.lpData.is_null() || cds.cbData == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) }
    };
    FORWARD_WPARAM.store(wp.0, Ordering::Relaxed);
    if cds.dwData == COPYDATA_APPBAR {
        // AppBar traffic (also our own taskbar guard's): Explorer answers, no state needed.
        return forward_copydata(cds.dwData, bytes);
    }
    let mut guard = core();
    let Some(c) = guard.as_mut() else { return forward_copydata(cds.dwData, bytes) };
    let now = now_ms();
    let r = c.on_copydata(cds.dwData, bytes, now);
    if cds.dwData == COPYDATA_NOTIFY {
        // The app may destroy its HICON right after `Shell_NotifyIcon` returns: convert now.
        enrich(c);
        arm_emit(hwnd, c, now);
    }
    r
}

fn arm_emit(hwnd: HWND, c: &HostCore<Win32Forwarder>, now: u64) {
    if let Some(ms) = c.emit_due_in(now) {
        unsafe { SetTimer(Some(hwnd), EMIT_TIMER, ms.max(1) as u32, None) };
    }
}

fn on_timer(hwnd: HWND, id: usize) {
    let now = now_ms();
    match id {
        PRUNE_TIMER => {
            // Explorer re-raised its taskbar: icons added meanwhile went to Explorer only.
            // Get back in front and have the apps re-add (rate-limited).
            let displaced = displaced(hwnd);
            // Every tick: back in front if needed, and follow Explorer's tray rect.
            place(hwnd);
            let rebroadcast = displaced && {
                let last = LAST_DISPLACED_MS.load(Ordering::Relaxed);
                (last == 0 || now.saturating_sub(last) >= DISPLACED_REBROADCAST_MS)
                    && LAST_DISPLACED_MS.compare_exchange(last, now.max(1), Ordering::Relaxed, Ordering::Relaxed).is_ok()
            };
            let mut guard = core();
            let Some(c) = guard.as_mut() else { return };
            if rebroadcast {
                c.on_taskbar_created(now);
            }
            if c.prune(|h| unsafe { IsWindow(Some(hwnd_of(h))) }.as_bool(), now) {
                arm_emit(hwnd, c, now);
            }
        }
        EMIT_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), EMIT_TIMER);
            }
            let views = {
                let mut guard = core();
                let Some(c) = guard.as_mut() else { return };
                if c.take_emit(now) {
                    let live: Vec<String> =
                        c.registry().icons().iter().map(|i| format!("tray-{}", i.key.as_string())).collect();
                    crate::icons::retain_pngs("tray-", |k| live.iter().any(|l| l == k));
                    Some(c.views())
                } else {
                    arm_emit(hwnd, c, now);
                    None
                }
            };
            if let Some(v) = views {
                emit(v);
            }
        }
        _ => {}
    }
}

// ---- Icon enrichment: owner, promoted, PNG ---------------------------------------------

fn pid_of(hwnd: u32) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd_of(hwnd), Some(&mut pid)) };
    pid
}

fn enrich(c: &mut HostCore<Win32Forwarder>) {
    let size = (ICON_PX * f32::from_bits(ICON_SCALE.load(Ordering::Relaxed))).round().max(16.0) as u32;
    let mut settings: Option<Vec<NotifySetting>> = None;
    for idx in 0..c.registry().icons().len() {
        let icon = &c.registry().icons()[idx];
        if icon.owner_pid == 0 {
            let pid = pid_of(icon.data.hwnd);
            if pid != 0 {
                let exe = crate::wintrack::win::exe_of_pid(pid);
                let s = settings.get_or_insert_with(read_notify_settings);
                let promoted = is_promoted(s, icon.data.guid, exe.as_deref(), icon.data.uid);
                let icon = &mut c.registry_mut().icons_mut()[idx];
                icon.owner_pid = pid;
                icon.promoted = promoted;
            }
        }
        let icon = &c.registry().icons()[idx];
        if icon.icon_png.is_none() {
            let png =
                unsafe { hicon_to_png(icon.data.hicon, size) }.unwrap_or_else(|| crate::icons::GENERIC_ICON.to_vec());
            crate::icons::register_png(&format!("tray-{}", icon.key.as_string()), png.clone());
            c.set_png(idx, png);
        }
    }
}

/// Copies the (shared USER) HICON, reads its bitmaps, scales to `size` and encodes a PNG.
unsafe fn hicon_to_png(h: u32, size: u32) -> Option<Vec<u8>> {
    if h == 0 {
        return None;
    }
    unsafe {
        let icon = CopyIcon(HICON(h as i32 as isize as *mut _)).ok()?;
        let bits = icon_bits(icon);
        let _ = DestroyIcon(icon);
        let (rgba, w, hgt) = bits?;
        let mut img = image::RgbaImage::from_raw(w, hgt, rgba)?;
        if w != size || hgt != size {
            img = image::imageops::resize(&img, size, size, image::imageops::FilterType::CatmullRom);
        }
        crate::icons::encode_png(img.into_raw(), size, size).ok()
    }
}

unsafe fn icon_bits(icon: HICON) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        let mut ii = ICONINFO::default();
        GetIconInfo(icon, &mut ii).ok()?;
        let mask = if ii.hbmMask.0.is_null() { None } else { read_bitmap(ii.hbmMask) };
        let color = if ii.hbmColor.0.is_null() { None } else { read_bitmap(ii.hbmColor) };
        for b in [ii.hbmMask, ii.hbmColor] {
            if !b.0.is_null() {
                let _ = DeleteObject(b.into());
            }
        }
        match (color, mask) {
            (Some((c, w, h)), m) => {
                let m = m.map(|m| m.0).unwrap_or_default();
                Some((icon_rgba(Some(&c), &m, w, h), w, h))
            }
            // Monochrome: the mask holds AND then XOR, twice the icon height.
            (None, Some((m, w, h2))) if h2 >= 2 => Some((icon_rgba(None, &m, w, h2 / 2), w, h2 / 2)),
            _ => None,
        }
    }
}

/// Any bitmap as 32 bpp top-down BGRA.
unsafe fn read_bitmap(hbm: HBITMAP) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(hbm.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth.unsigned_abs(), bm.bmHeight.unsigned_abs());
        if w == 0 || h == 0 || w > 1024 || h > 2048 {
            return None;
        }
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(dc, hbm, 0, h, Some(buf.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        (lines != 0).then_some((buf, w, h))
    }
}

// ---- HKCU\Control Panel\NotifyIconSettings ---------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn reg_raw(key: HKEY, sub: &[u16], value: PCWSTR, flags: REG_ROUTINE_FLAGS) -> Option<Vec<u8>> {
    unsafe {
        let mut len = 0u32;
        if RegGetValueW(key, PCWSTR(sub.as_ptr()), value, flags, None, None, Some(&mut len)) != ERROR_SUCCESS || len == 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let r = RegGetValueW(key, PCWSTR(sub.as_ptr()), value, flags, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len));
        if r != ERROR_SUCCESS {
            return None;
        }
        buf.truncate(len as usize);
        Some(buf)
    }
}

unsafe fn reg_string(key: HKEY, sub: &[u16], value: PCWSTR) -> Option<String> {
    let b = unsafe { reg_raw(key, sub, value, RRF_RT_REG_SZ) }?;
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    Some(String::from_utf16_lossy(&units))
}

unsafe fn reg_dword(key: HKEY, sub: &[u16], value: PCWSTR) -> Option<u32> {
    let b = unsafe { reg_raw(key, sub, value, RRF_RT_REG_DWORD) }?;
    (b.len() >= 4).then(|| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn known_folder(guid: &str) -> Option<String> {
    let g = parse_guid(guid)?;
    let id = GUID {
        data1: u32::from_le_bytes([g[0], g[1], g[2], g[3]]),
        data2: u16::from_le_bytes([g[4], g[5]]),
        data3: u16::from_le_bytes([g[6], g[7]]),
        data4: [g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]],
    };
    unsafe {
        let p: PWSTR = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Every entry of `HKCU\Control Panel\NotifyIconSettings` (empty before Windows 11).
fn read_notify_settings() -> Vec<NotifySetting> {
    let mut out = Vec::new();
    let mut folders: HashMap<String, Option<String>> = HashMap::new();
    unsafe {
        let mut root = HKEY::default();
        let path = wide(r"Control Panel\NotifyIconSettings");
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), None, KEY_READ, &mut root) != ERROR_SUCCESS {
            return out;
        }
        for i in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(root, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) != ERROR_SUCCESS {
                break;
            }
            let sub: Vec<u16> = name[..len as usize].iter().copied().chain(std::iter::once(0)).collect();
            let exe = reg_string(root, &sub, w!("ExecutablePath")).map(|p| {
                expand_known_folder(&p, |g| folders.entry(g.to_string()).or_insert_with(|| known_folder(g)).clone())
            });
            let guid = reg_string(root, &sub, w!("IconGuid")).and_then(|s| parse_guid(&s)).or_else(|| {
                reg_raw(root, &sub, w!("IconGuid"), RRF_RT_REG_BINARY).filter(|b| b.len() == 16).map(|b| {
                    let mut g = [0u8; 16];
                    g.copy_from_slice(&b);
                    g
                })
            });
            out.push(NotifySetting {
                exe,
                uid: reg_dword(root, &sub, w!("UID")),
                guid,
                promoted: reg_dword(root, &sub, w!("IsPromoted")) == Some(1),
            });
        }
        let _ = RegCloseKey(root);
    }
    out
}

// ---- Clicks ----------------------------------------------------------------------------

/// `tray_click`: `(x, y)` is the icon centre in physical px relative to the dock window
/// `window`. Sends the icon's callback messages, anchored at that point on screen.
pub fn click(window: isize, key: &str, button: TrayClick, x: i32, y: i32) {
    let win = HWND(window as *mut _);
    let mut wr = RECT::default();
    if unsafe { GetWindowRect(win, &mut wr) }.is_err() {
        return;
    }
    let (sx, sy) = (wr.left + x, wr.top + y);
    let dpi = unsafe { GetDpiForWindow(win) }.max(96) as i32;
    let half = ICON_HALF_CSS * dpi / 96;
    let target = {
        let mut guard = core();
        let Some(c) = guard.as_mut() else { return };
        c.set_icon_rect(key, Rect { x: sx - half, y: sy - half, w: 2 * half, h: 2 * half });
        c.click_target(key)
    };
    let Some(t) = target else { return };
    if t.callback_message == 0 {
        return;
    }
    let owner = hwnd_of(t.hwnd);
    let pid = pid_of(t.hwnd);
    unsafe {
        // We received the click (last input event), so we may hand the foreground over:
        // the app's menu or flyout can then take focus and close properly.
        if pid != 0 {
            let _ = AllowSetForegroundWindow(pid);
        }
        for (_, wp, lp) in callback_messages(button, t.version, t.uid, (sx as i16, sy as i16)) {
            let _ = PostMessageW(Some(owner), t.callback_message, WPARAM(wp), LPARAM(lp));
        }
    }
}
