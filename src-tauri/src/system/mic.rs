//! Global microphone mute: the default communications capture endpoint (`Ctrl+Alt+M`,
//! `mic_toggle` from the Control Center). Watched like the render volume (`volume.rs`).
//!
//! `audio://devices` belongs to the Control Center (Track C), which owns the full payload
//! (outputs + micMuted): this module never emits it, it only hands the mic state to
//! `crate::system::publish_mic_muted`.
use std::sync::atomic::{AtomicU8, Ordering};

/// Last mic state handed out: 0 unknown, 1 unmuted, 2 muted.
static LAST: AtomicU8 = AtomicU8::new(0);

fn encode(muted: bool) -> u8 {
    if muted { 2 } else { 1 }
}

/// A new state must be published: the first one, then only changes (the endpoint callback
/// repeats the state on every volume change of the mic).
pub fn should_publish(last: Option<bool>, muted: bool) -> bool {
    last != Some(muted)
}

/// Current mic state (`get_system_state`); false while unknown or without a microphone.
pub fn mic_muted() -> bool {
    LAST.load(Ordering::Relaxed) == 2
}

/// Records the state and forwards it to the `audio://devices` owner when it changed.
#[cfg_attr(not(windows), allow(dead_code))]
fn publish(app: &tauri::AppHandle, muted: bool) {
    let last = match LAST.swap(encode(muted), Ordering::Relaxed) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    };
    if should_publish(last, muted) {
        crate::system::publish_mic_muted(app, muted);
    }
}

/// How long the dock peeks to show « Micro coupé » / « Micro activé » after a toggle.
pub const TOGGLE_PEEK_MS: u64 = 1500;

#[cfg(windows)]
pub mod win {
    //! COM watcher on the default communications capture endpoint (same shape as the
    //! render watcher in `volume.rs`: callbacks only push to the worker's channel).
    use std::sync::OnceLock;
    use std::sync::mpsc::{self, Sender};
    use tauri::AppHandle;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::Endpoints::{
        IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
    };
    use windows::Win32::Media::Audio::{
        AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
        IMMNotificationClient_Impl, MMDeviceEnumerator, eCapture, eCommunications,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
    use windows::core::{PCWSTR, Result, implement};

    enum Msg {
        Changed(bool),
        Rebind,
        Toggle,
    }

    static TX: OnceLock<Sender<Msg>> = OnceLock::new();

    /// Toggles the mic mute (hotkey thread, `mic_toggle`). Non-blocking.
    pub fn toggle() {
        match TX.get() {
            Some(tx) => {
                let _ = tx.send(Msg::Toggle);
            }
            None => eprintln!("lucent: mic watcher not running"),
        }
    }

    #[implement(IAudioEndpointVolumeCallback)]
    struct MuteCallback(Sender<Msg>);

    impl IAudioEndpointVolumeCallback_Impl for MuteCallback_Impl {
        fn OnNotify(&self, pnotify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
            if let Some(n) = unsafe { pnotify.as_ref() } {
                let _ = self.0.send(Msg::Changed(n.bMuted.as_bool()));
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
            if flow == eCapture && role == eCommunications {
                let _ = self.0.send(Msg::Rebind);
            }
            Ok(())
        }
        fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
            Ok(())
        }
    }

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
            let device = enumerator.GetDefaultAudioEndpoint(eCapture, eCommunications)?;
            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            let callback: IAudioEndpointVolumeCallback = MuteCallback(tx.clone()).into();
            endpoint.RegisterControlChangeNotify(&callback)?;
            Ok(Bound { endpoint, callback })
        }
    }

    fn muted(b: &Bound) -> bool {
        unsafe { b.endpoint.GetMute() }.map(|m| m.as_bool()).unwrap_or(false)
    }

    pub fn start(app: AppHandle) {
        let (tx, rx) = mpsc::channel::<Msg>();
        if TX.set(tx.clone()).is_err() {
            return;
        }
        let spawned = std::thread::Builder::new().name("lucent-mic".into()).spawn(move || {
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            run(&app, &tx, &rx);
            if com {
                unsafe { CoUninitialize() };
            }
        });
        if let Err(e) = spawned {
            eprintln!("lucent: cannot start mic watcher: {e}");
        }
    }

    fn run(app: &AppHandle, tx: &Sender<Msg>, rx: &mpsc::Receiver<Msg>) {
        let enumerator: IMMDeviceEnumerator = match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
            Ok(e) => e,
            Err(e) => return eprintln!("lucent: no audio device enumerator (mic): {e}"),
        };
        let client: IMMNotificationClient = DeviceWatcher(tx.clone()).into();
        if let Err(e) = unsafe { enumerator.RegisterEndpointNotificationCallback(&client) } {
            eprintln!("lucent: cannot watch capture devices: {e}");
        }
        let mut bound: Option<Bound> = None;
        let rebind = |bound: &mut Option<Bound>| {
            *bound = None;
            match bind(&enumerator, tx) {
                Ok(b) => {
                    super::publish(app, muted(&b));
                    *bound = Some(b);
                }
                // No microphone: nothing to show as muted.
                Err(_) => {
                    super::publish(app, false);
                }
            }
        };
        rebind(&mut bound);
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Changed(m) => {
                    super::publish(app, m);
                }
                Msg::Rebind => rebind(&mut bound),
                Msg::Toggle => {
                    let Some(b) = &bound else {
                        eprintln!("lucent: no microphone to mute");
                        continue;
                    };
                    let next = !muted(b);
                    if let Err(e) = unsafe { b.endpoint.SetMute(next, std::ptr::null()) } {
                        eprintln!("lucent: cannot set mic mute: {e}");
                        continue;
                    }
                    // Publish now (the callback follows and is deduplicated) and show the
                    // « Micro coupé » / « Micro activé » card on every dock.
                    super::publish(app, muted(b));
                    crate::dock_host::peek(String::new(), super::TOGGLE_PEEK_MS);
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
    fn publishes_the_first_state_then_only_changes() {
        assert!(should_publish(None, false));
        assert!(should_publish(None, true));
        assert!(!should_publish(Some(true), true));
        assert!(!should_publish(Some(false), false));
        assert!(should_publish(Some(false), true));
        assert!(should_publish(Some(true), false));
    }
}
