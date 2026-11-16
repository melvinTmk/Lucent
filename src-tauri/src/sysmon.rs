//! System monitor for the Game Mode pill (Task 6): CPU, GPU, RAM, ping and WSL memory,
//! plus the `wsl_free` / `wsl_shutdown` actions.
//!
//! Sampling runs only while acquired (ref-counted `acquire`/`release`), on its own thread,
//! at most once per second, and the thread ends after the last release. Emits
//! `sysmon://stats` (`SysStats`, camelCase).
//!
//! Vanguard: no handle on any game. The only process opened is the WSL VM (`vmmemWSL` /
//! `vmmem`) with PROCESS_QUERY_LIMITED_INFORMATION for its working set. NVML is loaded into
//! our own process.
//!
//! WSL: Lucent never starts WSL. Every `wsl.exe` action first checks
//! `wsl.exe -l --running -q` and targets a distro that is already running.
use serde::Serialize;
use std::sync::mpsc::Sender;
use std::sync::{Mutex, MutexGuard};
use tauri::AppHandle;

/// Physical memory in MiB.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct RamStats {
    pub used: u64,
    pub total: u64,
}

/// Payload of `sysmon://stats` (mirror of `SysStats` in `src/shared/types.ts`).
/// `cpu`/`gpu` in %, `gpuTemp` in °C, `ram` and `wslRamMb` in MiB.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SysStats {
    pub cpu: f32,
    pub ram: RamStats,
    pub gpu: Option<f32>,
    pub gpu_temp: Option<f32>,
    pub ping_ms: Option<u32>,
    pub wsl_ram_mb: Option<u64>,
}

pub const EVENT: &str = "sysmon://stats";
/// Sampling period (≤ 1 Hz).
pub const PERIOD: std::time::Duration = std::time::Duration::from_secs(1);
/// ICMP echo timeout.
pub const PING_TIMEOUT_MS: u32 = 1000;

/// CPU usage in % between two `GetSystemTimes` samples `(idle, kernel, user)`; kernel time
/// includes idle time. Counters going backwards or an empty interval give 0.
pub fn cpu_percent(prev: (u64, u64, u64), now: (u64, u64, u64)) -> f32 {
    let (Some(idle), Some(kernel), Some(user)) =
        (now.0.checked_sub(prev.0), now.1.checked_sub(prev.1), now.2.checked_sub(prev.2))
    else {
        return 0.0;
    };
    let Some(total) = kernel.checked_add(user).filter(|&t| t > 0) else { return 0.0 };
    let busy = total.saturating_sub(idle);
    ((busy as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32
}

/// Ping on every second tick (tick 0 included, so the first sample has a value).
pub fn ping_due(tick: u64) -> bool {
    tick.is_multiple_of(2)
}

/// The target as an IPv4 literal (no name resolution needed).
pub fn literal_ipv4(s: &str) -> Option<[u8; 4]> {
    s.trim().parse::<std::net::Ipv4Addr>().ok().map(|a| a.octets())
}

/// Rank of a process name as the WSL VM: 2 = `vmmemWSL`, 1 = `vmmem`, 0 = not it.
pub fn wsl_vm_rank(name: &str) -> u8 {
    let n = name.trim();
    let n = n.strip_suffix(".exe").or_else(|| n.strip_suffix(".EXE")).unwrap_or(n);
    if n.eq_ignore_ascii_case("vmmemWSL") {
        2
    } else if n.eq_ignore_ascii_case("vmmem") {
        1
    } else {
        0
    }
}

pub fn is_wsl_vm_process(name: &str) -> bool {
    wsl_vm_rank(name) > 0
}

pub fn mb(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

pub const NOT_RUNNING: &str = "WSL n'est pas lancé";

/// `ok` is `None` when nothing was attempted (no running distro).
pub fn free_message(running: &[String], ok: Option<bool>) -> String {
    match (running.is_empty(), ok) {
        (true, _) | (false, None) => NOT_RUNNING.into(),
        (false, Some(true)) => "Cache mémoire de WSL libéré".into(),
        (false, Some(false)) => "Impossible de libérer la mémoire de WSL".into(),
    }
}

pub fn shutdown_message(running: &[String], ok: Option<bool>) -> String {
    match (running.is_empty(), ok) {
        (true, _) | (false, None) => NOT_RUNNING.into(),
        (false, Some(true)) => format!("WSL arrêté ({})", running.join(", ")),
        (false, Some(false)) => "Impossible d'arrêter WSL".into(),
    }
}

/// Ref count around the sampling thread: the first acquire starts it, the last release
/// hands back its handle so the caller stops it.
pub struct Sampler<H> {
    count: usize,
    handle: Option<H>,
}

impl<H> Sampler<H> {
    pub const fn new() -> Self {
        Sampler { count: 0, handle: None }
    }

    pub fn acquire(&mut self, start: impl FnOnce() -> Option<H>) {
        self.count += 1;
        if self.count == 1 && self.handle.is_none() {
            self.handle = start();
        }
    }

    /// The handle to stop, on the last release only.
    pub fn release(&mut self) -> Option<H> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        if self.count == 0 { self.handle.take() } else { None }
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

impl<H> Default for Sampler<H> {
    fn default() -> Self {
        Self::new()
    }
}

/// Dropping the sender stops the sampling thread (it sees the channel disconnect).
static SAMPLER: Mutex<Sampler<Sender<()>>> = Mutex::new(Sampler::new());

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts sampling if this is the first holder.
pub fn acquire(app: &AppHandle) {
    lock(&SAMPLER).acquire(|| spawn(app));
}

/// Stops sampling when the last holder releases.
pub fn release() {
    let stop = lock(&SAMPLER).release();
    drop(stop);
}

/// Alias of [`acquire`].
pub fn start_sampling(app: &AppHandle) {
    acquire(app);
}

/// Alias of [`release`].
pub fn stop_sampling() {
    release();
}

#[cfg(windows)]
fn spawn(app: &AppHandle) -> Option<Sender<()>> {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let app = app.clone();
    match std::thread::Builder::new().name("lucent-sysmon".into()).spawn(move || win::run(app, rx)) {
        Ok(_) => Some(tx),
        Err(e) => {
            eprintln!("lucent: cannot spawn sysmon thread: {e}");
            None
        }
    }
}

#[cfg(not(windows))]
fn spawn(_app: &AppHandle) -> Option<Sender<()>> {
    None
}

/// `wsl_free`: drop the Linux page cache of a running distro (as root). Blocking.
pub fn wsl_free() -> String {
    let running = crate::search::wsl::running_distros();
    let Some(distro) = pick_distro(&running) else { return free_message(&running, None) };
    #[cfg(windows)]
    let ok = crate::search::wsl::win::run_hidden(
        "wsl.exe",
        &["-d", &distro, "-u", "root", "-e", "sh", "-c", "sync; echo 1 > /proc/sys/vm/drop_caches"],
    )
    .is_some();
    #[cfg(not(windows))]
    let ok = {
        let _ = distro;
        false
    };
    free_message(&running, Some(ok))
}

/// `wsl_shutdown`: `wsl.exe --shutdown`, only when something runs. Blocking. The UI asks
/// for confirmation first.
pub fn wsl_shutdown() -> String {
    let running = crate::search::wsl::running_distros();
    if running.is_empty() {
        return shutdown_message(&running, None);
    }
    #[cfg(windows)]
    let ok = crate::search::wsl::win::run_hidden("wsl.exe", &["--shutdown"]).is_some();
    #[cfg(not(windows))]
    let ok = false;
    shutdown_message(&running, Some(ok))
}

/// The default distro when it runs, else the first running one (never a stopped distro:
/// `wsl.exe` without `-d` would boot the default one).
fn pick_distro(running: &[String]) -> Option<String> {
    let default = crate::search::wsl::default_distro();
    default
        .and_then(|d| running.iter().find(|r| r.eq_ignore_ascii_case(&d)).cloned())
        .or_else(|| running.first().cloned())
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::settings::SettingsStore;
    use std::ffi::c_void;
    use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
    use std::time::Instant;
    use tauri::{Emitter, Manager};
    use windows::Win32::Foundation::{CloseHandle, FILETIME, FreeLibrary, HANDLE, HMODULE};
    use windows::Win32::NetworkManagement::IpHelper::{ICMP_ECHO_REPLY, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho};
    use windows::Win32::Networking::WinSock::{
        ADDRINFOW, AF_INET, FreeAddrInfoW, GetAddrInfoW, SOCKADDR_IN, WSACleanup, WSADATA, WSAStartup,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows::Win32::System::Threading::{GetSystemTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::core::{PCWSTR, s};

    /// Sampling loop; returns when the sender is dropped.
    pub fn run(app: AppHandle, rx: Receiver<()>) {
        let gpu = Nvml::load();
        let icmp = Icmp::open();
        let mut ping = Ping::default();
        let mut prev_cpu = system_times();
        let mut tick: u64 = 0;
        let mut started = Instant::now();
        loop {
            // Wait first so the first CPU figure spans a full period.
            match rx.recv_timeout(PERIOD.saturating_sub(started.elapsed())) {
                Err(RecvTimeoutError::Disconnected) => break,
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            }
            started = Instant::now();
            let now_cpu = system_times();
            let cpu = match (prev_cpu, now_cpu) {
                (Some(p), Some(n)) => cpu_percent(p, n),
                _ => 0.0,
            };
            prev_cpu = now_cpu;
            if ping_due(tick) {
                let target = app.state::<SettingsStore>().get().game_mode.ping_target;
                ping.last = icmp.as_ref().and_then(|i| ping.measure(i, &target));
            }
            let (gpu_util, gpu_temp) = gpu.as_ref().map(Nvml::sample).unwrap_or((None, None));
            let stats = SysStats {
                cpu,
                ram: ram(),
                gpu: gpu_util,
                gpu_temp,
                ping_ms: ping.last,
                wsl_ram_mb: wsl_ram_mb(),
            };
            // Released while sampling: do not emit a stale frame.
            if matches!(rx.try_recv(), Err(TryRecvError::Disconnected)) {
                break;
            }
            if let Err(e) = app.emit(EVENT, &stats) {
                eprintln!("lucent: cannot emit {EVENT}: {e}");
            }
            tick = tick.wrapping_add(1);
        }
    }

    fn ft(f: FILETIME) -> u64 {
        (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)
    }

    fn system_times() -> Option<(u64, u64, u64)> {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
        Some((ft(idle), ft(kernel), ft(user)))
    }

    fn ram() -> RamStats {
        let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        if unsafe { GlobalMemoryStatusEx(&mut m) }.is_err() {
            return RamStats::default();
        }
        RamStats { used: mb(m.ullTotalPhys.saturating_sub(m.ullAvailPhys)), total: mb(m.ullTotalPhys) }
    }

    /// Working set of the WSL VM process; `None` when it is not running or not readable.
    fn wsl_ram_mb() -> Option<u64> {
        let pid = unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
            let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut best: Option<(u8, u32)> = None;
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                    let rank = wsl_vm_rank(&String::from_utf16_lossy(&e.szExeFile[..n]));
                    if rank > best.map_or(0, |b| b.0) {
                        best = Some((rank, e.th32ProcessID));
                    }
                    if Process32NextW(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
            best?.1
        };
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut c = PROCESS_MEMORY_COUNTERS { cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
            let ok = K32GetProcessMemoryInfo(h, &mut c, c.cb).as_bool();
            let _ = CloseHandle(h);
            ok.then(|| mb(c.WorkingSetSize as u64))
        }
    }

    // ---- GPU (NVML, loaded at runtime) ----

    type NvmlInit = unsafe extern "C" fn() -> i32;
    type NvmlShutdown = unsafe extern "C" fn() -> i32;
    type NvmlHandleByIndex = unsafe extern "C" fn(u32, *mut *mut c_void) -> i32;
    #[repr(C)]
    #[derive(Default)]
    struct NvmlUtilization {
        gpu: u32,
        memory: u32,
    }
    type NvmlUtilizationRates = unsafe extern "C" fn(*mut c_void, *mut NvmlUtilization) -> i32;
    type NvmlTemperature = unsafe extern "C" fn(*mut c_void, u32, *mut u32) -> i32;
    const NVML_SUCCESS: i32 = 0;
    const NVML_TEMPERATURE_GPU: u32 = 0;

    struct Nvml {
        lib: HMODULE,
        device: *mut c_void,
        utilization: NvmlUtilizationRates,
        temperature: NvmlTemperature,
        shutdown: Option<NvmlShutdown>,
    }

    impl Nvml {
        /// `None` without an NVIDIA driver (or on any NVML failure): `gpu: null`.
        fn load() -> Option<Nvml> {
            let lib = load_nvml()?;
            let n = unsafe { Self::init(lib) };
            if n.is_none() {
                unsafe {
                    let _ = FreeLibrary(lib);
                }
            }
            n
        }

        unsafe fn init(lib: HMODULE) -> Option<Nvml> {
            unsafe {
                let init: NvmlInit = std::mem::transmute(GetProcAddress(lib, s!("nvmlInit_v2"))?);
                let by_index: NvmlHandleByIndex =
                    std::mem::transmute(GetProcAddress(lib, s!("nvmlDeviceGetHandleByIndex_v2"))?);
                let utilization: NvmlUtilizationRates =
                    std::mem::transmute(GetProcAddress(lib, s!("nvmlDeviceGetUtilizationRates"))?);
                let temperature: NvmlTemperature =
                    std::mem::transmute(GetProcAddress(lib, s!("nvmlDeviceGetTemperature"))?);
                let shutdown: Option<NvmlShutdown> =
                    GetProcAddress(lib, s!("nvmlShutdown")).map(|f| std::mem::transmute(f));
                if init() != NVML_SUCCESS {
                    return None;
                }
                let mut device = std::ptr::null_mut();
                if by_index(0, &mut device) != NVML_SUCCESS || device.is_null() {
                    if let Some(sd) = shutdown {
                        sd();
                    }
                    return None;
                }
                Some(Nvml { lib, device, utilization, temperature, shutdown })
            }
        }

        fn sample(&self) -> (Option<f32>, Option<f32>) {
            unsafe {
                let mut u = NvmlUtilization::default();
                let util = ((self.utilization)(self.device, &mut u) == NVML_SUCCESS).then_some(u.gpu as f32);
                let mut t = 0u32;
                let temp = ((self.temperature)(self.device, NVML_TEMPERATURE_GPU, &mut t) == NVML_SUCCESS)
                    .then_some(t as f32);
                (util, temp)
            }
        }
    }

    impl Drop for Nvml {
        fn drop(&mut self) {
            unsafe {
                if let Some(sd) = self.shutdown {
                    sd();
                }
                let _ = FreeLibrary(self.lib);
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// `nvml.dll` (System32 on current drivers), else the legacy NVSMI folder.
    fn load_nvml() -> Option<HMODULE> {
        if let Ok(h) = unsafe { LoadLibraryW(windows::core::w!("nvml.dll")) } {
            return Some(h);
        }
        let pf = std::env::var("ProgramFiles").ok()?;
        let path = wide(&format!(r"{pf}\NVIDIA Corporation\NVSMI\nvml.dll"));
        unsafe { LoadLibraryW(PCWSTR(path.as_ptr())) }.ok()
    }

    // ---- Ping (ICMP echo) ----

    struct Icmp(HANDLE);

    impl Icmp {
        fn open() -> Option<Icmp> {
            match unsafe { IcmpCreateFile() } {
                Ok(h) => Some(Icmp(h)),
                Err(e) => {
                    eprintln!("lucent: IcmpCreateFile failed: {e}");
                    None
                }
            }
        }

        /// Round trip in ms, `None` on timeout or error.
        fn echo(&self, addr: [u8; 4]) -> Option<u32> {
            const DATA: [u8; 32] = [0x61; 32];
            // Reply = ICMP_ECHO_REPLY + echoed data + room for an ICMP error (8 bytes).
            let mut buf = vec![0u8; size_of::<ICMP_ECHO_REPLY>() + DATA.len() + 8 + 16];
            let n = unsafe {
                IcmpSendEcho(
                    self.0,
                    u32::from_ne_bytes(addr),
                    DATA.as_ptr().cast(),
                    DATA.len() as u16,
                    None,
                    buf.as_mut_ptr().cast(),
                    buf.len() as u32,
                    PING_TIMEOUT_MS,
                )
            };
            if n == 0 {
                return None;
            }
            // SAFETY: IcmpSendEcho wrote at least one ICMP_ECHO_REPLY at the buffer start;
            // read_unaligned because a Vec<u8> carries no alignment guarantee.
            let reply = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast::<ICMP_ECHO_REPLY>()) };
            (reply.Status == 0).then_some(reply.RoundTripTime)
        }
    }

    impl Drop for Icmp {
        fn drop(&mut self) {
            let _ = unsafe { IcmpCloseHandle(self.0) };
        }
    }

    /// Last result plus the resolved address of the current target (resolved once per
    /// target string; re-resolved after a failure).
    #[derive(Default)]
    struct Ping {
        last: Option<u32>,
        resolved: Option<(String, [u8; 4])>,
    }

    impl Ping {
        fn measure(&mut self, icmp: &Icmp, target: &str) -> Option<u32> {
            let addr = match literal_ipv4(target) {
                Some(a) => a,
                None => match &self.resolved {
                    Some((t, a)) if t == target => *a,
                    _ => {
                        let a = resolve_ipv4(target)?;
                        self.resolved = Some((target.to_string(), a));
                        a
                    }
                },
            };
            let r = icmp.echo(addr);
            if r.is_none() {
                self.resolved = None;
            }
            r
        }
    }

    /// First IPv4 address of a host name via `GetAddrInfoW`.
    fn resolve_ipv4(host: &str) -> Option<[u8; 4]> {
        let host = wide(host.trim());
        unsafe {
            let mut wsa = WSADATA::default();
            if WSAStartup(0x0202, &mut wsa) != 0 {
                return None;
            }
            let hints = ADDRINFOW { ai_family: AF_INET.0 as i32, ..Default::default() };
            let mut res: *mut ADDRINFOW = std::ptr::null_mut();
            let mut out = None;
            if GetAddrInfoW(PCWSTR(host.as_ptr()), PCWSTR::null(), Some(&hints), &mut res) == 0 {
                let mut cur = res;
                while !cur.is_null() {
                    let ai = &*cur;
                    if ai.ai_family == AF_INET.0 as i32 && !ai.ai_addr.is_null() {
                        let sin = std::ptr::read_unaligned(ai.ai_addr.cast::<SOCKADDR_IN>());
                        out = Some(sin.sin_addr.S_un.S_addr.to_ne_bytes());
                        break;
                    }
                    cur = ai.ai_next;
                }
                FreeAddrInfoW(Some(res));
            }
            WSACleanup();
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_table() {
        // (idle, kernel, user); kernel includes idle.
        assert_eq!(cpu_percent((0, 0, 0), (50, 100, 0)), 50.0);
        assert_eq!(cpu_percent((100, 200, 100), (150, 300, 200)), 75.0);
        assert_eq!(cpu_percent((10, 20, 30), (60, 120, 30)), 50.0);
        assert_eq!(cpu_percent((0, 0, 0), (100, 100, 0)), 0.0);
        assert_eq!(cpu_percent((0, 0, 0), (0, 100, 100)), 100.0);
        // Zero delta → 0.
        assert_eq!(cpu_percent((5, 10, 10), (5, 10, 10)), 0.0);
        // Counters going backwards (wrap/reset) never panic or go out of range.
        assert_eq!(cpu_percent((100, 200, 200), (10, 20, 20)), 0.0);
        assert_eq!(cpu_percent((u64::MAX - 1, u64::MAX, u64::MAX), (1, 2, 2)), 0.0);
        // Idle delta larger than total (inconsistent sample) clamps to 0.
        assert_eq!(cpu_percent((0, 0, 0), (200, 100, 0)), 0.0);
    }

    #[test]
    fn stats_json_is_camel_case() {
        let s = SysStats {
            cpu: 23.5,
            ram: RamStats { used: 11_468, total: 32_768 },
            gpu: Some(41.0),
            gpu_temp: Some(62.0),
            ping_ms: Some(18),
            wsl_ram_mb: Some(3_174),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "cpu": 23.5,
                "ram": { "used": 11468, "total": 32768 },
                "gpu": 41.0,
                "gpuTemp": 62.0,
                "pingMs": 18,
                "wslRamMb": 3174
            })
        );
        let none = SysStats::default();
        let v = serde_json::to_value(&none).unwrap();
        assert_eq!(v["gpu"], serde_json::Value::Null);
        assert_eq!(v["gpuTemp"], serde_json::Value::Null);
        assert_eq!(v["pingMs"], serde_json::Value::Null);
        assert_eq!(v["wslRamMb"], serde_json::Value::Null);
    }

    #[test]
    fn ref_count_starts_and_stops_once() {
        let mut s: Sampler<u32> = Sampler::new();
        let mut starts = 0;
        s.acquire(|| {
            starts += 1;
            Some(7)
        });
        s.acquire(|| {
            starts += 1;
            Some(8)
        });
        assert_eq!(starts, 1);
        assert_eq!(s.count(), 2);
        assert_eq!(s.release(), None, "still held once");
        assert_eq!(s.release(), Some(7), "last release stops the running thread");
        assert_eq!(s.release(), None, "extra release is harmless");
        assert_eq!(s.count(), 0);
        // A new acquire starts again.
        s.acquire(|| {
            starts += 1;
            Some(9)
        });
        assert_eq!(starts, 2);
        assert_eq!(s.release(), Some(9));
    }

    #[test]
    fn ping_every_second_tick_and_ip_detection() {
        assert!(ping_due(0));
        assert!(!ping_due(1));
        assert!(ping_due(2));
        assert_eq!(literal_ipv4("1.1.1.1"), Some([1, 1, 1, 1]));
        assert_eq!(literal_ipv4(" 8.8.4.4 "), Some([8, 8, 4, 4]));
        assert_eq!(literal_ipv4("dns.google"), None);
        assert_eq!(literal_ipv4("1.1.1"), None);
    }

    #[test]
    fn wsl_vm_process_names() {
        assert!(is_wsl_vm_process("vmmemWSL"));
        assert!(is_wsl_vm_process("VmmemWSL.exe"));
        assert!(is_wsl_vm_process("vmmem"));
        assert!(is_wsl_vm_process("vmmem.exe"));
        assert!(!is_wsl_vm_process("vmmemcmd.exe"));
        assert!(!is_wsl_vm_process("VALORANT-Win64-Shipping.exe"));
        // vmmemWSL is preferred over a generic vmmem.
        assert!(wsl_vm_rank("vmmemWSL") > wsl_vm_rank("vmmem"));
    }

    #[test]
    fn wsl_result_messages() {
        assert_eq!(free_message(&[], None), "WSL n'est pas lancé");
        assert_eq!(shutdown_message(&[], None), "WSL n'est pas lancé");
        let r = vec!["Ubuntu".to_string(), "docker-desktop".to_string()];
        assert_eq!(free_message(&r, Some(true)), "Cache mémoire de WSL libéré");
        assert_eq!(free_message(&r, Some(false)), "Impossible de libérer la mémoire de WSL");
        assert_eq!(shutdown_message(&r, Some(true)), "WSL arrêté (Ubuntu, docker-desktop)");
        assert_eq!(shutdown_message(&r, Some(false)), "Impossible d'arrêter WSL");
    }

    #[test]
    fn bytes_to_mb() {
        assert_eq!(mb(0), 0);
        assert_eq!(mb(1024 * 1024 * 3 + 5), 3);
    }
}
