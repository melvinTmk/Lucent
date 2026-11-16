//! Default render endpoint volume: pure icon logic + the COM watcher (Windows).
use serde::Serialize;

/// `"mute" | "low" | "mid" | "high"`; thresholds `0`, `< 0.34`, `< 0.67`.
pub fn volume_icon(level: f32, muted: bool) -> &'static str {
    if muted || level <= 0.0 {
        "mute"
    } else if level < 0.34 {
        "low"
    } else if level < 0.67 {
        "mid"
    } else {
        "high"
    }
}

/// `system://volume` payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VolumeState {
    pub level: f32,
    pub muted: bool,
    pub icon: &'static str,
}

impl VolumeState {
    pub fn new(level: f32, muted: bool) -> Self {
        let level = level.clamp(0.0, 1.0);
        Self { level, muted, icon: volume_icon(level, muted) }
    }
}

#[cfg(windows)]
pub mod win {
    use super::VolumeState;
    use std::sync::mpsc::{self, Sender};
    use std::sync::OnceLock;
    use tauri::AppHandle;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::Endpoints::{
        IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
    };
    use windows::Win32::Media::Audio::{
        AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, EDataFlow, ERole, IMMDeviceEnumerator,
        IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, eConsole, eRender,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::core::{PCWSTR, Result, implement};

    enum Msg {
        /// From the endpoint callback.
        Changed(f32, bool),
        /// Default device changed / appeared: bind again.
        Rebind,
        Set(f32),
        ToggleMute,
    }

    static TX: OnceLock<Sender<Msg>> = OnceLock::new();

    /// Sets the master volume (0..1). Non-blocking: the watcher thread applies it.
    pub fn set_volume(level: f32) {
        if let Some(tx) = TX.get() {
            let _ = tx.send(Msg::Set(level));
        }
    }

    pub fn toggle_mute() {
        if let Some(tx) = TX.get() {
            let _ = tx.send(Msg::ToggleMute);
        }
    }

    #[implement(IAudioEndpointVolumeCallback)]
    struct VolumeCallback(Sender<Msg>);

    impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
        fn OnNotify(&self, pnotify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
            // Callback thread: copy the two values out and hand them to the worker.
            if let Some(n) = unsafe { pnotify.as_ref() } {
                let _ = self.0.send(Msg::Changed(n.fMasterVolume, n.bMuted.as_bool()));
            }
            Ok(())
        }
    }

    #[implement(IMMNotificationClient)]
    struct DeviceWatcher(Sender<Msg>);

    impl IMMNotificationClient_Impl for DeviceWatcher_Impl {
        fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
            let _ = self.0.send(Msg::Rebind);
            Ok(())
        }
        fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
            Ok(())
        }
        fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
            Ok(())
        }
        fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> Result<()> {
            if flow == eRender && role == eConsole {
                let _ = self.0.send(Msg::Rebind);
            }
            Ok(())
        }
        fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
            Ok(())
        }
    }

    /// The bound endpoint and the callback registered on it.
    struct Bound {
        endpoint: IAudioEndpointVolume,
        callback: IAudioEndpointVolumeCallback,
    }

    impl Drop for Bound {
        fn drop(&mut self) {
            unsafe {
                let _ = self.endpoint.UnregisterControlChangeNotify(&self.callback);
            }
        }
    }

    fn bind(enumerator: &IMMDeviceEnumerator, tx: &Sender<Msg>) -> Result<Bound> {
        unsafe {
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            let callback: IAudioEndpointVolumeCallback = VolumeCallback(tx.clone()).into();
            endpoint.RegisterControlChangeNotify(&callback)?;
            Ok(Bound { endpoint, callback })
        }
    }

    fn read(b: &Bound) -> VolumeState {
        let (level, muted) = unsafe {
            (
                b.endpoint.GetMasterVolumeLevelScalar().unwrap_or(0.0),
                b.endpoint.GetMute().map(|m| m.as_bool()).unwrap_or(false),
            )
        };
        VolumeState::new(level, muted)
    }

    pub fn start(app: AppHandle) {
        let (tx, rx) = mpsc::channel::<Msg>();
        if TX.set(tx.clone()).is_err() {
            return;
        }
        let spawned = std::thread::Builder::new().name("lucent-volume".into()).spawn(move || {
            // MTA on this thread: callbacks arrive on COM worker threads, never on ours.
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            run(&app, &tx, &rx);
            if com {
                unsafe { CoUninitialize() };
            }
        });
        if let Err(e) = spawned {
            eprintln!("lucent: cannot start volume watcher: {e}");
        }
    }

    fn run(app: &AppHandle, tx: &Sender<Msg>, rx: &mpsc::Receiver<Msg>) {
        let enumerator: IMMDeviceEnumerator = match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
            Ok(e) => e,
            Err(e) => return eprintln!("lucent: no audio device enumerator: {e}"),
        };
        let client: IMMNotificationClient = DeviceWatcher(tx.clone()).into();
        if let Err(e) = unsafe { enumerator.RegisterEndpointNotificationCallback(&client) } {
            eprintln!("lucent: cannot watch audio devices: {e}");
        }
        let mut bound: Option<Bound> = None;
        let rebind = |bound: &mut Option<Bound>| {
            *bound = None; // unregister the old callback first
            match bind(&enumerator, tx) {
                Ok(b) => {
                    crate::system::publish_volume(app, read(&b));
                    *bound = Some(b);
                }
                // No render device: show it as muted until one appears.
                Err(_) => crate::system::publish_volume(app, VolumeState::new(0.0, true)),
            }
        };
        rebind(&mut bound);
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Changed(level, muted) => crate::system::publish_volume(app, VolumeState::new(level, muted)),
                Msg::Rebind => rebind(&mut bound),
                Msg::Set(level) => {
                    if let Some(b) = &bound {
                        unsafe {
                            let _ = b.endpoint.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null());
                            if level > 0.0 && b.endpoint.GetMute().map(|m| m.as_bool()).unwrap_or(false) {
                                let _ = b.endpoint.SetMute(false, std::ptr::null());
                            }
                        }
                    }
                }
                Msg::ToggleMute => {
                    if let Some(b) = &bound {
                        unsafe {
                            let muted = b.endpoint.GetMute().map(|m| m.as_bool()).unwrap_or(false);
                            let _ = b.endpoint.SetMute(!muted, std::ptr::null());
                        }
                    }
                }
            }
        }
        unsafe {
            let _ = enumerator.UnregisterEndpointNotificationCallback(&client);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn volume_icon_thresholds() {
        assert_eq!(volume_icon(0.5, true), "mute");
        assert_eq!(volume_icon(0.0, false), "mute");
        assert_eq!(volume_icon(0.2, false), "low");
        assert_eq!(volume_icon(0.5, false), "mid");
        assert_eq!(volume_icon(0.9, false), "high");
    }
}
