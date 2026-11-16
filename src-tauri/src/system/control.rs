//! Control Center (clock click in the dock): Wi-Fi / Bluetooth radios, dark mode and the
//! brightness of the monitor under the cursor (DDC/CI). Pure mapping logic + a worker
//! thread (Windows) created on demand and gone after 60 s idle. Every call is
//! non-blocking: commands only push to the worker's channel; the worker emits `cc://state`.
use super::network::{NetKind, NetworkState};
use serde::Serialize;

/// HKCU key holding the light/dark theme values.
pub const PERSONALIZE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
/// VCP code of the luminance (brightness) control, MCCS.
pub const VCP_BRIGHTNESS: u8 = 0x10;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WifiState {
    pub on: bool,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BluetoothState {
    pub on: bool,
}

/// `cc://state` payload: `None` members = not available on this machine (hidden tile).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlCenterState {
    pub wifi: Option<WifiState>,
    pub bluetooth: Option<BluetoothState>,
    pub dark_mode: bool,
    /// 0..100; None when the monitor has no software brightness.
    pub brightness: Option<u32>,
}

/// `AppsUseLightTheme`: 0 = dark; missing value = light (the Windows default).
pub fn dark_from_registry(apps_use_light: Option<u32>) -> bool {
    apps_use_light == Some(0)
}

/// DWORD written to both `AppsUseLightTheme` and `SystemUsesLightTheme`.
pub fn light_value(dark: bool) -> u32 {
    if dark { 0 } else { 1 }
}

/// Slider value (any number from the page) to a 0..100 percentage; NaN -> 0.
pub fn clamp_brightness(v: f64) -> u32 {
    if v.is_nan() { 0 } else { v.round().clamp(0.0, 100.0) as u32 }
}

/// DDC reply (current, maximum) to a percentage; a zero maximum means unsupported.
pub fn ddc_percent(current: u32, max: u32) -> Option<u32> {
    if max == 0 {
        return None;
    }
    let p = (current.min(max) as f64 * 100.0 / max as f64).round();
    Some(p as u32)
}

/// Percentage to the monitor's DDC scale (0..max).
pub fn ddc_value(percent: u32, max: u32) -> u32 {
    ((percent.min(100) as f64 * max as f64) / 100.0).round() as u32
}

/// SSID shown under the Wi-Fi tile: only when the active connection is Wi-Fi.
pub fn wifi_name(net: &NetworkState) -> Option<String> {
    match net.kind {
        NetKind::Wifi => net.name.clone(),
        _ => None,
    }
}

#[cfg(windows)]
pub mod win {
    use super::{
        BluetoothState, ControlCenterState, PERSONALIZE_KEY, VCP_BRIGHTNESS, WifiState, dark_from_registry, ddc_percent,
        ddc_value, light_value, wifi_name,
    };
    use std::sync::Mutex;
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::time::Duration;
    use tauri::{AppHandle, Emitter};
    use windows::Devices::Radios::{Radio, RadioKind, RadioState};
    use windows::Win32::Devices::Display::{
        DestroyPhysicalMonitors, GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
        GetVCPFeatureAndVCPFeatureReply, PHYSICAL_MONITOR, SetVCPFeature,
    };
    use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };
    use windows::core::{HSTRING, w};

    /// The worker exits after this long without a message (it is recreated on demand).
    const IDLE: Duration = Duration::from_secs(60);

    enum Msg {
        Refresh,
        Wifi(bool),
        Bluetooth(bool),
        Dark(bool),
        Brightness(u32),
    }

    static WORKER: Mutex<Option<Sender<Msg>>> = Mutex::new(None);

    fn send(app: &AppHandle, msg: Msg) {
        let mut slot = WORKER.lock().unwrap_or_else(|e| e.into_inner());
        let msg = match slot.as_ref() {
            Some(tx) => match tx.send(msg) {
                Ok(()) => return,
                Err(mpsc::SendError(m)) => m, // the worker is gone: start a new one
            },
            None => msg,
        };
        let (tx, rx) = mpsc::channel::<Msg>();
        let _ = tx.send(msg);
        let app = app.clone();
        let spawned = std::thread::Builder::new().name("lucent-control".into()).spawn(move || {
            // WinRT (Radios) needs COM on this thread.
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            run(&app, &rx);
            if com {
                unsafe { CoUninitialize() };
            }
        });
        match spawned {
            Ok(_) => *slot = Some(tx),
            Err(e) => eprintln!("lucent: cannot start control center worker: {e}"),
        }
    }

    pub fn refresh(app: &AppHandle) {
        send(app, Msg::Refresh);
    }
    pub fn set_wifi(app: &AppHandle, on: bool) {
        send(app, Msg::Wifi(on));
    }
    pub fn set_bluetooth(app: &AppHandle, on: bool) {
        send(app, Msg::Bluetooth(on));
    }
    pub fn set_dark(app: &AppHandle, on: bool) {
        send(app, Msg::Dark(on));
    }
    pub fn set_brightness(app: &AppHandle, value: u32) {
        send(app, Msg::Brightness(value));
    }

    /// Monitor the brightness slider drives (the one under the cursor at the last refresh)
    /// and its DDC maximum.
    struct Target {
        monitor: HMONITOR,
        max: u32,
    }

    fn run(app: &AppHandle, rx: &mpsc::Receiver<Msg>) {
        if let Ok(op) = Radio::RequestAccessAsync() {
            let _ = op.join();
        }
        let mut target: Option<Target> = None;
        loop {
            let first = match rx.recv_timeout(IDLE) {
                Ok(m) => m,
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    // `send` holds the lock while sending: checking the queue under it
                    // means no message can land in a worker that is about to exit.
                    let mut slot = WORKER.lock().unwrap_or_else(|e| e.into_inner());
                    match rx.try_recv() {
                        Ok(m) => m,
                        Err(_) => {
                            *slot = None;
                            return;
                        }
                    }
                }
            };
            // Coalesce a burst (slider drags): radios/dark in order, only the last brightness.
            let mut batch = vec![first];
            while let Ok(m) = rx.try_recv() {
                batch.push(m);
            }
            let mut brightness = None;
            let mut emit = false;
            let mut theme_changed = false;
            for m in batch {
                match m {
                    Msg::Refresh => emit = true,
                    Msg::Wifi(on) => {
                        set_radio(RadioKind::WiFi, on);
                        emit = true;
                    }
                    Msg::Bluetooth(on) => {
                        set_radio(RadioKind::Bluetooth, on);
                        emit = true;
                    }
                    Msg::Dark(on) => {
                        write_dark(on);
                        theme_changed = true;
                        emit = true;
                    }
                    Msg::Brightness(v) => brightness = Some(v),
                }
            }
            if let Some(v) = brightness {
                // The HMONITOR goes stale after a display change (GetNumberOfPhysical…
                // fails): re-resolve the monitor under the cursor once and retry.
                if !target.as_ref().is_some_and(|t| write_brightness(t, v)) {
                    target = cursor_monitor().and_then(read_brightness).map(|(t, _)| t);
                    if let Some(t) = &target {
                        write_brightness(t, v);
                    }
                }
            }
            if emit {
                let (state, t) = read_state();
                target = t;
                if let Err(e) = app.emit("cc://state", &state) {
                    eprintln!("lucent: cannot emit cc://state: {e}");
                }
            }
            // Explorer and the apps repaint after the broadcast: send it after the page
            // already shows the new state (the broadcast can take a moment).
            if theme_changed {
                broadcast_theme_change();
            }
        }
    }

    fn read_state() -> (ControlCenterState, Option<Target>) {
        let (wifi, bluetooth) = read_radios();
        let net = crate::system::snapshot().network;
        let target = cursor_monitor().and_then(read_brightness);
        let state = ControlCenterState {
            wifi: wifi.map(|on| WifiState { on, name: if on { wifi_name(&net) } else { None } }),
            bluetooth: bluetooth.map(|on| BluetoothState { on }),
            dark_mode: read_dark(),
            brightness: target.as_ref().map(|(_, p)| *p),
        };
        (state, target.map(|(t, _)| t))
    }

    // ---- Radios --------------------------------------------------------------------

    fn radios() -> Vec<Radio> {
        let list = match Radio::GetRadiosAsync().and_then(|op| op.join()) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("lucent: cannot list radios: {e}");
                return Vec::new();
            }
        };
        let n = list.Size().unwrap_or(0);
        (0..n).filter_map(|i| list.GetAt(i).ok()).collect()
    }

    /// (Wi-Fi on, Bluetooth on); None = no such radio.
    fn read_radios() -> (Option<bool>, Option<bool>) {
        let mut wifi = None;
        let mut bt = None;
        for r in radios() {
            let on = r.State().map(|s| s == RadioState::On).unwrap_or(false);
            match r.Kind() {
                Ok(RadioKind::WiFi) => wifi = Some(wifi.unwrap_or(false) || on),
                Ok(RadioKind::Bluetooth) => bt = Some(bt.unwrap_or(false) || on),
                _ => {}
            }
        }
        (wifi, bt)
    }

    fn set_radio(kind: RadioKind, on: bool) {
        let state = if on { RadioState::On } else { RadioState::Off };
        for r in radios().into_iter().filter(|r| r.Kind().ok() == Some(kind)) {
            match r.SetStateAsync(state).and_then(|op| op.join()) {
                Ok(s) if s.0 != 1 => eprintln!("lucent: radio change refused ({})", s.0),
                Ok(_) => {}
                Err(e) => eprintln!("lucent: cannot change radio: {e}"),
            }
        }
    }

    // ---- Dark mode -----------------------------------------------------------------

    fn read_dark() -> bool {
        let key = HSTRING::from(PERSONALIZE_KEY);
        let mut value: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                w!("AppsUseLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut u32 as *mut _),
                Some(&mut len),
            )
        };
        dark_from_registry(if r.is_ok() { Some(value) } else { None })
    }

    fn write_dark(dark: bool) {
        let key = HSTRING::from(PERSONALIZE_KEY);
        let v = light_value(dark);
        for name in [w!("AppsUseLightTheme"), w!("SystemUsesLightTheme")] {
            let r = unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    &key,
                    name,
                    REG_DWORD.0,
                    Some(&v as *const u32 as *const _),
                    std::mem::size_of::<u32>() as u32,
                )
            };
            if r.is_err() {
                eprintln!("lucent: cannot write theme value: {r:?}");
            }
        }
    }

    fn broadcast_theme_change() {
        let area = w!("ImmersiveColorSet");
        unsafe {
            let _ = SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(area.as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                500,
                None,
            );
        }
    }

    // ---- Brightness (DDC/CI) -------------------------------------------------------

    fn cursor_monitor() -> Option<HMONITOR> {
        let mut p = POINT::default();
        unsafe { GetCursorPos(&mut p) }.ok()?;
        let m = unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST) };
        (!m.is_invalid()).then_some(m)
    }

    /// Physical monitors behind `monitor` (several when the desktop is duplicated).
    fn with_physical<T>(monitor: HMONITOR, f: impl FnOnce(&[PHYSICAL_MONITOR]) -> T) -> Option<T> {
        let mut n = 0u32;
        unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut n) }.ok()?;
        if n == 0 {
            return None;
        }
        let mut list = vec![PHYSICAL_MONITOR::default(); n as usize];
        unsafe { GetPhysicalMonitorsFromHMONITOR(monitor, &mut list) }.ok()?;
        let out = f(&list);
        if let Err(e) = unsafe { DestroyPhysicalMonitors(&list) } {
            eprintln!("lucent: cannot release physical monitors: {e}");
        }
        Some(out)
    }

    fn read_brightness(monitor: HMONITOR) -> Option<(Target, u32)> {
        with_physical(monitor, |list| {
            list.iter().find_map(|pm| {
                let (mut cur, mut max) = (0u32, 0u32);
                let ok = unsafe {
                    GetVCPFeatureAndVCPFeatureReply(pm.hPhysicalMonitor, VCP_BRIGHTNESS, None, &mut cur, Some(&mut max))
                };
                if ok == 0 {
                    return None;
                }
                ddc_percent(cur, max).map(|p| (Target { monitor, max }, p))
            })
        })
        .flatten()
    }

    /// False when the physical monitors of `t.monitor` cannot be resolved (stale target).
    fn write_brightness(t: &Target, percent: u32) -> bool {
        let value = ddc_value(percent, t.max);
        with_physical(t.monitor, |list| {
            for pm in list {
                if unsafe { SetVCPFeature(pm.hPhysicalMonitor, VCP_BRIGHTNESS, value) } == 0 {
                    eprintln!("lucent: DDC brightness write failed");
                }
            }
        })
        .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_mode_registry_mapping() {
        assert!(dark_from_registry(Some(0)));
        assert!(!dark_from_registry(Some(1)));
        assert!(!dark_from_registry(None));
        assert_eq!(light_value(true), 0);
        assert_eq!(light_value(false), 1);
        // Round trip: what we write reads back as the asked mode.
        for dark in [true, false] {
            assert_eq!(dark_from_registry(Some(light_value(dark))), dark);
        }
    }

    #[test]
    fn brightness_clamped_to_0_100() {
        assert_eq!(clamp_brightness(-5.0), 0);
        assert_eq!(clamp_brightness(0.0), 0);
        assert_eq!(clamp_brightness(42.4), 42);
        assert_eq!(clamp_brightness(42.6), 43);
        assert_eq!(clamp_brightness(100.0), 100);
        assert_eq!(clamp_brightness(250.0), 100);
        assert_eq!(clamp_brightness(f64::NAN), 0);
    }

    #[test]
    fn ddc_scaling() {
        assert_eq!(ddc_percent(50, 100), Some(50));
        assert_eq!(ddc_percent(128, 255), Some(50));
        assert_eq!(ddc_percent(300, 100), Some(100));
        assert_eq!(ddc_percent(10, 0), None);
        assert_eq!(ddc_value(50, 255), 128);
        assert_eq!(ddc_value(100, 100), 100);
        assert_eq!(ddc_value(150, 80), 80);
        assert_eq!(ddc_value(0, 255), 0);
    }

    #[test]
    fn wifi_name_only_on_wifi() {
        let n = |kind, name: Option<&str>| NetworkState { kind, name: name.map(String::from) };
        assert_eq!(wifi_name(&n(NetKind::Wifi, Some("Maison"))), Some("Maison".into()));
        assert_eq!(wifi_name(&n(NetKind::Ethernet, Some("Maison"))), None);
        assert_eq!(wifi_name(&n(NetKind::None, None)), None);
    }

    #[test]
    fn state_serializes_camel_case_with_nulls() {
        let s = ControlCenterState { wifi: None, bluetooth: Some(BluetoothState { on: true }), dark_mode: true, brightness: None };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j, serde_json::json!({"wifi": null, "bluetooth": {"on": true}, "darkMode": true, "brightness": null}));
    }
}
