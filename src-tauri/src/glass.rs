//! Automatic Liquid Glass tone: light glass over a light wallpaper, dark glass over a dark
//! one. The tone comes from the average luminance of the wallpaper area behind each dock,
//! read from the wallpaper file (never from a screen capture). Pure logic here; the
//! Win32 part (wallpaper path, worker thread, `dock://tone` emission) is in `win`.
use crate::monitors::Rect;
use crate::settings::GlassMode;
use image::RgbImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlassTone {
    Light,
    Dark,
}

impl GlassTone {
    /// `dock://tone` payload.
    pub fn as_str(self) -> &'static str {
        match self {
            GlassTone::Light => "light",
            GlassTone::Dark => "dark",
        }
    }
}

/// Above this average luminance the glass turns light.
const LIGHT_ABOVE: f32 = 0.6;
/// Below this average luminance the glass turns dark. In between: keep the previous tone.
const DARK_BELOW: f32 = 0.4;

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Relative luminance (0..1) of an sRGB colour: linearised, Rec. 709 weights.
pub fn relative_luminance(rgb: [u8; 3]) -> f32 {
    0.2126 * srgb_to_linear(rgb[0]) + 0.7152 * srgb_to_linear(rgb[1]) + 0.0722 * srgb_to_linear(rgb[2])
}

/// Hysteresis between `DARK_BELOW` and `LIGHT_ABOVE` so a mid-grey wallpaper never flickers.
pub fn tone_for(avg: f32, previous: GlassTone) -> GlassTone {
    if avg > LIGHT_ABOVE {
        GlassTone::Light
    } else if avg < DARK_BELOW {
        GlassTone::Dark
    } else {
        previous
    }
}

/// Mean relative luminance of the pixels of `img` inside `region` (image px, clamped to
/// the image). An empty intersection is neutral (0.5: keeps the previous tone).
pub fn region_average(img: &RgbImage, region: Rect) -> f32 {
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    let x0 = (region.x as i64).clamp(0, iw);
    let y0 = (region.y as i64).clamp(0, ih);
    let x1 = (region.x as i64 + region.w.max(0) as i64).clamp(0, iw);
    let y1 = (region.y as i64 + region.h.max(0) as i64).clamp(0, ih);
    if x0 >= x1 || y0 >= y1 {
        return 0.5;
    }
    // 256-entry table: one lookup per channel instead of three powf per pixel.
    let lut: Vec<f32> = (0..=255u8).map(srgb_to_linear).collect();
    let mut sum = 0.0f64;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x as u32, y as u32).0;
            sum += (0.2126 * lut[p[0] as usize] + 0.7152 * lut[p[1] as usize] + 0.0722 * lut[p[2] as usize]) as f64;
        }
    }
    (sum / ((x1 - x0) * (y1 - y0)) as f64) as f32
}

/// Maps a screen `area` on `monitor` to image pixels for an `img_w`×`img_h` wallpaper shown
/// with the "Fill" fit (scaled to cover the monitor, centred, overflow cropped).
pub fn fill_region(img_w: u32, img_h: u32, monitor: Rect, area: Rect) -> Rect {
    if img_w == 0 || img_h == 0 || monitor.w <= 0 || monitor.h <= 0 {
        return Rect::default();
    }
    let scale = (monitor.w as f64 / img_w as f64).max(monitor.h as f64 / img_h as f64);
    let off_x = (monitor.w as f64 - img_w as f64 * scale) / 2.0;
    let off_y = (monitor.h as f64 - img_h as f64 * scale) / 2.0;
    let to_x = |sx: i32| (((sx - monitor.x) as f64 - off_x) / scale).round() as i32;
    let to_y = |sy: i32| (((sy - monitor.y) as f64 - off_y) / scale).round() as i32;
    let (x0, y0) = (to_x(area.x), to_y(area.y));
    let (x1, y1) = (to_x(area.x + area.w), to_y(area.y + area.h));
    Rect { x: x0, y: y0, w: (x1 - x0).max(1), h: (y1 - y0).max(1) }
}

/// `GlassMode::Light`/`Dark` override the automatic tone.
pub fn effective_tone(mode: GlassMode, auto: GlassTone) -> GlassTone {
    match mode {
        GlassMode::Auto => auto,
        GlassMode::Light => GlassTone::Light,
        GlassMode::Dark => GlassTone::Dark,
    }
}

/// Long side of the cached, downscaled wallpaper (plenty for an average).
pub const CACHE_LONG_SIDE: u32 = 640;

/// Downscales a decoded wallpaper for the cache, keeping its aspect ratio.
pub fn downscale(img: &image::DynamicImage) -> RgbImage {
    let (w, h) = (img.width().max(1), img.height().max(1));
    let k = (CACHE_LONG_SIDE as f64 / w.max(h) as f64).min(1.0);
    let (tw, th) = (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1));
    img.thumbnail_exact(tw, th).to_rgb8()
}

/// Wallpaper luminance under `area` (screen px) on `monitor`. `cached` is the downscaled
/// wallpaper: it keeps the original aspect ratio, so the Fill mapping is the same.
pub fn area_luminance(cached: &RgbImage, monitor: Rect, area: Rect) -> f32 {
    region_average(cached, fill_region(cached.width(), cached.height(), monitor, area))
}

#[cfg(windows)]
pub mod win {
    //! Worker thread: decodes the wallpaper once per change (cached, downscaled), then
    //! computes and emits `dock://tone` to each dock window on layout/wallpaper/mode change.
    use super::{GlassTone, area_luminance, downscale, effective_tone, relative_luminance, tone_for};
    use crate::monitors::Rect;
    use crate::settings::GlassMode;
    use image::RgbImage;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::SystemTime;
    use tauri::{AppHandle, Emitter};
    use windows::Win32::Graphics::Gdi::{COLOR_DESKTOP, GetSysColor};
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETDESKWALLPAPER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };

    pub enum Msg {
        /// Dock window `label` sits at `area` (screen px) on a monitor at `monitor`.
        Dock { label: String, monitor: Rect, area: Rect },
        Forget(String),
        Mode(GlassMode),
        /// `WM_SETTINGCHANGE`: the wallpaper may have changed.
        SettingChange,
    }

    static TX: OnceLock<Sender<Msg>> = OnceLock::new();

    /// Last tone emitted per dock label, for popups that must match their dock.
    static LAST_TONE: std::sync::Mutex<Vec<(String, &'static str)>> = std::sync::Mutex::new(Vec::new());

    /// Tone currently shown by dock `label` (`"light"` until the first emission).
    pub fn last_tone(label: &str) -> &'static str {
        let v = LAST_TONE.lock().unwrap_or_else(|e| e.into_inner());
        v.iter().find(|(l, _)| l == label).map_or("light", |(_, t)| t)
    }

    pub fn send(msg: Msg) {
        if let Some(tx) = TX.get() {
            let _ = tx.send(msg);
        }
    }

    /// `fn()` callback for the shell listener's `WM_SETTINGCHANGE`.
    pub fn on_setting_change() {
        send(Msg::SettingChange);
    }

    pub fn start(app: &AppHandle, mode: GlassMode) {
        let (tx, rx) = mpsc::channel();
        if TX.set(tx).is_err() {
            return;
        }
        let app = app.clone();
        std::thread::Builder::new()
            .name("lucent-glass".into())
            .spawn(move || Worker::new(app, mode).run(rx))
            .expect("spawn glass thread");
    }

    #[derive(PartialEq, Clone)]
    struct Stamp {
        path: PathBuf,
        modified: Option<SystemTime>,
        len: u64,
    }

    enum Backdrop {
        Image(RgbImage),
        /// No wallpaper image (solid colour background, or unreadable file).
        Solid(f32),
    }

    struct DockEntry {
        monitor: Rect,
        area: Rect,
        auto: GlassTone,
    }

    struct Worker {
        app: AppHandle,
        mode: GlassMode,
        stamp: Option<Stamp>,
        backdrop: Option<Backdrop>,
        docks: HashMap<String, DockEntry>,
    }

    impl Worker {
        fn new(app: AppHandle, mode: GlassMode) -> Self {
            Self { app, mode, stamp: None, backdrop: None, docks: HashMap::new() }
        }

        fn run(mut self, rx: Receiver<Msg>) {
            while let Ok(msg) = rx.recv() {
                match msg {
                    Msg::Dock { label, monitor, area } => {
                        self.refresh_wallpaper(false);
                        let auto = self.docks.get(&label).map_or(GlassTone::Light, |d| d.auto);
                        self.docks.insert(label.clone(), DockEntry { monitor, area, auto });
                        self.update(&label);
                    }
                    Msg::Forget(label) => {
                        self.docks.remove(&label);
                    }
                    Msg::Mode(mode) => {
                        if mode != self.mode {
                            self.mode = mode;
                            self.update_all();
                        }
                    }
                    Msg::SettingChange => {
                        if self.refresh_wallpaper(true) {
                            self.update_all();
                        }
                    }
                }
            }
        }

        /// Re-decodes only when the wallpaper file (path, mtime, size) changed.
        /// Returns whether the backdrop changed.
        fn refresh_wallpaper(&mut self, recheck: bool) -> bool {
            if self.backdrop.is_some() && !recheck {
                return false;
            }
            let stamp = wallpaper_path().and_then(|path| {
                let meta = std::fs::metadata(&path).ok()?;
                Some(Stamp { path, modified: meta.modified().ok(), len: meta.len() })
            });
            if self.backdrop.is_some() && stamp == self.stamp {
                return false;
            }
            let backdrop = match &stamp {
                Some(s) => match decode(&s.path) {
                    Ok(img) => Backdrop::Image(img),
                    Err(e) => {
                        eprintln!("lucent: cannot decode wallpaper {}: {e}", s.path.display());
                        Backdrop::Solid(desktop_color_luminance())
                    }
                },
                None => Backdrop::Solid(desktop_color_luminance()),
            };
            self.stamp = stamp;
            self.backdrop = Some(backdrop);
            true
        }

        fn update_all(&mut self) {
            let labels: Vec<String> = self.docks.keys().cloned().collect();
            for l in labels {
                self.update(&l);
            }
        }

        fn update(&mut self, label: &str) {
            let Some(d) = self.docks.get_mut(label) else { return };
            let avg = match &self.backdrop {
                Some(Backdrop::Image(img)) => area_luminance(img, d.monitor, d.area),
                Some(Backdrop::Solid(l)) => *l,
                None => 0.5,
            };
            d.auto = tone_for(avg, d.auto);
            let tone = effective_tone(self.mode, d.auto);
            {
                let mut v = LAST_TONE.lock().unwrap_or_else(|e| e.into_inner());
                v.retain(|(l, _)| l != label);
                v.push((label.to_string(), tone.as_str()));
            }
            if let Err(e) = self.app.emit_to(label, "dock://tone", tone.as_str()) {
                eprintln!("lucent: cannot emit dock://tone to {label}: {e}");
            }
        }
    }

    fn decode(path: &std::path::Path) -> Result<RgbImage, String> {
        // `TranscodedWallpaper` has no extension: sniff the format from the bytes.
        let img = image::ImageReader::open(path)
            .map_err(|e| e.to_string())?
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .decode()
            .map_err(|e| e.to_string())?;
        Ok(downscale(&img))
    }

    fn wallpaper_path() -> Option<PathBuf> {
        let mut buf = [0u16; 1024];
        unsafe {
            SystemParametersInfoW(
                SPI_GETDESKWALLPAPER,
                buf.len() as u32,
                Some(buf.as_mut_ptr().cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
            .ok()?;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    }

    fn desktop_color_luminance() -> f32 {
        let c = unsafe { GetSysColor(COLOR_DESKTOP) }; // 0x00BBGGRR
        relative_luminance([(c & 0xff) as u8, ((c >> 8) & 0xff) as u8, ((c >> 16) & 0xff) as u8])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn white_is_light_black_is_dark() {
        assert_eq!(tone_for(relative_luminance([255; 3]), GlassTone::Dark), GlassTone::Light);
        assert_eq!(tone_for(relative_luminance([0; 3]), GlassTone::Light), GlassTone::Dark);
    }

    #[test]
    fn mid_luminance_keeps_previous() {
        assert_eq!(tone_for(0.5, GlassTone::Dark), GlassTone::Dark);
        assert_eq!(tone_for(0.5, GlassTone::Light), GlassTone::Light);
    }

    #[test]
    fn region_average_only_reads_region() {
        let img = RgbImage::from_fn(100, 50, |x, _| if x < 50 { Rgb([0, 0, 0]) } else { Rgb([255, 255, 255]) });
        let white = region_average(&img, Rect { x: 60, y: 10, w: 30, h: 30 });
        assert!((white - 1.0).abs() < 1e-3, "white half: {white}");
        let black = region_average(&img, Rect { x: 0, y: 0, w: 50, h: 50 });
        assert!(black < 1e-3, "black half: {black}");
    }

    #[test]
    fn region_average_clamps_and_handles_empty() {
        let img = RgbImage::from_pixel(10, 10, Rgb([255, 255, 255]));
        assert!((region_average(&img, Rect { x: -5, y: 5, w: 100, h: 100 }) - 1.0).abs() < 1e-3);
        assert_eq!(region_average(&img, Rect { x: 20, y: 20, w: 5, h: 5 }), 0.5);
    }

    #[test]
    fn luminance_uses_rec709_weights() {
        assert!((relative_luminance([0, 255, 0]) - 0.7152).abs() < 1e-4);
        assert!((relative_luminance([128, 128, 128]) - 0.2158).abs() < 1e-3);
    }

    #[test]
    fn fill_region_maps_same_aspect_and_crops_wider_image() {
        let mon = Rect { x: 1920, y: 0, w: 1920, h: 1080 };
        // Same aspect, half resolution: plain scale by 0.5.
        let r = fill_region(960, 540, mon, Rect { x: 1920 + 800, y: 1000, w: 320, h: 80 });
        assert_eq!(r, Rect { x: 400, y: 500, w: 160, h: 40 });
        // 2:1 image on 16:9: height fits (scale 1080/1000), sides cropped.
        let r = fill_region(2000, 1000, mon, Rect { x: 1920, y: 0, w: 1920, h: 1080 });
        let crop = ((2000.0 - 1920.0 / 1.08) / 2.0_f64).round() as i32;
        assert_eq!(r.x, crop);
        assert_eq!(r.y, 0);
        assert_eq!(r.h, 1000);
    }

    #[test]
    fn mode_overrides_auto() {
        assert_eq!(effective_tone(GlassMode::Auto, GlassTone::Dark), GlassTone::Dark);
        assert_eq!(effective_tone(GlassMode::Light, GlassTone::Dark), GlassTone::Light);
        assert_eq!(effective_tone(GlassMode::Dark, GlassTone::Light), GlassTone::Dark);
    }

    #[test]
    fn downscale_keeps_aspect() {
        let img = image::DynamicImage::ImageRgb8(RgbImage::new(3840, 2160));
        let d = downscale(&img);
        assert_eq!((d.width(), d.height()), (640, 360));
    }

    #[test]
    fn dock_area_on_bright_bottom_is_light() {
        // Top half dark, bottom half white: a bottom dock sees white.
        let img = RgbImage::from_fn(160, 90, |_, y| if y < 45 { Rgb([10, 10, 10]) } else { Rgb([250, 250, 250]) });
        let mon = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let lum = area_luminance(&img, mon, Rect { x: 600, y: 990, w: 720, h: 80 });
        assert_eq!(tone_for(lum, GlassTone::Dark), GlassTone::Light);
        let lum = area_luminance(&img, mon, Rect { x: 600, y: 10, w: 720, h: 80 });
        assert_eq!(tone_for(lum, GlassTone::Light), GlassTone::Dark);
    }
}
