//! Audio outputs for the Control Center: the active render endpoints (sorted, default
//! flagged), watched with `IMMNotificationClient`, and the default switch through the
//! undocumented `IPolicyConfig` (as every Windows audio switcher does). When that COM
//! class is unavailable the list is published empty, so the page hides it.
use serde::Serialize;

/// One `audio://devices.outputs` entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AudioOutput {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// `(id, friendly name)` pairs to the published list: sorted by name (case-insensitive,
/// then id for identical names), the default endpoint flagged.
pub fn build_outputs(devices: Vec<(String, String)>, default_id: Option<&str>) -> Vec<AudioOutput> {
    let mut out: Vec<AudioOutput> = devices
        .into_iter()
        .map(|(id, name)| {
            let default = default_id == Some(id.as_str());
            let name = if name.trim().is_empty() { id.clone() } else { name };
            AudioOutput { id, name, default }
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
    out
}

#[cfg(windows)]
#[allow(non_snake_case)] // IPolicyConfig keeps its COM method names
pub mod win {
    use super::build_outputs;
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::time::Duration;
    use tauri::AppHandle;
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{
        DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient,
        IMMNotificationClient_Impl, MMDeviceEnumerator, eCommunications, eConsole, eMultimedia, eRender,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, STGM_READ,
    };
    use windows::core::{GUID, HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, PCWSTR, PWSTR, Result, implement};

    /// Device events come in bursts (a headset adds several endpoints): settle first.
    const DEBOUNCE: Duration = Duration::from_millis(150);

    /// `CPolicyConfigClient`.
    const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    /// Undocumented `IPolicyConfig` (Windows 7+). Only `SetDefaultEndpoint` is used; the
    /// other slots keep the vtable layout.
    #[windows_core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    unsafe trait IPolicyConfig: IUnknown {
        unsafe fn GetMixFormat(&self, id: PCWSTR, format: *mut *mut c_void) -> HRESULT;
        unsafe fn GetDeviceFormat(&self, id: PCWSTR, default: i32, format: *mut *mut c_void) -> HRESULT;
        unsafe fn ResetDeviceFormat(&self, id: PCWSTR) -> HRESULT;
        unsafe fn SetDeviceFormat(&self, id: PCWSTR, endpoint: *mut c_void, mix: *mut c_void) -> HRESULT;
        unsafe fn GetProcessingPeriod(&self, id: PCWSTR, default: i32, period: *mut i64, min: *mut i64) -> HRESULT;
        unsafe fn SetProcessingPeriod(&self, id: PCWSTR, period: *mut i64) -> HRESULT;
        unsafe fn GetShareMode(&self, id: PCWSTR, mode: *mut c_void) -> HRESULT;
        unsafe fn SetShareMode(&self, id: PCWSTR, mode: *mut c_void) -> HRESULT;
        unsafe fn GetPropertyValue(&self, id: PCWSTR, key: *const PROPERTYKEY, value: *mut c_void) -> HRESULT;
        unsafe fn SetPropertyValue(&self, id: PCWSTR, key: *const PROPERTYKEY, value: *mut c_void) -> HRESULT;
        unsafe fn SetDefaultEndpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
        unsafe fn SetEndpointVisibility(&self, id: PCWSTR, visible: i32) -> HRESULT;
    }

    enum Msg {
        Refresh,
        Set(String),
    }

    static TX: OnceLock<Sender<Msg>> = OnceLock::new();

    /// Makes `id` the default output for every role. Non-blocking.
    pub fn set_output(id: String) {
        if let Some(tx) = TX.get() {
            let _ = tx.send(Msg::Set(id));
        }
    }

    #[implement(IMMNotificationClient)]
    struct Watcher(Sender<Msg>);

    impl IMMNotificationClient_Impl for Watcher_Impl {
        fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
            let _ = self.0.send(Msg::Refresh);
            Ok(())
        }
        fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
            let _ = self.0.send(Msg::Refresh);
            Ok(())
        }
        fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
            let _ = self.0.send(Msg::Refresh);
            Ok(())
        }
        fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> Result<()> {
            if flow == eRender && role == eConsole {
                let _ = self.0.send(Msg::Refresh);
            }
            Ok(())
        }
        fn OnPropertyValueChanged(&self, _id: &PCWSTR, key: &PROPERTYKEY) -> Result<()> {
            // Only a rename matters; other properties change constantly on some drivers.
            if *key == PKEY_Device_FriendlyName {
                let _ = self.0.send(Msg::Refresh);
            }
            Ok(())
        }
    }

    pub fn start(app: AppHandle) {
        let (tx, rx) = mpsc::channel::<Msg>();
        if TX.set(tx.clone()).is_err() {
            return;
        }
        let spawned = std::thread::Builder::new().name("lucent-audio-out".into()).spawn(move || {
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            run(&app, &tx, &rx);
            if com {
                unsafe { CoUninitialize() };
            }
        });
        if let Err(e) = spawned {
            eprintln!("lucent: cannot start audio output watcher: {e}");
        }
    }

    fn run(app: &AppHandle, tx: &Sender<Msg>, rx: &mpsc::Receiver<Msg>) {
        let enumerator: IMMDeviceEnumerator = match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
            Ok(e) => e,
            Err(e) => return eprintln!("lucent: no audio device enumerator (outputs): {e}"),
        };
        // Without IPolicyConfig the list could not switch anything: leave it empty (hidden).
        let policy: Option<IPolicyConfig> = match unsafe { CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) } {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("lucent: IPolicyConfig unavailable, output list hidden: {e}");
                None
            }
        };
        let Some(policy) = policy else {
            crate::system::publish_audio_outputs(app, Vec::new());
            return;
        };
        let client: IMMNotificationClient = Watcher(tx.clone()).into();
        if let Err(e) = unsafe { enumerator.RegisterEndpointNotificationCallback(&client) } {
            eprintln!("lucent: cannot watch audio outputs: {e}");
        }
        crate::system::publish_audio_outputs(app, list(&enumerator));
        while let Ok(msg) = rx.recv() {
            let mut refresh = false;
            let mut pending = Some(msg);
            // Drain the burst: apply sets in order, refresh once at the end.
            loop {
                match pending.take() {
                    Some(Msg::Refresh) => refresh = true,
                    Some(Msg::Set(id)) => {
                        set_default(&policy, &id);
                        refresh = true;
                    }
                    None => {}
                }
                match rx.recv_timeout(DEBOUNCE) {
                    Ok(m) => pending = Some(m),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            if refresh {
                crate::system::publish_audio_outputs(app, list(&enumerator));
            }
        }
        unsafe {
            let _ = enumerator.UnregisterEndpointNotificationCallback(&client);
        }
    }

    fn set_default(policy: &IPolicyConfig, id: &str) {
        let wid = HSTRING::from(id);
        for role in [eConsole, eMultimedia, eCommunications] {
            let hr = unsafe { policy.SetDefaultEndpoint(PCWSTR(wid.as_ptr()), role) };
            if hr.is_err() {
                eprintln!("lucent: cannot set default output: {hr:?}");
            }
        }
    }

    /// Takes a COM-allocated string.
    fn take(p: PWSTR) -> Option<String> {
        if p.is_null() {
            return None;
        }
        let s = unsafe { p.to_string() }.ok();
        unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
        s
    }

    fn device_id(d: &IMMDevice) -> Option<String> {
        unsafe { d.GetId() }.ok().and_then(take)
    }

    fn friendly_name(d: &IMMDevice) -> String {
        let read = || -> Result<String> {
            let store = unsafe { d.OpenPropertyStore(STGM_READ)? };
            let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName)? };
            let p = unsafe { PropVariantToStringAlloc(&value)? };
            Ok(take(p).unwrap_or_default())
        };
        read().unwrap_or_default()
    }

    fn list(enumerator: &IMMDeviceEnumerator) -> Vec<super::AudioOutput> {
        let default = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .ok()
            .and_then(|d| device_id(&d));
        let devices = match unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) } {
            Ok(c) => c,
            Err(e) => {
                eprintln!("lucent: cannot list audio outputs: {e}");
                return Vec::new();
            }
        };
        let n = unsafe { devices.GetCount() }.unwrap_or(0);
        let pairs = (0..n)
            .filter_map(|i| unsafe { devices.Item(i) }.ok())
            .filter_map(|d| Some((device_id(&d)?, friendly_name(&d))))
            .collect();
        build_outputs(pairs, default.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(id: &str, name: &str) -> (String, String) {
        (id.into(), name.into())
    }

    #[test]
    fn outputs_sorted_by_name_with_default_flag() {
        let out = build_outputs(
            vec![pair("{3}", "Speakers (Realtek)"), pair("{1}", "casque USB"), pair("{2}", "LG UltraGear (NVIDIA)")],
            Some("{3}"),
        );
        let names: Vec<_> = out.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["casque USB", "LG UltraGear (NVIDIA)", "Speakers (Realtek)"]);
        let defaults: Vec<_> = out.iter().map(|o| o.default).collect();
        assert_eq!(defaults, [false, false, true]);
    }

    #[test]
    fn no_default_and_unknown_default() {
        assert!(build_outputs(vec![pair("a", "A")], None).iter().all(|o| !o.default));
        assert!(build_outputs(vec![pair("a", "A")], Some("zz")).iter().all(|o| !o.default));
    }

    #[test]
    fn same_names_ordered_by_id_and_blank_name_falls_back_to_id() {
        let out = build_outputs(vec![pair("b", "Haut-parleurs"), pair("a", "Haut-parleurs"), pair("z", "  ")], None);
        let ids: Vec<_> = out.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "z"]);
        assert_eq!(out[2].name, "z");
    }

    #[test]
    fn serializes_as_ts_contract() {
        let j = serde_json::to_value(build_outputs(vec![pair("x", "X")], Some("x"))).unwrap();
        assert_eq!(j, serde_json::json!([{"id": "x", "name": "X", "default": true}]));
    }
}
