//! System media session (GSMTC): pure helpers + the WinRT watcher (Windows).
use serde::Serialize;

/// `media://state` payload (`null` when there is no session).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaState {
    pub title: String,
    pub artist: String,
    /// `data:image/png;base64,…`, 96×96, or None.
    pub art_url: Option<String>,
    pub playing: bool,
    pub can_prev: bool,
    pub can_next: bool,
    pub can_play_pause: bool,
}

/// Largest artwork side accepted: a header declaring more is refused before decoding.
const MAX_ART_SIDE: u32 = 4096;
/// Decoder allocation cap (a 4096×4096 RGBA image is exactly 64 MiB).
const MAX_ART_ALLOC: u64 = 64 * 1024 * 1024;

/// Decodes the artwork (with explicit size/allocation limits: the bytes come from any
/// player), cover-crops it to a square, resizes to 96×96, re-encodes as PNG.
pub fn art_data_url(bytes: &[u8]) -> Option<String> {
    use base64::Engine;
    const SIDE: u32 = 96;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_ART_SIDE);
    limits.max_image_height = Some(MAX_ART_SIDE);
    limits.max_alloc = Some(MAX_ART_ALLOC);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    if side == 0 {
        return None;
    }
    let square = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    let small = square.resize_exact(SIDE, SIDE, image::imageops::FilterType::Triangle);
    let mut out = std::io::Cursor::new(Vec::new());
    small.write_to(&mut out, image::ImageFormat::Png).ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(out.into_inner());
    Some(format!("data:image/png;base64,{b64}"))
}

/// Players often publish the title before the thumbnail: retry a missing artwork a few
/// times for the same track before giving up.
pub const ART_RETRIES: u8 = 3;

/// Last artwork read, keyed by (title, artist). Reset it (`ArtCache::default()`) when
/// the session changes: two players can show the same title/artist.
#[derive(Default)]
pub struct ArtCache {
    key: Option<(String, String)>,
    url: Option<String>,
    tries: u8,
}

impl ArtCache {
    /// The cached artwork for this track, or `read()` (at most `ART_RETRIES` times per
    /// track while it returns None).
    pub fn get(&mut self, title: &str, artist: &str, read: impl FnOnce() -> Option<String>) -> Option<String> {
        let same = self.key.as_ref().is_some_and(|(t, a)| t == title && a == artist);
        if !same {
            self.key = Some((title.to_owned(), artist.to_owned()));
            self.url = None;
            self.tries = 0;
        }
        if self.url.is_none() && self.tries < ART_RETRIES {
            self.tries += 1;
            self.url = read();
        }
        self.url.clone()
    }

    /// The player said its properties changed (possibly only the cover): the next `get`
    /// reads the artwork again, even for the same track.
    pub fn properties_changed(&mut self) {
        self.url = None;
        self.tries = 0;
    }
}

/// A session is shown only when it has a title.
pub fn is_displayable(title: &str) -> bool {
    !title.trim().is_empty()
}

#[cfg(windows)]
pub mod win {
    use super::{ArtCache, MediaState, art_data_url, is_displayable};
    use std::sync::OnceLock;
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::time::Duration;
    use tauri::AppHandle;
    use windows::Foundation::TypedEventHandler;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionMediaProperties as Properties,
        GlobalSystemMediaTransportControlsSessionPlaybackControls as Controls,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    use windows::Storage::Streams::DataReader;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::core::Result;

    /// Artwork bigger than this is ignored (players send a few hundred KB at most).
    const MAX_ART_BYTES: u64 = 4 * 1024 * 1024;
    /// Safety net: some sessions stop raising events.
    const POLL: Duration = Duration::from_secs(2);

    #[derive(Clone, Copy)]
    enum Action {
        PlayPause,
        Next,
        Previous,
    }

    enum Msg {
        /// From the WinRT event handlers (or a control): read the session again.
        Refresh,
        /// `MediaPropertiesChanged`: read the session again, cover included (the player
        /// may have changed only the artwork for the same title).
        PropertiesChanged,
        Control(Action),
    }

    static TX: OnceLock<Sender<Msg>> = OnceLock::new();

    /// `"play_pause" | "next" | "previous"`; anything else is ignored. Non-blocking.
    pub fn control(action: &str) {
        let action = match action {
            "play_pause" => Action::PlayPause,
            "next" => Action::Next,
            "previous" => Action::Previous,
            _ => return,
        };
        if let Some(tx) = TX.get() {
            let _ = tx.send(Msg::Control(action));
        }
    }

    pub fn start(app: AppHandle) {
        let (tx, rx) = mpsc::channel::<Msg>();
        if TX.set(tx.clone()).is_err() {
            return;
        }
        let spawned = std::thread::Builder::new().name("lucent-media".into()).spawn(move || {
            // MTA: event handlers run on WinRT worker threads and only post to the channel.
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            run(&app, &tx, &rx);
            if com {
                unsafe { CoUninitialize() };
            }
        });
        if let Err(e) = spawned {
            eprintln!("lucent: cannot start media watcher: {e}");
        }
    }

    fn notifier<S, A>(tx: &Sender<Msg>, msg: fn() -> Msg) -> TypedEventHandler<S, A>
    where
        S: windows::core::RuntimeType + 'static,
        A: windows::core::RuntimeType + 'static,
    {
        let tx = tx.clone();
        TypedEventHandler::new(move |_, _| {
            let _ = tx.send(msg());
            Ok(())
        })
    }

    fn refresher<S, A>(tx: &Sender<Msg>) -> TypedEventHandler<S, A>
    where
        S: windows::core::RuntimeType + 'static,
        A: windows::core::RuntimeType + 'static,
    {
        notifier(tx, || Msg::Refresh)
    }

    /// The session we listen to, with its two event tokens (removed on drop).
    struct Watched {
        session: Session,
        props: Option<i64>,
        playback: Option<i64>,
    }

    impl Watched {
        fn new(session: Session, tx: &Sender<Msg>) -> Self {
            let props = session.MediaPropertiesChanged(&notifier(tx, || Msg::PropertiesChanged)).ok();
            let playback = session.PlaybackInfoChanged(&refresher(tx)).ok();
            if props.is_none() || playback.is_none() {
                eprintln!("lucent: cannot subscribe to media session events (polling only)");
            }
            Self { session, props, playback }
        }
    }

    impl Drop for Watched {
        fn drop(&mut self) {
            if let Some(t) = self.props {
                let _ = self.session.RemoveMediaPropertiesChanged(t);
            }
            if let Some(t) = self.playback {
                let _ = self.session.RemovePlaybackInfoChanged(t);
            }
        }
    }

    // Note: the `.join()` calls below (and in `read` / `apply`) have no timeout. A hung
    // player blocks only this media thread (island frozen, dock unaffected); a timeout is
    // deferred.
    fn read_art(props: &Properties) -> Result<Option<String>> {
        let stream = props.Thumbnail()?.OpenReadAsync()?.join()?;
        let size = stream.Size()?;
        if size == 0 || size > MAX_ART_BYTES {
            return Ok(None);
        }
        let reader = DataReader::CreateDataReader(&stream)?;
        let n = reader.LoadAsync(size as u32)?.join()?;
        let mut buf = vec![0u8; n as usize];
        reader.ReadBytes(&mut buf)?;
        Ok(art_data_url(&buf))
    }

    fn read(session: &Session, art: &mut ArtCache) -> Result<Option<MediaState>> {
        let props = session.TryGetMediaPropertiesAsync()?.join()?;
        let title = props.Title().map(|s| s.to_string()).unwrap_or_default();
        if !is_displayable(&title) {
            return Ok(None);
        }
        let artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
        let info = session.GetPlaybackInfo()?;
        let playing = info.PlaybackStatus().is_ok_and(|s| s == Status::Playing);
        let controls = info.Controls().ok();
        let flag = |f: fn(&Controls) -> Result<bool>| {
            controls.as_ref().and_then(|c| f(c).ok()).unwrap_or(false)
        };
        let art_url = art.get(&title, &artist, || read_art(&props).ok().flatten());
        Ok(Some(MediaState {
            title,
            artist,
            art_url,
            playing,
            can_prev: flag(|c| c.IsPreviousEnabled()),
            can_next: flag(|c| c.IsNextEnabled()),
            can_play_pause: flag(|c| c.IsPlayPauseToggleEnabled()),
        }))
    }

    fn apply(session: &Session, action: Action) {
        let op = match action {
            Action::PlayPause => session.TryTogglePlayPauseAsync(),
            Action::Next => session.TrySkipNextAsync(),
            Action::Previous => session.TrySkipPreviousAsync(),
        };
        // The bool result ("the player accepted it") is not useful: the refresh shows it.
        if let Err(e) = op.and_then(|op| op.join()) {
            eprintln!("lucent: media control failed: {e}");
        }
    }

    fn run(app: &AppHandle, tx: &Sender<Msg>, rx: &mpsc::Receiver<Msg>) {
        let manager = match Manager::RequestAsync().and_then(|op| op.join()) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("lucent: no media session manager: {e}");
                return crate::system::publish_media(app, None);
            }
        };
        let session_token = manager.CurrentSessionChanged(&refresher(tx));
        if let Err(e) = &session_token {
            eprintln!("lucent: cannot watch media sessions (polling only): {e}");
        }
        let mut watched: Option<Watched> = None;
        let mut art = ArtCache::default();
        let mut failing = false; // log a read error once, not every poll
        let mut refresh = |watched: &mut Option<Watched>, art: &mut ArtCache| {
            let current = manager.GetCurrentSession().ok();
            let same = match (&current, watched.as_ref()) {
                (Some(c), Some(w)) => *c == w.session,
                (None, None) => true,
                _ => false,
            };
            if !same {
                *watched = None; // remove the old tokens first
                *watched = current.map(|s| Watched::new(s, tx));
                *art = ArtCache::default(); // never keep the previous player's cover
            }
            let state = match watched.as_ref() {
                None => None,
                Some(w) => match read(&w.session, art) {
                    Ok(s) => {
                        failing = false;
                        s
                    }
                    Err(e) => {
                        if !failing {
                            eprintln!("lucent: cannot read media session: {e}");
                        }
                        failing = true;
                        None
                    }
                },
            };
            crate::system::publish_media(app, state);
        };
        // Poll and playback events keep the cached cover; a properties event re-reads it.
        let handle = |msg: Msg, watched: &Option<Watched>, art: &mut ArtCache| match msg {
            Msg::Refresh => {}
            Msg::PropertiesChanged => art.properties_changed(),
            Msg::Control(action) => {
                if let Some(w) = watched {
                    apply(&w.session, action);
                }
            }
        };
        refresh(&mut watched, &mut art);
        loop {
            match rx.recv_timeout(POLL) {
                Ok(msg) => handle(msg, &watched, &mut art),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            // Players raise several events per track change: one read for the whole burst.
            while let Ok(msg) = rx.try_recv() {
                handle(msg, &watched, &mut art);
            }
            refresh(&mut watched, &mut art);
        }
        drop(watched);
        if let Ok(t) = session_token {
            let _ = manager.RemoveCurrentSessionChanged(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(w, h));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn art_data_url_makes_96px_png() {
        let s = art_data_url(&png(200, 100)).expect("some data url");
        assert!(s.starts_with("data:image/png;base64,"));
        let b64 = &s["data:image/png;base64,".len()..];
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (96, 96));
    }

    /// CRC-32 (PNG chunk checksum), bitwise: enough for a hand-built header.
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }

    /// A PNG signature + IHDR declaring `w`×`h` RGBA, + an empty IDAT and IEND.
    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut chunk = |kind: &[u8; 4], data: &[u8]| {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
        };
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(b"IHDR", &ihdr);
        chunk(b"IDAT", &[]);
        chunk(b"IEND", &[]);
        out
    }

    #[test]
    fn art_data_url_rejects_huge_declared_size() {
        assert_eq!(art_data_url(&png_header(20000, 20000)), None);
    }

    #[test]
    fn art_data_url_rejects_side_over_4096() {
        // A real, decodable image: only the dimension limit can reject it.
        assert_eq!(art_data_url(&png(4097, 1)), None);
        assert_eq!(art_data_url(&png(1, 4097)), None);
        assert!(art_data_url(&png(4096, 2)).is_some());
    }

    #[test]
    fn art_data_url_rejects_garbage() {
        assert_eq!(art_data_url(b"not an image"), None);
        assert_eq!(art_data_url(&[]), None);
    }

    /// A reader that counts its calls and returns `url`.
    fn reader<'a>(calls: &'a std::cell::Cell<u32>, url: Option<&'a str>) -> impl FnOnce() -> Option<String> + 'a {
        move || {
            calls.set(calls.get() + 1);
            url.map(str::to_owned)
        }
    }

    #[test]
    fn art_cache_reuses_artwork_for_the_same_track() {
        let calls = std::cell::Cell::new(0);
        let mut c = ArtCache::default();
        assert_eq!(c.get("t", "a", reader(&calls, Some("u1"))), Some("u1".into()));
        assert_eq!(c.get("t", "a", reader(&calls, Some("u2"))), Some("u1".into()));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn art_cache_rereads_on_track_change() {
        let calls = std::cell::Cell::new(0);
        let mut c = ArtCache::default();
        c.get("t", "a", reader(&calls, Some("u1")));
        assert_eq!(c.get("t2", "a", reader(&calls, Some("u2"))), Some("u2".into()));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn art_cache_retries_a_missing_artwork_a_few_times() {
        let calls = std::cell::Cell::new(0);
        let mut c = ArtCache::default();
        for _ in 0..10 {
            assert_eq!(c.get("t", "a", reader(&calls, None)), None);
        }
        assert_eq!(calls.get(), u32::from(ART_RETRIES));
    }

    #[test]
    fn art_cache_properties_changed_rereads_the_cover() {
        let calls = std::cell::Cell::new(0);
        let mut c = ArtCache::default();
        c.get("t", "a", reader(&calls, Some("old")));
        c.properties_changed();
        assert_eq!(c.get("t", "a", reader(&calls, Some("new"))), Some("new".into()));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn art_cache_properties_changed_resets_retries() {
        let calls = std::cell::Cell::new(0);
        let mut c = ArtCache::default();
        for _ in 0..ART_RETRIES {
            c.get("t", "a", reader(&calls, None));
        }
        c.properties_changed();
        assert_eq!(c.get("t", "a", reader(&calls, Some("late"))), Some("late".into()));
    }

    #[test]
    fn art_cache_reset_forgets_previous_session_cover() {
        // Two players can show the same title/artist: a session switch must not keep the
        // previous player's cover.
        let calls = std::cell::Cell::new(0);
        let mut c = ArtCache::default();
        c.get("t", "a", reader(&calls, Some("spotify")));
        c = ArtCache::default();
        assert_eq!(c.get("t", "a", reader(&calls, Some("brave"))), Some("brave".into()));
    }

    #[test]
    fn displayable_needs_a_non_blank_title() {
        assert!(!is_displayable("  "));
        assert!(!is_displayable(""));
        assert!(is_displayable("Song"));
    }

    #[test]
    fn media_state_serialises_camel_case() {
        let s = MediaState {
            title: "t".into(),
            artist: "a".into(),
            art_url: None,
            playing: true,
            can_prev: true,
            can_next: false,
            can_play_pause: true,
        };
        let v = serde_json::to_value(&s).unwrap();
        for k in ["title", "artist", "artUrl", "playing", "canPrev", "canNext", "canPlayPause"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
    }
}
