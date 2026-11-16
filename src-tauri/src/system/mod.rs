//! System icons (volume, network, notifications). Watchers live on their own COM (MTA)
//! threads; callbacks only push to a channel, the thread then stores + emits the state.
pub mod audio_out;
pub mod control;
pub mod display_input;
pub mod media;
pub mod mic;
pub mod network;
pub mod open;
pub mod volume;

use media::MediaState;
use network::NetworkState;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use volume::VolumeState;

/// `get_system_state` payload (the page pulls it on boot, then follows the events).
#[derive(Clone, Debug, Serialize)]
pub struct SystemState {
    pub volume: Option<VolumeState>,
    pub network: NetworkState,
    /// Default communications mic muted (then `audio://devices`).
    #[serde(rename = "micMuted")]
    pub mic_muted: bool,
}

static STATE: Mutex<SystemState> = Mutex::new(SystemState {
    volume: None,
    network: NetworkState { kind: network::NetKind::None, name: None },
    mic_muted: false,
});

pub fn snapshot() -> SystemState {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    s.mic_muted = mic::mic_muted();
    s
}

/// Stores the volume (so `get_system_state` sees it) and emits `system://volume`.
pub(crate) fn publish_volume(app: &AppHandle, v: VolumeState) {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).volume = Some(v.clone());
    if let Err(e) = app.emit("system://volume", v) {
        eprintln!("lucent: cannot emit system://volume: {e}");
    }
}

static MEDIA: Mutex<Option<MediaState>> = Mutex::new(None);

/// Current media session (`get_media`); None when there is none (or not on Windows).
pub fn media_snapshot() -> Option<MediaState> {
    MEDIA.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Stores the media state and emits `media://state` (`null` = no session), only on change.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn publish_media(app: &AppHandle, m: Option<MediaState>) {
    {
        let mut cur = MEDIA.lock().unwrap_or_else(|e| e.into_inner());
        if *cur == m {
            return;
        }
        *cur = m.clone();
    }
    if let Err(e) = app.emit("media://state", m) {
        eprintln!("lucent: cannot emit media://state: {e}");
    }
}

/// Stores the network state and emits `system://network`.
pub(crate) fn publish_network(app: &AppHandle, n: NetworkState) {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).network = n.clone();
    if let Err(e) = app.emit("system://network", n) {
        eprintln!("lucent: cannot emit system://network: {e}");
    }
}

/// `audio://devices` payload. Outputs (Control Center, `audio_out`) and the mic mute state
/// (`mic`) come from two watchers: both go through this merged state so each emit carries
/// the whole payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevices {
    pub outputs: Vec<audio_out::AudioOutput>,
    pub mic_muted: bool,
}

static AUDIO: Mutex<AudioDevices> = Mutex::new(AudioDevices { outputs: Vec::new(), mic_muted: false });

/// Current `audio://devices` payload (`get_audio_devices`).
pub fn audio_snapshot() -> AudioDevices {
    AUDIO.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn update_audio(app: &AppHandle, f: impl FnOnce(&mut AudioDevices)) {
    let payload = {
        let mut cur = AUDIO.lock().unwrap_or_else(|e| e.into_inner());
        let before = cur.clone();
        f(&mut cur);
        if *cur == before {
            return;
        }
        cur.clone()
    };
    if let Err(e) = app.emit("audio://devices", payload) {
        eprintln!("lucent: cannot emit audio://devices: {e}");
    }
}

/// Stores the output list and emits the merged `audio://devices` (only on change).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn publish_audio_outputs(app: &AppHandle, outputs: Vec<audio_out::AudioOutput>) {
    update_audio(app, |a| a.outputs = outputs);
}

/// Stores the mic mute state and emits the merged `audio://devices` (only on change).
/// Called by the mic watcher (`mic.rs`).
#[allow(dead_code)]
pub(crate) fn publish_mic_muted(app: &AppHandle, muted: bool) {
    update_audio(app, |a| a.mic_muted = muted);
}

/// Starts the volume, network, media and audio output watchers (each on its own thread).
#[cfg(windows)]
pub fn start(app: &AppHandle) {
    volume::win::start(app.clone());
    audio_out::win::start(app.clone());
    network::win::start(app.clone());
    media::win::start(app.clone());
}
