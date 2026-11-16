//! Shared monitor: switch a monitor's input between this PC and another one over DDC/CI
//! (VCP 0x60, "Input Source"). Codes are per model (an HKC 24E4 uses 1 = DisplayPort here,
//! 3 = HDMI to the other PC, and hangs its DDC on the MCCS code 0x11), so both codes are
//! settings, never guessed.
//!
//! DDC calls can block for a long time inside the driver (a wedged monitor never answers,
//! and the calling thread cannot be stopped). Everything runs on one dedicated worker; a
//! request arriving while it is busy is refused instead of queued.
use serde::Serialize;

/// MCCS "Input Source".
pub const VCP_INPUT: u8 = 0x60;

/// Input to switch to: back here when the monitor shows the other PC, else to the other PC.
/// Unknown current input (read failed): to the other PC.
pub fn toggle_target(current: Option<u32>, here: u8, other: u8) -> u8 {
    match current {
        Some(c) if (c & 0xFF) as u8 == other => here,
        _ => other,
    }
}

/// EDID friendly names ("24E4") of the connected monitors vs the configured one: exact
/// match first, else the first that contains it (case-insensitive).
pub fn pick_monitor<'a>(names: &'a [(String, String)], wanted: &str) -> Option<&'a (String, String)> {
    let w = wanted.trim().to_lowercase();
    if w.is_empty() {
        return None;
    }
    names
        .iter()
        .find(|(_, n)| n.trim().to_lowercase() == w)
        .or_else(|| names.iter().find(|(_, n)| n.to_lowercase().contains(&w)))
}

/// Indices of the active paths to keep when `gdi`'s output is switched off.
pub fn paths_without(sources: &[String], gdi: &str) -> Vec<usize> {
    sources.iter().enumerate().filter(|(_, s)| s.as_str() != gdi).map(|(i, _)| i).collect()
}

/// A DDC-capable monitor for the Réglages list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DdcMonitor {
    /// EDID friendly name ("24E4"); what the settings store.
    pub name: String,
    /// Current input code, None when the monitor did not answer.
    pub input: Option<u32>,
}

/// `display://switched` payload: the dock shows it as a short card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Switched {
    pub text: String,
    pub ok: bool,
}

pub fn switched_text(monitor: &str, to_other: bool, other_label: &str, ok: bool) -> String {
    let dest = if to_other { other_label.trim() } else { "ce PC" };
    let dest = if dest.is_empty() { "l'autre PC" } else { dest };
    if ok { format!("Écran {monitor} → {dest}") } else { format!("L'écran {monitor} ne répond pas") }
}

#[cfg(windows)]
pub mod win {
    use super::{DdcMonitor, Switched, VCP_INPUT, pick_monitor, switched_text, toggle_target};
    use crate::settings::DisplaySwitchSettings;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;
    use tauri::{AppHandle, Emitter};
    use windows::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO,
        DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DestroyPhysicalMonitors,
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, GetNumberOfPhysicalMonitorsFromHMONITOR,
        GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply, PHYSICAL_MONITOR, QDC_ONLY_ACTIVE_PATHS,
        QueryDisplayConfig, SDC_ALLOW_CHANGES, SDC_APPLY, SDC_TOPOLOGY_EXTEND, SDC_USE_SUPPLIED_DISPLAY_CONFIG,
        SetDisplayConfig, SetVCPFeature,
    };
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};
    use windows::core::BOOL;

    enum Job {
        /// `None`: toggle; `Some(true)`: to the other PC; `Some(false)`: back here.
        Switch(DisplaySwitchSettings, Option<bool>),
        List(Sender<Vec<DdcMonitor>>),
    }

    static TX: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    static BUSY: AtomicBool = AtomicBool::new(false);
    static APP: OnceLock<AppHandle> = OnceLock::new();

    fn wide(s: &[u16]) -> String {
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        String::from_utf16_lossy(&s[..end])
    }

    struct Active {
        paths: Vec<DISPLAYCONFIG_PATH_INFO>,
        modes: Vec<DISPLAYCONFIG_MODE_INFO>,
        /// Source GDI name of each path ("" when unknown), aligned with `paths`.
        sources: Vec<String>,
        /// (GDI name, EDID friendly name) of the outputs with a name.
        names: Vec<(String, String)>,
    }

    fn active() -> Option<Active> {
        unsafe {
            let (mut np, mut nm) = (0u32, 0u32);
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).is_err() {
                return None;
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None).is_err() {
                return None;
            }
            paths.truncate(np as usize);
            modes.truncate(nm as usize);
            let mut sources = Vec::new();
            let mut names = Vec::new();
            for p in &paths {
                let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
                src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
                src.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
                src.header.adapterId = p.sourceInfo.adapterId;
                src.header.id = p.sourceInfo.id;
                let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
                tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
                tgt.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
                tgt.header.adapterId = p.targetInfo.adapterId;
                tgt.header.id = p.targetInfo.id;
                let gdi = if DisplayConfigGetDeviceInfo(&mut src.header) == 0 { wide(&src.viewGdiDeviceName) } else { String::new() };
                sources.push(gdi.clone());
                if DisplayConfigGetDeviceInfo(&mut tgt.header) != 0 {
                    continue;
                }
                let name = wide(&tgt.monitorFriendlyDeviceName);
                if !name.is_empty() && !gdi.is_empty() {
                    names.push((gdi, name));
                }
            }
            Some(Active { paths, modes, sources, names })
        }
    }

    /// (GDI device `\\.\DISPLAY1`, EDID friendly name) of every active output.
    fn monitor_names() -> Vec<(String, String)> {
        active().map(|a| a.names).unwrap_or_default()
    }

    /// Takes `gdi`'s output off the desktop, as if unplugged: Windows moves its windows to
    /// the other monitors and the cursor can no longer wander onto it. Not saved to the
    /// display database, so « Étendre » brings back the usual layout.
    fn detach_output(gdi: &str) -> bool {
        let Some(a) = active() else { return false };
        let keep: Vec<DISPLAYCONFIG_PATH_INFO> = super::paths_without(&a.sources, gdi).into_iter().map(|i| a.paths[i]).collect();
        if keep.is_empty() || keep.len() == a.paths.len() {
            return false;
        }
        let flags = SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
        let r = unsafe { SetDisplayConfig(Some(&keep), Some(&a.modes), flags) };
        if r != 0 {
            crate::diag::event(format!("display detach failed ({r})"));
        }
        r == 0
    }

    /// Back to the extended desktop (Win+P « Étendre »): the last extended layout.
    fn extend() -> bool {
        let r = unsafe { SetDisplayConfig(None, None, SDC_APPLY | SDC_TOPOLOGY_EXTEND) };
        if r != 0 {
            crate::diag::event(format!("display extend failed ({r})"));
        }
        r == 0
    }

    fn hmonitor_for(gdi: &str) -> Option<HMONITOR> {
        struct Find<'a> {
            gdi: &'a str,
            found: Option<HMONITOR>,
        }
        unsafe extern "system" fn cb(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
            let f = unsafe { &mut *(data.0 as *mut Find) };
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if unsafe { GetMonitorInfoW(m, &mut info as *mut _ as *mut _) }.as_bool() && wide(&info.szDevice) == f.gdi {
                f.found = Some(m);
                return BOOL(0);
            }
            BOOL(1)
        }
        let mut f = Find { gdi, found: None };
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut f as *mut _ as isize));
        }
        f.found
    }

    /// Runs `f` on the first physical monitor behind `m`, then releases them.
    fn with_physical<T>(m: HMONITOR, f: impl FnOnce(&PHYSICAL_MONITOR) -> T) -> Option<T> {
        let mut n = 0u32;
        unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(m, &mut n) }.ok()?;
        if n == 0 {
            return None;
        }
        let mut list = vec![PHYSICAL_MONITOR::default(); n as usize];
        unsafe { GetPhysicalMonitorsFromHMONITOR(m, &mut list) }.ok()?;
        let out = f(&list[0]);
        let _ = unsafe { DestroyPhysicalMonitors(&list) };
        Some(out)
    }

    fn read_input(pm: &PHYSICAL_MONITOR) -> Option<u32> {
        let (mut cur, mut max) = (0u32, 0u32);
        let ok = unsafe { GetVCPFeatureAndVCPFeatureReply(pm.hPhysicalMonitor, VCP_INPUT, None, &mut cur, Some(&mut max)) };
        (ok != 0).then_some(cur & 0xFF)
    }

    /// The monitor is detached (switched to the other PC by us): extend the desktop again,
    /// wait for it to come back, then give it this PC's input.
    fn come_back(s: &DisplaySwitchSettings) -> Switched {
        extend();
        let start = std::time::Instant::now();
        let found = loop {
            if let Some(f) = pick_monitor(&monitor_names(), &s.monitor).cloned() {
                if let Some(m) = hmonitor_for(&f.0) {
                    break Some((f.1, m));
                }
            }
            if start.elapsed() > Duration::from_secs(8) {
                break None;
            }
            std::thread::sleep(Duration::from_millis(300));
        };
        let Some((name, m)) = found else {
            return Switched { text: format!("Écran {} introuvable", s.monitor), ok: false };
        };
        // The monitor just reappeared: its DDC may need a moment.
        let mut ok = false;
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(500));
            ok = with_physical(m, |pm| unsafe { SetVCPFeature(pm.hPhysicalMonitor, VCP_INPUT, s.here as u32) } != 0).unwrap_or(false);
            if ok {
                break;
            }
        }
        crate::diag::event(format!("display switch {name}: back (reattached) ok={ok}"));
        Switched { text: switched_text(&name, false, &s.other_label, ok), ok }
    }

    fn do_switch(s: &DisplaySwitchSettings, dir: Option<bool>) -> Switched {
        let names = monitor_names();
        let Some((gdi, name)) = pick_monitor(&names, &s.monitor).cloned() else {
            // Not on the desktop: detached by an earlier switch (or unplugged).
            if s.detach && dir != Some(true) {
                return come_back(s);
            }
            return Switched { text: format!("Écran {} introuvable", s.monitor), ok: false };
        };
        let Some(m) = hmonitor_for(&gdi) else {
            return Switched { text: format!("Écran {name} introuvable"), ok: false };
        };
        let result = with_physical(m, |pm| {
            let target = match dir {
                Some(true) => s.other,
                Some(false) => s.here,
                None => toggle_target(read_input(pm), s.here, s.other),
            };
            let ok = unsafe { SetVCPFeature(pm.hPhysicalMonitor, VCP_INPUT, target as u32) } != 0;
            (target == s.other, ok)
        });
        let (to_other, ok) = result.unwrap_or((false, false));
        let detached = to_other && ok && s.detach && {
            std::thread::sleep(Duration::from_millis(300));
            detach_output(&gdi)
        };
        crate::diag::event(format!("display switch {name}: to_other={to_other} ok={ok} detached={detached}"));
        Switched { text: switched_text(&name, to_other, &s.other_label, ok), ok }
    }

    fn list() -> Vec<DdcMonitor> {
        monitor_names()
            .into_iter()
            .filter_map(|(gdi, name)| {
                let m = hmonitor_for(&gdi)?;
                // Monitors without DDC/CI have no physical handle with VCP: listed, no input.
                let input = with_physical(m, read_input).flatten();
                Some(DdcMonitor { name, input })
            })
            .collect()
    }

    fn worker(rx: mpsc::Receiver<Job>) {
        for job in rx {
            match job {
                Job::Switch(s, dir) => {
                    let r = do_switch(&s, dir);
                    // Silent when it worked (the screen itself changing is the feedback);
                    // a failure shows a card, the dock peeking for it.
                    if !r.ok {
                        for m in crate::monitors::win::enumerate() {
                            crate::dock_host::peek(m.id, 2200);
                        }
                        if let Some(app) = APP.get() {
                            let _ = app.emit("display://switched", r);
                        }
                    }
                }
                Job::List(reply) => {
                    let _ = reply.send(list());
                }
            }
            BUSY.store(false, Ordering::SeqCst);
        }
    }

    fn submit(job: Job) -> bool {
        if BUSY.swap(true, Ordering::SeqCst) {
            return false;
        }
        let tx = TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            let spawned = std::thread::Builder::new().name("lucent-ddc-input".into()).spawn(move || worker(rx));
            if let Err(e) = spawned {
                eprintln!("lucent: cannot spawn DDC input thread: {e}");
            }
            Mutex::new(tx)
        });
        let sent = tx.lock().unwrap_or_else(|e| e.into_inner()).send(job).is_ok();
        if !sent {
            BUSY.store(false, Ordering::SeqCst);
        }
        sent
    }

    pub fn init(app: &AppHandle) {
        let _ = APP.set(app.clone());
    }

    /// Non-blocking. A busy (possibly wedged) worker refuses: the user gets a card saying so.
    pub fn switch(s: DisplaySwitchSettings, dir: Option<bool>) {
        if s.monitor.trim().is_empty() {
            return;
        }
        let name = s.monitor.clone();
        if !submit(Job::Switch(s, dir)) {
            if let Some(app) = APP.get() {
                let _ = app.emit("display://switched", Switched { text: switched_text(&name, false, "", false), ok: false });
            }
        }
    }

    /// Blocking up to `timeout` (call from the blocking pool): empty when busy or too slow.
    pub fn list_blocking(timeout: Duration) -> Vec<DdcMonitor> {
        let (tx, rx) = mpsc::channel();
        if !submit(Job::List(tx)) {
            return Vec::new();
        }
        rx.recv_timeout(timeout).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_between_the_two_codes() {
        assert_eq!(toggle_target(Some(1), 1, 3), 3);
        assert_eq!(toggle_target(Some(3), 1, 3), 1);
        // Some monitors put flags in the high byte.
        assert_eq!(toggle_target(Some(0x0103), 1, 3), 1);
        assert_eq!(toggle_target(None, 1, 3), 3);
        assert_eq!(toggle_target(Some(2), 1, 3), 3);
    }

    #[test]
    fn monitor_by_friendly_name() {
        let names = vec![
            (r"\\.\DISPLAY1".to_string(), "Odyssey G40B".to_string()),
            (r"\\.\DISPLAY2".to_string(), "24E4".to_string()),
        ];
        assert_eq!(pick_monitor(&names, "24e4").map(|n| n.0.as_str()), Some(r"\\.\DISPLAY2"));
        assert_eq!(pick_monitor(&names, "odyssey").map(|n| n.0.as_str()), Some(r"\\.\DISPLAY1"));
        assert_eq!(pick_monitor(&names, ""), None);
        assert_eq!(pick_monitor(&names, "LG"), None);
    }

    #[test]
    fn detaching_keeps_the_other_outputs() {
        let s = vec![r"\\.\DISPLAY1".to_string(), r"\\.\DISPLAY2".to_string()];
        assert_eq!(paths_without(&s, r"\\.\DISPLAY2"), vec![0]);
        assert_eq!(paths_without(&s, r"\\.\DISPLAY9"), vec![0, 1]);
    }

    #[test]
    fn card_texts() {
        assert_eq!(switched_text("24E4", true, "PC portable", true), "Écran 24E4 → PC portable");
        assert_eq!(switched_text("24E4", true, " ", true), "Écran 24E4 → l'autre PC");
        assert_eq!(switched_text("24E4", false, "PC portable", true), "Écran 24E4 → ce PC");
        assert_eq!(switched_text("24E4", false, "", false), "L'écran 24E4 ne répond pas");
    }
}
