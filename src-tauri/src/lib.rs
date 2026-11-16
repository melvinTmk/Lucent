pub mod actions;
pub mod autostart;
#[cfg(windows)]
pub mod backdrop;
pub mod cli;
pub mod diag;
pub mod dock_drop;
pub mod dock_host;
pub mod dock_model;
pub mod game;
pub mod glass;
pub mod hotkey;
pub mod icons;
pub mod monitors;
pub mod paths;
pub mod region;
pub mod reveal;
pub mod search;
pub mod search_window;
pub mod settings;
pub mod settings_window;
pub mod single_instance;
pub mod sysmon;
pub mod system;
pub mod taskbar;
pub mod tray;
pub mod valorant;
pub mod thumbs;
pub mod recycle_bin;
pub mod stack;
pub mod wintrack;
pub mod switcher;
pub mod overview;
pub mod snap;
pub mod corners;
pub mod power_menu;

use settings::{Settings, SettingsStore};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Listener, Manager, State};
use wintrack::WindowTracker;

/// All native-menu strings (French). Menus are built in Rust, so they cannot live in
/// `src/shared/i18n.ts`; keep them in this single block.
mod menu_text {
    pub const PIN: &str = "Garder dans le Dock";
    pub const UNPIN: &str = "Retirer du Dock";
    pub const QUIT: &str = "Quitter";
    pub const NEW_WINDOW: &str = "Nouvelle fenêtre";
    pub const END_TASK: &str = "Terminer la tâche";
    pub const TASK_MANAGER: &str = "Gestionnaire des tâches";
    pub const SETTINGS: &str = "Réglages…";
    pub const EXIT_APP: &str = "Quitter Lucent";
}

const MENU_SETTINGS_ID: &str = "dock:settings";
const MENU_EXIT_ID: &str = "app:exit";
const MENU_TASK_MANAGER_ID: &str = "app:taskmgr";

/// Current dock items from live settings + tracked windows.
pub(crate) fn current_items(app: &AppHandle) -> Vec<dock_model::DockItem> {
    let pinned = app.state::<SettingsStore>().get().dock.pinned;
    let windows = app.state::<Arc<WindowTracker>>().snapshot();
    dock_model::build_items(&pinned, &windows, &actions::display_name)
}

/// Rebuild and broadcast `dock://items`. The dock window (Task 6) subscribes later.
fn emit_items(app: &AppHandle) {
    if let Err(e) = app.emit("dock://items", current_items(app)) {
        eprintln!("lucent: cannot emit dock://items: {e}");
    }
}

/// Current dock items; the page calls it on boot so it never misses the first list.
#[tauri::command]
fn get_dock_items(app: AppHandle) -> Vec<dock_model::DockItem> {
    current_items(&app)
}

/// `monitor_id` is the clicked dock's monitor (the thumbnails open above that dock).
#[tauri::command]
fn dock_click(app: AppHandle, item_id: String, monitor_id: String) {
    let Some(item) = current_items(&app).into_iter().find(|i| i.id == item_id) else { return };
    match dock_model::click_outcome(&item) {
        dock_model::ClickOutcome::Launch(a) => {
            close_thumbs();
            actions::launch_detached(a);
        }
        dock_model::ClickOutcome::Focus(h) => {
            close_thumbs();
            actions::focus(h);
        }
        dock_model::ClickOutcome::ShowThumbs(item_id) => {
            let windows = item.windows.iter().map(|w| (w.hwnd, w.title.clone())).collect();
            #[cfg(windows)]
            dock_host::send(dock_host::Msg::ShowThumbs { item_id, monitor_id, windows });
            #[cfg(not(windows))]
            let _ = (item_id, monitor_id, windows);
        }
    }
}

fn close_thumbs() {
    #[cfg(windows)]
    dock_host::send(dock_host::Msg::CloseThumbs);
}

/// Hover preview (dock page's hover machine): the pointer rested on running item
/// `item_id`, or moved to it while a popup was open. Opens even for a single window.
#[tauri::command]
fn dock_hover(app: AppHandle, item_id: String, monitor_id: String) {
    let Some(item) = current_items(&app).into_iter().find(|i| i.id == item_id) else { return };
    let windows: Vec<(isize, String)> = item.windows.iter().map(|w| (w.hwnd, w.title.clone())).collect();
    if windows.is_empty() {
        return;
    }
    #[cfg(windows)]
    dock_host::send(dock_host::Msg::HoverThumbs { item_id, monitor_id, windows });
    #[cfg(not(windows))]
    let _ = (monitor_id, windows);
}

/// The pointer left both the icon and the popup for the grace delay.
#[tauri::command]
fn thumbs_close() {
    close_thumbs();
}

/// The pointer entered / left the thumbnails popup: relayed to the docks' hover machines.
#[tauri::command]
fn thumbs_pointer(app: AppHandle, inside: bool) {
    if let Err(e) = app.emit("thumbs://pointer", inside) {
        eprintln!("lucent: cannot emit thumbs://pointer: {e}");
    }
}

/// Files dropped on a dock icon: open them with that app. Worker thread (COM, may block).
#[tauri::command]
fn dock_drop_open(app: AppHandle, item_id: String, paths: Vec<String>) {
    let Some(item) = current_items(&app).into_iter().find(|i| i.id == item_id) else { return };
    std::thread::spawn(move || {
        if let Err(e) = dock_drop::open(&item.app, &paths) {
            eprintln!("lucent: drop on {}: {e}", item.name);
        }
    });
}

/// Spring-loading: a drag rested on a running app, bring its most recent window forward.
#[tauri::command]
fn dock_spring(app: AppHandle, item_id: String) {
    let Some(item) = current_items(&app).into_iter().find(|i| i.id == item_id) else { return };
    let hwnds: Vec<isize> = item.windows.iter().map(|w| w.hwnd).collect();
    dock_drop::spring(&hwnds);
}

/// A card was clicked: bring that window to the front (restoring it if minimised), close.
#[tauri::command]
fn thumbs_pick(hwnd: isize) {
    actions::focus(hwnd);
    close_thumbs();
}

/// Shows the native context menu; the choice is applied in `on_dock_menu_event`.
/// `None` (click on the dock background) shows only the settings and quit entries.
#[tauri::command]
fn dock_menu(app: AppHandle, window: tauri::Window, item_id: Option<String>) -> Result<(), String> {
    use tauri::menu::{ContextMenu, Menu, MenuItem, PredefinedMenuItem, Submenu};
    let item = match &item_id {
        Some(id) => current_items(&app).into_iter().find(|i| &i.id == id),
        None => None,
    };
    let e = |e: tauri::Error| e.to_string();
    let menu = Menu::new(&app).map_err(e)?;
    if let Some(item) = &item {
        let (pin_id, pin_label) = if item.pinned {
            ("unpin", menu_text::UNPIN)
        } else {
            ("pin", menu_text::PIN)
        };
        menu.append(&MenuItem::with_id(&app, format!("dock:{pin_id}:{}", item.id), pin_label, true, None::<&str>).map_err(e)?).map_err(e)?;
        if !item.windows.is_empty() {
            menu.append(&MenuItem::with_id(&app, format!("dock:new:{}", item.id), menu_text::NEW_WINDOW, true, None::<&str>).map_err(e)?).map_err(e)?;
            menu.append(&PredefinedMenuItem::separator(&app).map_err(e)?).map_err(e)?;
            menu.append(&MenuItem::with_id(&app, format!("dock:quit:{}", item.id), menu_text::QUIT, true, None::<&str>).map_err(e)?).map_err(e)?;
            menu.append(&MenuItem::with_id(&app, format!("dock:kill:{}", item.id), menu_text::END_TASK, true, None::<&str>).map_err(e)?).map_err(e)?;
        }
        menu.append(&PredefinedMenuItem::separator(&app).map_err(e)?).map_err(e)?;
    }
    menu.append(&MenuItem::with_id(&app, MENU_TASK_MANAGER_ID, menu_text::TASK_MANAGER, true, None::<&str>).map_err(e)?).map_err(e)?;
    menu.append(&MenuItem::with_id(&app, MENU_SETTINGS_ID, menu_text::SETTINGS, true, None::<&str>).map_err(e)?).map_err(e)?;
    if item.is_none() {
        let entries = power_menu::menu_spec()
            .into_iter()
            .map(|m| MenuItem::with_id(&app, m.id, m.label, true, None::<&str>))
            .collect::<Result<Vec<_>, _>>()
            .map_err(e)?;
        let refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = entries.iter().map(|i| i as _).collect();
        menu.append(&Submenu::with_items(&app, power_menu::text::SUBMENU, true, &refs).map_err(e)?).map_err(e)?;
        menu.append(&MenuItem::with_id(&app, MENU_EXIT_ID, menu_text::EXIT_APP, true, None::<&str>).map_err(e)?).map_err(e)?;
    }
    menu.popup(window).map_err(e)
}

fn on_dock_menu_event(app: &AppHandle, id: &str) {
    use dock_model::ContextAction;
    if id == MENU_SETTINGS_ID {
        settings_window::open_async(app);
        return;
    }
    if id == MENU_TASK_MANAGER_ID {
        actions::open_task_manager();
        return;
    }
    if let Some(action) = power_menu::action_from_menu_id(id) {
        // Native OK/Cancel box first for the destructive ones. Only ever from a menu click.
        #[cfg(windows)]
        power_menu::win::spawn(action, true);
        #[cfg(not(windows))]
        let _ = action;
        return;
    }
    if id == MENU_EXIT_ID {
        // `RunEvent::Exit` restores the taskbar and gives the tray icons back.
        app.exit(0);
        return;
    }
    let mut parts = id.splitn(3, ':');
    let (Some("dock"), Some(verb), Some(item_id)) = (parts.next(), parts.next(), parts.next()) else { return };
    let action = match verb {
        "pin" => ContextAction::Pin,
        "unpin" => ContextAction::Unpin,
        "quit" => ContextAction::Quit,
        "new" | "kill" => {
            let Some(item) = current_items(app).into_iter().find(|i| i.id == item_id) else { return };
            if verb == "kill" {
                actions::end_task(&item.windows);
            } else {
                actions::launch_detached(item.app.clone());
            }
            return;
        }
        _ => return,
    };
    let Some(item) = current_items(app).into_iter().find(|i| i.id == item_id) else { return };
    if action == ContextAction::Quit {
        actions::quit(&item.windows);
        return;
    }
    let store = app.state::<SettingsStore>();
    let mut s = store.get();
    s.dock.pinned = dock_model::apply_context(std::mem::take(&mut s.dock.pinned), &item, action);
    if let Err(e) = store.update(app, s) {
        eprintln!("lucent: cannot save pins: {e}");
    }
}

/// Power menu entry point (Control Center). The UI confirms restart / shut down / sign out
/// itself before calling; unknown names are rejected. Never bound to a hotkey.
#[tauri::command]
fn power_action(action: String) -> Result<(), String> {
    let action = power_menu::PowerAction::parse(&action).ok_or_else(|| format!("unknown power action {action:?}"))?;
    #[cfg(windows)]
    power_menu::win::spawn(action, false);
    #[cfg(not(windows))]
    let _ = action;
    Ok(())
}

/// Drag-reorder from the dock page (`before_id` None = end of the section). Persists the pins.
#[tauri::command]
fn dock_reorder(app: AppHandle, item_id: String, before_id: Option<String>) {
    let items = current_items(&app);
    let store = app.state::<SettingsStore>();
    let mut s = store.get();
    s.dock.pinned = dock_model::reorder(std::mem::take(&mut s.dock.pinned), &items, &item_id, before_id.as_deref());
    if let Err(e) = store.update(&app, s) {
        eprintln!("lucent: cannot save pin order: {e}");
    }
}

/// The dock page reports its bounding size and island rects (physical px, relative to
/// the window): the dock window is resized/re-placed and clipped to the islands. `pill`:
/// only the Game Mode system pill is shown (the hidden dock window then stays on-screen and
/// holds the system monitor, see `dock_host::show_pill`).
#[tauri::command]
fn report_dock_layout(monitor_id: String, width: u32, height: u32, islands: Vec<monitors::Rect>, hit: Vec<monitors::Rect>, pill: bool) {
    #[cfg(windows)]
    dock_host::send(dock_host::Msg::Layout { monitor_id, width, height, islands, hit, pill });
    #[cfg(not(windows))]
    let _ = (monitor_id, width, height, islands, hit, pill);
}

/// Current tray icons; the dock page calls it on boot (then follows `tray://icons`).
#[tauri::command]
fn get_tray_icons() -> Vec<tray::registry::TrayIconView> {
    #[cfg(windows)]
    return tray::host::win::views();
    #[cfg(not(windows))]
    Vec::new()
}

/// Click on a tray icon. `(x, y)`: icon centre in physical px relative to the dock window.
#[tauri::command]
fn tray_click(window: tauri::Window, key: String, button: String, x: i32, y: i32) {
    use tray::input::TrayClick;
    let click = match button.as_str() {
        "left" => TrayClick::Left,
        "right" => TrayClick::Right,
        "double" => TrayClick::Double,
        _ => return,
    };
    #[cfg(windows)]
    if let Ok(h) = window.hwnd() {
        tray::host::win::click(h.0 as isize, &key, click, x, y);
    }
    #[cfg(not(windows))]
    let _ = (window, key, click, x, y);
}

/// Volume + network, pulled by the dock page on boot (then it follows `system://*`).
#[tauri::command]
fn get_system_state() -> system::SystemState {
    system::snapshot()
}

/// Sets the master volume (0..1). Non-blocking: the volume thread applies it.
#[tauri::command]
fn set_volume(level: f32) {
    #[cfg(windows)]
    system::volume::win::set_volume(level);
    #[cfg(not(windows))]
    let _ = level;
}

#[tauri::command]
fn toggle_mute() {
    #[cfg(windows)]
    system::volume::win::toggle_mute();
}

/// Toggles the default communications mic mute (Control Center, dock mic glyph).
/// Non-blocking: the mic watcher applies it, emits `audio://devices` and peeks the dock.
#[tauri::command]
fn mic_toggle() {
    #[cfg(windows)]
    system::mic::win::toggle();
}

/// Current system media session (null when none); the page then follows `media://state`.
#[tauri::command]
fn get_media() -> Option<system::media::MediaState> {
    system::media_snapshot()
}

/// `"play_pause" | "next" | "previous"` (anything else is ignored). Non-blocking.
#[tauri::command]
fn media_control(action: String) {
    #[cfg(windows)]
    system::media::win::control(&action);
    #[cfg(not(windows))]
    let _ = action;
}

/// Both `open_*` hand the ShellExecute call to a worker thread, so they never block.
#[tauri::command]
fn open_notifications() {
    system::open::open_notifications();
}

#[tauri::command]
fn open_network() {
    system::open::open_network();
}

// ---- Control Center (M6 Track C) ------------------------------------------------
// Every cc_* command only queues work for the control worker, which emits `cc://state`.

/// Reads the radios, theme and brightness again and emits `cc://state`.
#[tauri::command]
fn cc_refresh(app: AppHandle) {
    #[cfg(windows)]
    system::control::win::refresh(&app);
    #[cfg(not(windows))]
    let _ = app;
}

#[tauri::command]
fn cc_set_wifi(app: AppHandle, on: bool) {
    #[cfg(windows)]
    system::control::win::set_wifi(&app, on);
    #[cfg(not(windows))]
    let _ = (app, on);
}

#[tauri::command]
fn cc_set_bluetooth(app: AppHandle, on: bool) {
    #[cfg(windows)]
    system::control::win::set_bluetooth(&app, on);
    #[cfg(not(windows))]
    let _ = (app, on);
}

#[tauri::command]
fn cc_set_dark(app: AppHandle, on: bool) {
    #[cfg(windows)]
    system::control::win::set_dark(&app, on);
    #[cfg(not(windows))]
    let _ = (app, on);
}

/// `value` 0..100 (clamped), applied to the monitor under the cursor at the last refresh.
#[tauri::command]
fn cc_set_brightness(app: AppHandle, value: f64) {
    let v = system::control::clamp_brightness(value);
    #[cfg(windows)]
    system::control::win::set_brightness(&app, v);
    #[cfg(not(windows))]
    let _ = (app, v);
}

/// Shared monitor: `to_other` None = toggle. Non-blocking (DDC worker).
#[tauri::command]
fn display_switch(app: AppHandle, to_other: Option<bool>) {
    #[cfg(windows)]
    system::display_input::win::switch(app.state::<SettingsStore>().get().display_switch, to_other);
    #[cfg(not(windows))]
    let _ = (app, to_other);
}

/// DDC/CI monitors and their current input (Réglages › Écran partagé). Bounded wait.
#[tauri::command]
async fn list_ddc_monitors() -> Vec<system::display_input::DdcMonitor> {
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(|| system::display_input::win::list_blocking(std::time::Duration::from_secs(4)))
        .await
        .unwrap_or_default();
    #[cfg(not(windows))]
    Vec::new()
}

/// Makes `id` the default output (console, multimedia and communications roles).
#[tauri::command]
fn audio_set_output(id: String) {
    #[cfg(windows)]
    system::audio_out::win::set_output(id);
    #[cfg(not(windows))]
    let _ = id;
}

/// Current `audio://devices` payload (the dock pulls it on boot, then follows the event).
#[tauri::command]
fn get_audio_devices() -> system::AudioDevices {
    system::audio_snapshot()
}

#[tauri::command]
fn get_settings(store: State<'_, SettingsStore>) -> Settings {
    store.get()
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    store: State<'_, SettingsStore>,
    settings: Settings,
) -> Result<(), String> {
    store.update(&app, settings)
}

/// Answer to the "is this a game?" prompt: persisted in include/exclude, then handed to
/// the game session (a "yes" for the foreground exe starts the session).
#[tauri::command]
fn game_answer(app: AppHandle, store: State<'_, SettingsStore>, exe: String, is_game: bool) -> Result<(), String> {
    let exe = exe.trim().to_lowercase();
    let mut s = store.get();
    game::session::apply_answer(&mut s.game_mode, &exe, is_game);
    store.update(&app, s)?;
    game::service::send(game::service::Msg::Answer { exe, is_game });
    Ok(())
}

/// Installed games (launcher libraries + manual includes). Discovery may hit the disk.
#[tauri::command]
async fn list_detected_games(app: AppHandle) -> Result<Vec<game::library::DetectedGame>, String> {
    let include = app.state::<SettingsStore>().get().game_mode.include;
    tauri::async_runtime::spawn_blocking(move || game::service::detected_games(&include))
        .await
        .map_err(|e| e.to_string())
}

/// Drops WSL's page cache when a distro already runs (never starts WSL). French one-liner.
#[tauri::command]
async fn wsl_free() -> String {
    tauri::async_runtime::spawn_blocking(sysmon::wsl_free).await.unwrap_or_else(|e| {
        eprintln!("lucent: wsl_free task failed: {e}");
        "Impossible de libérer la mémoire de WSL".into()
    })
}

/// One HenrikDev MMR call with the saved settings. French one-liner.
#[tauri::command]
async fn valo_test(app: AppHandle) -> String {
    let settings = app.state::<SettingsStore>().get().valorant;
    tauri::async_runtime::spawn_blocking(move || {
        let line = valorant::api::test_line(&settings);
        // The key works: show the last match's card in the dock (pictures included).
        if line.starts_with("OK") {
            let app = app.clone();
            std::thread::spawn(move || valorant::watch::preview_last_match(&app));
        }
        line
    })
    .await
    .unwrap_or_else(|e| {
        eprintln!("lucent: valo_test task failed: {e}");
        "API injoignable".into()
    })
}

/// `wsl --shutdown` when something runs (the UI confirms first). French one-liner.
#[tauri::command]
async fn wsl_shutdown() -> String {
    tauri::async_runtime::spawn_blocking(sysmon::wsl_shutdown).await.unwrap_or_else(|e| {
        eprintln!("lucent: wsl_shutdown task failed: {e}");
        "Impossible d'arrêter WSL".into()
    })
}

/// Current game session; pages call it on boot, then follow `game://state`.
#[tauri::command]
fn get_game_state() -> game::session::GameStateDto {
    game::service::state()
}

// The three below hit the shell / disk (a big recycle bin or Downloads folder takes
// seconds): async + blocking pool, never on the main thread.
#[tauri::command]
async fn get_trash_status() -> Result<recycle_bin::TrashStatus, String> {
    tauri::async_runtime::spawn_blocking(recycle_bin::query_recycle_bin).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn empty_trash() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(recycle_bin::empty_recycle_bin).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_stack_files(folder: String, limit: Option<usize>) -> Result<Vec<stack::StackFile>, String> {
    tauri::async_runtime::spawn_blocking(move || stack::read_folder_stack(&folder, limit.unwrap_or(8)))
        .await
        .map_err(|e| e.to_string())?
}

/// Current search-hotkey registration state (the Réglages page pulls it, then follows
/// `hotkey://status`).
#[tauri::command]
fn get_hotkey_status() -> hotkey::HotkeyStatus {
    hotkey::status()
}

/// Every wanted hotkey's status, keyed by `HotkeyId` (camelCase); then `hotkeys://status`.
#[tauri::command]
fn get_hotkey_statuses() -> std::collections::BTreeMap<hotkey::HotkeyId, hotkey::HotkeyStatus> {
    hotkey::statuses()
}

fn emit_hotkey_status(app: &AppHandle) {
    if let Err(e) = app.emit("hotkey://status", hotkey::status()) {
        eprintln!("lucent: cannot emit hotkey://status: {e}");
    }
    if let Err(e) = app.emit("hotkeys://status", hotkey::statuses()) {
        eprintln!("lucent: cannot emit hotkeys://status: {e}");
    }
}

/// Registers the wanted combos, logs refusals (never fatal) and broadcasts the statuses.
fn apply_hotkeys(app: &AppHandle, map: hotkey::HotkeyMap) {
    if let Err(e) = hotkey::set_all(map) {
        eprintln!("lucent: {e}");
    }
    emit_hotkey_status(app);
}

/// Shared actions reachable from hotkeys, hot corners and menus (M6). Feature tracks fill
/// in the logged no-ops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Overview,
    Desktop,
    Lock,
    Search,
    Switcher,
}

impl Action {
    /// Hot-corner setting value (`settings::CORNER_ACTIONS`) -> action; "none"/unknown -> None.
    pub fn from_corner(s: &str) -> Option<Action> {
        match s {
            "overview" => Some(Action::Overview),
            "desktop" => Some(Action::Desktop),
            "lock" => Some(Action::Lock),
            "search" => Some(Action::Search),
            "switcher" => Some(Action::Switcher),
            _ => None,
        }
    }
}

/// Runs `action`. Never blocks (safe from the hotkey thread and polling loops).
pub fn run_action(app: &AppHandle, action: Action) {
    match action {
        Action::Search => search_window::show(app),
        Action::Overview => overview::toggle(app),
        // No Alt held (hot corner): the switcher stays open until a pick or Échap.
        Action::Switcher => switcher::open(app, false, true),
        #[cfg(windows)]
        Action::Desktop => corners::win::toggle_desktop(),
        #[cfg(windows)]
        Action::Lock => corners::win::lock(),
        #[cfg(not(windows))]
        Action::Desktop | Action::Lock => {}
    }
}

/// Hotkey thread callback: every id but Search is inert while a game is focused (they are
/// also unregistered then; this covers the race), then the id's handler runs.
fn on_hotkey(id: hotkey::HotkeyId) {
    if id != hotkey::HotkeyId::Search {
        let g = game::service::state();
        if g.active && g.focused {
            return;
        }
    }
    if !hotkey::dispatch(id) {
        eprintln!("lucent: no handler for hotkey {id:?}");
    }
}

/// « Quitter Lucent » (settings, Général). `RunEvent::Exit` restores the taskbar.
#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// The thumbnails page is loaded and listening: a pending first request can be shown.
#[tauri::command]
fn thumbs_ready() {
    #[cfg(windows)]
    dock_host::send(dock_host::Msg::ThumbsReady);
}

/// Échap in the search page.
#[tauri::command]
fn search_hide(app: AppHandle) {
    search_window::hide(&app);
}

/// Ranked results for `q`, off the IPC thread (the index can hold hundreds of thousands of files).
#[tauri::command]
async fn search_query(app: AppHandle, q: String) -> Result<Vec<search::Group>, String> {
    let index = app.state::<search::SearchIndex>().inner().clone();
    let history = app.state::<search::SharedHistory>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let h = history.lock().unwrap_or_else(|e| e.into_inner());
        index.query(&q, &h, search::LIMIT_PER_KIND)
    })
    .await
    .map_err(|e| e.to_string())
}

/// Validates a result: bumps the history and runs the opener of its kind. `q` is the query
/// of the response the selected row came from (dynamic entries are recomputed from it). The
/// page hides the window afterwards.
#[tauri::command]
async fn search_open(app: AppHandle, id: String, q: String, shift: bool, ctrl: bool) -> Result<(), String> {
    let index = app.state::<search::SearchIndex>().inner().clone();
    let history = app.state::<search::SharedHistory>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        search::open(&index, &history, &id, &q, search::OpenMods { shift, ctrl })
    })
    .await
    .map_err(|e| e.to_string())
}

/// The search page reports its content height (CSS px); the window follows (clamped).
#[tauri::command]
fn search_resize(app: AppHandle, height: u32) {
    search_window::resize(&app, height);
}

/// Creates (or focuses) the settings window. Async so the build never runs on the main thread.
#[tauri::command]
async fn open_settings(app: AppHandle, tab: Option<String>) -> Result<(), String> {
    settings_window::open_tab(&app, tab.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_external(url: String) {
    let allowed = url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("ms-settings:")
        || url.starts_with("shell:")
        || std::path::Path::new(&url).exists();
    if allowed {
        system::open::shell_open_str(&url);
    }
}

#[tauri::command]
fn list_monitors() -> Vec<monitors::MonitorInfo> {
    #[cfg(windows)]
    {
        monitors::win::enumerate()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Process-wide taskbar guard, reachable from the panic hook, the console handler,
/// the shell listener thread, the gate worker and `RunEvent::Exit`.
///
/// Hide/restore decisions go through one worker thread (`HideGate`): the taskbar is hidden
/// only while the setting is on *and* a dock window is up, and only on transitions, never
/// from the main thread or the dock host thread.
#[cfg(windows)]
mod taskbar_glue {
    use crate::settings::Settings;
    use crate::taskbar::{self, HideGate, TaskbarGuard, win::Win32Taskbar};
    use crate::tray::host::win as tray_host;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Mutex, OnceLock, TryLockError};
    use tauri::{AppHandle, Listener};

    type Guard = TaskbarGuard<Win32Taskbar>;
    static GUARD: OnceLock<Mutex<Guard>> = OnceLock::new();
    /// Hiding is only allowed once the watchdog is running.
    static WATCHDOG_OK: AtomicBool = AtomicBool::new(false);
    /// Set on the way out: nothing may hide the taskbar (or start the tray host) anymore.
    static EXITING: AtomicBool = AtomicBool::new(false);
    static GATE: OnceLock<Sender<GateMsg>> = OnceLock::new();

    enum GateMsg {
        /// `hide_windows_taskbar` from a settings save.
        Want(bool),
        /// A dock window is up / no dock window left.
        DocksReady(bool),
        /// Explorer (re)created its taskbar.
        TaskbarCreated,
        /// The tray host thread ended without `stop()`.
        TrayHostDied,
    }

    fn send(msg: GateMsg) {
        if let Some(tx) = GATE.get() {
            let _ = tx.send(msg);
        }
    }

    fn with_guard(f: impl FnOnce(&mut Guard)) {
        if let Some(m) = GUARD.get() {
            f(&mut m.lock().unwrap_or_else(|e| e.into_inner()));
        }
    }

    /// Gives the tray icons back to Explorer first (our `Shell_TrayWnd` destroyed), then
    /// shows the taskbar, both under the guard lock so a concurrent `hide` cannot start the
    /// tray host in between. The tray host never takes the guard lock: no deadlock.
    pub fn restore() {
        match GUARD.get() {
            Some(m) => {
                let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
                tray_host::stop();
                g.restore();
            }
            None => tray_host::stop(),
        }
    }

    /// Final restore (app exit, session end, console close): nothing re-hides afterwards.
    pub fn restore_for_exit() {
        EXITING.store(true, Ordering::SeqCst);
        restore();
        // Game mode's High performance plan goes back too (same exit paths).
        crate::game::power::restore_for_exit();
    }

    fn is_hidden() -> bool {
        GUARD.get().is_some_and(|m| m.lock().unwrap_or_else(|e| e.into_inner()).is_hidden())
    }

    /// Hides the taskbar, then hosts the tray icons in the dock (only once really hidden).
    fn hide() {
        if !WATCHDOG_OK.load(Ordering::SeqCst) {
            eprintln!("lucent: watchdog not running, taskbar left visible");
            return;
        }
        with_guard(|g| {
            if EXITING.load(Ordering::SeqCst) {
                return;
            }
            g.hide();
            if g.is_hidden() {
                tray_host::start();
            }
        });
    }

    fn gate_worker(rx: mpsc::Receiver<GateMsg>, mut gate: HideGate) {
        for msg in rx {
            let target = match msg {
                GateMsg::Want(w) => gate.set_want(w),
                GateMsg::DocksReady(r) => gate.set_docks_ready(r),
                // A hide that found no shell (explorer down) left the taskbar visible:
                // apply it now that the taskbar exists.
                GateMsg::TaskbarCreated => (gate.target() && !is_hidden()).then_some(true),
                GateMsg::TrayHostDied => {
                    // Bounded retry pace; restart only while we still hide the taskbar.
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    with_guard(|g| {
                        if g.is_hidden() && !EXITING.load(Ordering::SeqCst) {
                            tray_host::start();
                        }
                    });
                    None
                }
            };
            match target {
                Some(true) => hide(),
                Some(false) => restore(),
                None => {}
            }
        }
    }

    /// Explorer restarted. Secondary-monitor taskbars can appear a little after
    /// `TaskbarCreated`, so re-apply once more shortly after.
    fn on_taskbar_created() {
        with_guard(|g| g.on_explorer_restarted());
        crate::dock_host::send(crate::dock_host::Msg::ShellRestarted);
        send(GateMsg::TaskbarCreated);
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            with_guard(|g| g.on_explorer_restarted());
        });
    }

    /// Panic-safe: the panicking thread may hold the lock, so never block on it.
    fn emergency_restore() {
        match GUARD.get().map(|m| m.try_lock()) {
            Some(Ok(mut g)) => g.restore(),
            Some(Err(TryLockError::Poisoned(e))) => e.into_inner().restore(),
            // Lock busy or guard not built yet: restore from the marker on disk.
            _ => TaskbarGuard::new(Win32Taskbar, taskbar::marker_path()).recover_on_startup(),
        }
    }

    /// Installed first thing in `run()` (after the single-instance check), before anything
    /// can hide the taskbar. Debug builds abort on panic too (`[profile.dev] panic =
    /// "abort"`), so a panic never unwinds into a half state: this hook runs, then abort.
    pub fn install_safety_nets() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            emergency_restore();
            previous(info);
        }));
        taskbar::win::install_console_ctrl_handler(restore_for_exit);
    }

    /// `true`: a dock window is up; `false`: none left. From the dock host thread.
    pub fn set_docks_ready(ready: bool) {
        send(GateMsg::DocksReady(ready));
    }

    pub fn init(app: &AppHandle, hide_on_start: bool) {
        tray_host::init(app);
        tray_host::on_unexpected_exit(|| send(GateMsg::TrayHostDied));
        let mut guard = TaskbarGuard::new(Win32Taskbar, taskbar::marker_path());
        guard.recover_on_startup();
        let _ = GUARD.set(Mutex::new(guard));

        match taskbar::watchdog::spawn_watchdog() {
            Ok(()) => WATCHDOG_OK.store(true, Ordering::SeqCst),
            Err(e) => eprintln!("lucent: cannot spawn taskbar watchdog: {e}"),
        }

        // Nothing is hidden yet: the gate hides once the first dock page reports in.
        let (tx, rx) = mpsc::channel();
        if GATE.set(tx).is_ok() {
            let gate = HideGate::new(hide_on_start);
            std::thread::Builder::new()
                .name("lucent-taskbar-gate".into())
                .spawn(move || gate_worker(rx, gate))
                .expect("spawn taskbar gate thread");
        }

        taskbar::win::spawn_shell_listener(on_taskbar_created, restore_for_exit);

        // Windows 11 shows its taskbars again on its own (Explorer start, Start menu,
        // monitor changes): put them back out of sight while we hide them.
        let spawned = std::thread::Builder::new().name("lucent-taskbar-enforce".into()).spawn(|| {
            let mut last_log: Option<std::time::Instant> = None;
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if EXITING.load(Ordering::SeqCst) {
                    break;
                }
                let mut acted = false;
                with_guard(|g| acted = g.enforce());
                // At most one log line a minute if Windows keeps fighting.
                if acted && last_log.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(60)) {
                    crate::diag::event("taskbar shown by Windows, hidden again");
                    last_log = Some(std::time::Instant::now());
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("lucent: cannot spawn taskbar enforce thread: {e}");
        }

        app.listen_any("settings://changed", |event| {
            match serde_json::from_str::<Settings>(event.payload()) {
                Ok(s) => send(GateMsg::Want(s.general.hide_windows_taskbar)),
                Err(e) => eprintln!("lucent: bad settings://changed payload: {e}"),
            }
        });
    }
}

/// Old pins of Store apps by exe path (`...\WindowsApps\<package>\x.exe`) cannot be
/// launched and never match their running windows (tracked by AUMID): re-pin them by
/// AUMID once and save. Enumerates the AppsFolder only when such a pin exists.
fn migrate_packaged_pins(s: Settings, path: &std::path::Path) -> Settings {
    let needs = s.dock.pinned.iter().any(|p| matches!(&p.app, settings::AppId::Exe(e) if wintrack::is_windowsapps_path(e)));
    if !needs {
        return s;
    }
    let resolve = |e: &str| {
        if wintrack::is_windowsapps_path(e) { search::apps::aumid_for_packaged_exe(e) } else { None }
    };
    let Some(pinned) = settings::migrate_pins(&s.dock.pinned, resolve) else { return s };
    let mut s = s;
    s.dock.pinned = pinned;
    if let Err(e) = settings::save_to(path, &s) {
        eprintln!("lucent: cannot save migrated pins: {e}");
    }
    s
}

/// For the second-launch signal (`single_instance`), which arrives as a plain `fn()`.
#[cfg(windows)]
static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

#[cfg(windows)]
fn open_settings_from_second_launch() {
    if let Some(app) = APP.get() {
        settings_window::open_async(app);
    }
}

/// At sign-in the Run key fires before Explorer has its taskbar up and while DWM, the GPU
/// driver and the monitors are still settling: wait (bounded) for the shell first.
#[cfg(windows)]
fn wait_for_shell() {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(60) {
        if unsafe { FindWindowW(windows::core::w!("Shell_TrayWnd"), None) }.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    // A little more for the secondary taskbars and the display configuration.
    std::thread::sleep(std::time::Duration::from_secs(3));
    diag::event(format!("shell ready after {:?}", start.elapsed()));
}

/// Leaves at once, after giving the taskbar back. Skips the normal teardown on purpose: the
/// native glass (Windows.UI.Composition, DispatcherQueue on the main thread) and WebView2
/// released while the process unloads is where CoreMessaging fail-fasts (0xE0464645)
/// were seen. Everything persistent (settings, history) is already saved when it changes.
fn exit_now(code: i32) -> ! {
    #[cfg(windows)]
    taskbar_glue::restore_for_exit();
    diag::event(format!("exit ({code})"));
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), code as u32);
    }
    std::process::exit(code)
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let unattended = cli::is_unattended(&args);
    // Single instance, before anything else (safety nets, startup recovery, windows): a
    // second launch by the user asks the running one to open Réglages and leaves; a launch
    // by the system or the watchdog just leaves. The helper modes (`--watchdog`,
    // `--spawn-watchdog`, `--restore-taskbar`) never reach `run()`.
    #[cfg(windows)]
    if !single_instance::acquire() {
        if !unattended {
            single_instance::signal_running_instance();
        }
        return;
    }
    diag::init("app", true);
    #[cfg(windows)]
    diag::install_exception_breadcrumbs();
    #[cfg(windows)]
    taskbar_glue::install_safety_nets();
    #[cfg(windows)]
    if unattended {
        wait_for_shell();
    }

    let app = valorant::assets::register_protocol(search::quicklook::register_protocol(icons::register_protocol(tauri::Builder::default())))
        .setup(|app| {
            let path = settings::settings_path();
            let s = migrate_packaged_pins(settings::load_from(&path), &path);
            let hide_taskbar = s.general.hide_windows_taskbar;
            #[cfg(windows)]
            let dock_settings = s.clone();
            autostart::set_autostart(s.general.launch_at_startup);
            app.listen_any("settings://changed", |event| {
                match serde_json::from_str::<Settings>(event.payload()) {
                    Ok(s) => autostart::set_autostart(s.general.launch_at_startup),
                    Err(e) => eprintln!("lucent: bad settings://changed payload: {e}"),
                }
            });
            let startup_settings = s.clone();
            app.manage(SettingsStore::new(s, path));
            let search_index = search::SearchIndex::new();
            app.manage(search_index.clone());
            search::apps::init();
            search::apps::refresh_if_stale(&search_index);
            search::projects::init();
            search::projects::refresh_if_stale(&search_index, app.state::<SettingsStore>().get());
            search::files::start(&search_index, app.state::<SettingsStore>().get());
            app.manage(search::load_history());
            search::commands::init(app.handle().clone(), &search_index);
            search::emoji::init();
            search::valo::init(app.handle().clone());
            valorant::watch::init_pinned(app.handle());
            // History is memory-only; the listener runs only while the setting is on.
            search::clipboard::init(app.state::<SettingsStore>().get().search.clipboard_history);
            app.listen_any("settings://changed", move |event| {
                match serde_json::from_str::<Settings>(event.payload()) {
                    Ok(s) => search::clipboard::set_enabled(s.search.clipboard_history),
                    Err(e) => eprintln!("lucent: bad settings://changed payload: {e}"),
                }
            });
            // Global hotkeys. Handlers run on the hotkey thread: `toggle` only pushes to the
            // search window's channel, never blocks. Search is ignored while a focused game
            // owns the monitor under the cursor; the others are released while a game is
            // focused (`hotkey::set_game_focused`, driven by the game service).
            let search_app = app.handle().clone();
            hotkey::set_handler(hotkey::HotkeyId::Search, move |_| {
                if !game::service::blocks_hotkey() {
                    search_window::toggle(&search_app);
                }
            });
            switcher::init(app.handle());
            overview::init(app.handle());
            // Game Mode suspend/resume changes the statuses: the Raccourcis tab follows.
            let status_app = app.handle().clone();
            hotkey::set_game_status_listener(move || emit_hotkey_status(&status_app));
            hotkey::start(on_hotkey);
            let (startup_hotkeys, errs) = hotkey::desired(&startup_settings);
            for e in errs {
                eprintln!("lucent: {e}");
            }
            apply_hotkeys(app.handle(), startup_hotkeys.clone());
            // One worker applies changes in order; bursts collapse to the newest map.
            let (hk_tx, hk_rx) = std::sync::mpsc::channel::<hotkey::HotkeyMap>();
            let hk_app = app.handle().clone();
            let spawned = std::thread::Builder::new()
                .name("lucent-hotkey-apply".into())
                .spawn(move || {
                    let mut last = startup_hotkeys;
                    while let Ok(mut m) = hk_rx.recv() {
                        while let Ok(newer) = hk_rx.try_recv() {
                            m = newer;
                        }
                        let all_ok = hotkey::statuses().values().all(|st| st.ok);
                        if all_ok && m == last {
                            continue;
                        }
                        last = m.clone();
                        apply_hotkeys(&hk_app, m);
                    }
                });
            // Hotkey changes then apply only at the next launch; never fatal.
            if let Err(e) = spawned {
                eprintln!("lucent: cannot spawn hotkey apply thread: {e}");
            }
            app.listen_any("settings://changed", move |event| {
                let Ok(s) = serde_json::from_str::<Settings>(event.payload()) else { return };
                let (m, errs) = hotkey::desired(&s);
                for e in errs {
                    eprintln!("lucent: hotkey ignored: {e}");
                }
                let _ = hk_tx.send(m);
            });
            let tracker = std::sync::Arc::new(wintrack::WindowTracker::new());
            app.manage(tracker.clone());
            let handle = app.handle().clone();
            tracker.on_change(move |_| emit_items(&handle));
            let handle = app.handle().clone();
            app.listen_any("settings://changed", move |_| emit_items(&handle));
            app.on_menu_event(|app, event| on_dock_menu_event(app, event.id().as_ref()));
            #[cfg(windows)]
            {
                let own_pid = std::process::id();
                wintrack::win::watch(own_pid, move |raws| tracker.update(&raws, own_pid));
                // A run killed mid-session left High performance on: put the plan back.
                game::power::recover_on_startup();
                game::service::start(app.handle(), app.state::<SettingsStore>().get().game_mode);
            }
            #[cfg(windows)]
            {
                let _ = APP.set(app.handle().clone());
                taskbar::win::on_open_settings(open_settings_from_second_launch);
                taskbar_glue::init(app.handle(), hide_taskbar);
            }
            #[cfg(windows)]
            system::start(app.handle());
            #[cfg(windows)]
            {
                glass::win::start(app.handle(), dock_settings.dock.glass);
                taskbar::win::on_setting_change(glass::win::on_setting_change);
                dock_host::start(app.handle(), dock_settings, taskbar_glue::set_docks_ready);
                taskbar::win::on_display_change(|| {
                    diag::event("display change");
                    dock_host::request_reconcile();
                });
                app.listen_any("settings://changed", |event| {
                    match serde_json::from_str::<Settings>(event.payload()) {
                        Ok(s) => {
                            glass::win::send(glass::win::Msg::Mode(s.dock.glass));
                            dock_host::send(dock_host::Msg::Settings(Box::new(s)));
                        }
                        Err(e) => eprintln!("lucent: bad settings://changed payload: {e}"),
                    }
                });
            }
            #[cfg(not(windows))]
            let _ = hide_taskbar;
            // M6 track B: window snapping (handlers only feed the snap worker) and the
            // global mic mute (the hotkey only pushes to the mic watcher's channel).
            #[cfg(windows)]
            {
                snap::win::start();
                system::mic::win::start(app.handle().clone());
                hotkey::set_handler(hotkey::HotkeyId::MicMute, |_| system::mic::win::toggle());
                system::display_input::win::init(app.handle());
                let ds_app = app.handle().clone();
                hotkey::set_handler(hotkey::HotkeyId::DisplaySwitch, move |_| {
                    let s = ds_app.state::<SettingsStore>().get().display_switch;
                    system::display_input::win::switch(s, None);
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            report_dock_layout,
            get_dock_items,
            dock_click,
            dock_menu,
            power_action,
            dock_reorder,
            thumbs_pick,
            dock_hover,
            thumbs_close,
            thumbs_pointer,
            get_tray_icons,
            tray_click,
            get_system_state,
            set_volume,
            toggle_mute,
            get_media,
            media_control,
            open_notifications,
            open_network,
            open_settings,
            open_external,
            list_monitors,
            quit_app,
            thumbs_ready,
            get_hotkey_status,
            get_hotkey_statuses,
            search_hide,
            search_resize,
            search_query,
            search_open,
            game_answer,
            list_detected_games,
            get_game_state,
            wsl_free,
            valo_test,
            wsl_shutdown,
            get_trash_status,
            empty_trash,
            list_stack_files,
            dock_drop_open,
            dock_spring,
            search::quicklook::quicklook_open,
            search::quicklook::quicklook_close,
            cc_refresh,
            cc_set_wifi,
            cc_set_bluetooth,
            cc_set_dark,
            cc_set_brightness,
            audio_set_output,
            get_audio_devices,
            switcher::switcher_pick,
            switcher::switcher_ready,
            overview::overview_pick,
            overview::overview_close,
            overview::overview_ready,
            mic_toggle,
            display_switch,
            list_ddc_monitors
        ])
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(e) => {
            // Typically WebView2 not ready yet right after sign-in. One more try later,
            // by a fresh process (the watchdog does not exist yet).
            diag::event(format!("cannot build the app: {e}"));
            #[cfg(windows)]
            if args.iter().any(|a| a == cli::AUTOSTART_FLAG) {
                std::thread::sleep(std::time::Duration::from_secs(10));
                if let Err(e) = taskbar::watchdog::spawn_detached(&[cli::RELAUNCH_FLAG]) {
                    diag::event(format!("retry launch failed: {e}"));
                }
            }
            exit_now(3);
        }
    };

    app.run(|_app, event| match event {
        // The last window closing (dock windows are recreated on monitor changes) must not
        // quit the app; an explicit `app.exit(code)` carries a code and exits right here,
        // before tao/wry tear the windows down (see `exit_now`).
        tauri::RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
        tauri::RunEvent::ExitRequested { code: Some(code), .. } => exit_now(code),
        tauri::RunEvent::Exit => exit_now(0),
        _ => {}
    });
}

/// `--restore-taskbar`: restore the taskbar from the marker file and return (no app, no window).
#[cfg(windows)]
pub fn restore_taskbar_and_exit() {
    use taskbar::{TaskbarGuard, win::Win32Taskbar};
    TaskbarGuard::new(Win32Taskbar, taskbar::marker_path()).recover_on_startup();
    // The uninstaller killed every lucent.exe (watchdog included): give the tray icons
    // back to Explorer.
    taskbar::watchdog::broadcast_taskbar_created();
}
