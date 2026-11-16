//! Opens the Windows notification centre / network flyout (protocol URIs).
pub const NOTIFICATIONS_URI: &str = "ms-actioncenter:";
pub const NETWORK_URI: &str = "ms-availablenetworks:";

#[cfg(windows)]
fn shell_open(uri: &'static str) {
    shell_open_str(uri);
}

pub fn shell_open_str(uri: &str) {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::{HSTRING, PCWSTR, w};
        let uri = HSTRING::from(uri);
        // Off the caller's thread: ShellExecute can block for a moment (and may pump COM).
        std::thread::spawn(move || {
            let r = unsafe { ShellExecuteW(None, w!("open"), &uri, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
            // Values <= 32 are errors.
            if (r.0 as isize) <= 32 {
                eprintln!("lucent: cannot open {uri}");
            }
        });
    }
    #[cfg(not(windows))]
    let _ = uri;
}

pub fn open_notifications() {
    #[cfg(windows)]
    shell_open(NOTIFICATIONS_URI);
}

pub fn open_network() {
    #[cfg(windows)]
    shell_open(NETWORK_URI);
}
