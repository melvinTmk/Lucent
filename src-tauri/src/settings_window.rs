//! The settings window: created on demand, destroyed on close (it counts against the RAM
//! budget). Closing it never quits the app (`ExitRequested{code:None}` is prevented in `run`).
use tauri::window::{Effect, EffectsBuilder};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "settings";

/// Focuses the window if it exists, else builds it. Must not be called from the main thread
/// (building a webview there deadlocks): use from an async command or `open_async`.
pub fn open_tab(app: &AppHandle, tab: Option<&str>) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        if let Some(t) = tab {
            let _ = w.emit("settings://tab", t);
        }
        return Ok(());
    }
    // Mica behind a transparent webview; where Mica is unavailable (Windows 10) the page's
    // own translucent-to-solid CSS background stays readable.
    let url = match tab {
        Some(t) => format!("settings.html?tab={t}"),
        None => "settings.html".into(),
    };
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(url.into()))
        .title("Réglages de Lucent")
        .inner_size(720.0, 520.0)
        .resizable(false)
        .maximizable(false)
        .decorations(true)
        .transparent(true)
        .center()
        .effects(EffectsBuilder::new().effect(Effect::Mica).build())
        .build()?;
    Ok(())
}

pub fn open(app: &AppHandle) -> tauri::Result<()> {
    open_tab(app, None)
}

/// For event handlers that run on the main thread: build on a worker thread.
pub fn open_async(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = open(&app) {
            eprintln!("lucent: cannot open settings window: {e}");
        }
    });
}
