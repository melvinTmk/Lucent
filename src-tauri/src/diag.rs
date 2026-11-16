//! Diagnostics for a GUI process that has no console: every `eprintln!` goes to
//! `%LOCALAPPDATA%\Lucent\logs\lucent.log`, and fatal-looking exceptions leave a
//! breadcrumb there (code, thread, module+offset call chain) before the process dies.
//!
//! Rust's stderr looks the handle up on every write (`GetStdHandle`), so pointing
//! `STD_ERROR_HANDLE` at the file once redirects every existing `eprintln!`.
use std::path::PathBuf;

/// The log rolls over to `lucent.old.log` past this size (checked at startup).
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

pub fn log_dir() -> PathBuf {
    crate::paths::cache_dir().join("logs")
}

pub fn log_path() -> PathBuf {
    log_dir().join("lucent.log")
}

/// Local wall-clock time, `YYYY-MM-DD HH:MM:SS.mmm`.
pub fn now() -> String {
    #[cfg(windows)]
    {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        )
    }
    #[cfg(not(windows))]
    {
        format!("{:?}", std::time::SystemTime::now())
    }
}

/// A timestamped line (lifecycle events: start, exit, display change, relaunch...).
pub fn event(msg: impl std::fmt::Display) {
    eprintln!("[{}] [{}] {msg}", now(), std::process::id());
}

/// Redirects stderr to the log file (append). `rotate`: only the main app rolls the file
/// over, helper processes (watchdog) just append. Never fails the caller.
pub fn init(role: &str, rotate: bool) {
    let dir = log_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = log_path();
    if rotate && std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, dir.join("lucent.old.log"));
    }
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
    #[cfg(windows)]
    {
        use std::os::windows::io::IntoRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
        // In a debug build started from a terminal keep the console.
        let has_console = unsafe { windows::Win32::System::Console::GetConsoleWindow() }.0 != std::ptr::null_mut();
        if !has_console {
            // Leaked on purpose: the handle lives as long as the process.
            let h = HANDLE(file.into_raw_handle());
            unsafe {
                let _ = SetStdHandle(STD_ERROR_HANDLE, h);
                let _ = SetStdHandle(STD_OUTPUT_HANDLE, h);
            }
        }
    }
    #[cfg(not(windows))]
    drop(file);
    let args: Vec<String> = std::env::args().skip(1).collect();
    event(format!("---- {role} start, v{} args={args:?}", env!("CARGO_PKG_VERSION")));
}

#[cfg(windows)]
pub use win::install_exception_breadcrumbs;

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS, RtlCaptureStackBackTrace};
    use windows::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW,
    };
    use windows::core::PCWSTR;

    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    /// At most this many breadcrumbs per run (a first-chance storm must not fill the disk).
    const MAX_REPORTS: u32 = 8;
    static REPORTS: AtomicU32 = AtomicU32::new(0);

    /// Codes worth a breadcrumb: CoreMessaging/Composition fail-fast, access violation,
    /// stack overflow, fast fail, illegal instruction. Heap corruption is skipped (logging
    /// allocates).
    fn interesting(code: u32) -> bool {
        matches!(code, 0xE046_4645 | 0xC000_0005 | 0xC000_00FD | 0xC000_0409 | 0xC000_001D | 0xC000_0602)
    }

    /// `module.dll+0xoffset` for a code address.
    fn locate(addr: usize) -> String {
        let mut module = HMODULE::default();
        let ok = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                PCWSTR(addr as *const u16),
                &mut module,
            )
        };
        if ok.is_err() || module.is_invalid() {
            return format!("{addr:#x}");
        }
        let mut buf = [0u16; 260];
        let n = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
        let full = String::from_utf16_lossy(&buf[..n]);
        let name = full.rsplit('\\').next().unwrap_or(&full).to_string();
        format!("{name}+{:#x}", addr.wrapping_sub(module.0 as usize))
    }

    unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        let record = unsafe { info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) };
        let Some(record) = record else { return EXCEPTION_CONTINUE_SEARCH };
        let code = record.ExceptionCode.0 as u32;
        if !interesting(code) || REPORTS.fetch_add(1, Ordering::Relaxed) >= MAX_REPORTS {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("?").to_string();
        let mut frames = [std::ptr::null_mut(); 32];
        let n = unsafe { RtlCaptureStackBackTrace(0, &mut frames, None) } as usize;
        let chain: Vec<String> = frames[..n].iter().map(|&f| locate(f as usize)).collect();
        super::event(format!(
            "exception {code:#010x} at {} on thread '{name}' (first chance)\n    stack: {}",
            locate(record.ExceptionAddress as usize),
            chain.join(" <- ")
        ));
        EXCEPTION_CONTINUE_SEARCH
    }

    /// First in the chain; only observes (always continues the search).
    pub fn install_exception_breadcrumbs() {
        unsafe {
            AddVectoredExceptionHandler(1, Some(handler));
        }
    }
}
