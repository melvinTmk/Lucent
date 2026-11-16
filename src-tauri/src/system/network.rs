//! Network kind (Ethernet / Wi-Fi / none) and Wi-Fi SSID: pure selection logic + the
//! watcher (Windows).
use serde::Serialize;

pub const IF_TYPE_ETHERNET: u32 = 6;
pub const IF_TYPE_WIFI: u32 = 71;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NetKind {
    Ethernet,
    Wifi,
    None,
}

/// `system://network` payload. `name` is the Wi-Fi SSID (null when unknown / not Wi-Fi).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NetworkState {
    pub kind: NetKind,
    pub name: Option<String>,
}

/// Preference Ethernet(6) > Wi-Fi(71) > None, over `(IfType, up)` pairs. The caller
/// decides what "up" means (operationally up and routed).
pub fn network_kind(if_types: &[(u32, bool)]) -> NetKind {
    let has = |t: u32| if_types.iter().any(|&(ty, up)| up && ty == t);
    if has(IF_TYPE_ETHERNET) {
        NetKind::Ethernet
    } else if has(IF_TYPE_WIFI) {
        NetKind::Wifi
    } else {
        NetKind::None
    }
}

/// `MIB_NOTIFICATION_TYPE`: parameter change (0), instance added (1) or deleted (2) mean
/// the adapter set / link state may have changed; the initial notification (3) does not.
pub fn ip_change_triggers_refresh(notification_type: i32) -> bool {
    matches!(notification_type, 0..=2)
}

/// WLAN ACM notification codes worth a refresh: connection complete (10) and
/// disconnected (21); this is how an SSID change (roaming, switching network) shows up.
pub fn wlan_code_triggers_refresh(code: u32) -> bool {
    matches!(code, 10 | 21)
}

#[cfg(windows)]
pub mod win {
    use super::{NetKind, NetworkState, ip_change_triggers_refresh, network_kind, wlan_code_triggers_refresh};
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::time::Duration;
    use tauri::AppHandle;
    use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS, HANDLE};
    use windows::Win32::NetworkManagement::IpHelper::{
        CancelMibChangeNotify2, MIB_IPINTERFACE_ROW, MIB_NOTIFICATION_TYPE, NotifyIpInterfaceChange,
        GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
        IF_TYPE_IEEE80211, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::NetworkManagement::WiFi::{
        L2_NOTIFICATION_DATA, WLAN_CONNECTION_ATTRIBUTES, WLAN_NOTIFICATION_SOURCES, WLAN_NOTIFICATION_SOURCE_ACM, WlanRegisterNotification, WLAN_INTERFACE_INFO_LIST, wlan_interface_state_connected,
        wlan_intf_opcode_current_connection, WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory,
        WlanOpenHandle, WlanQueryInterface,
    };
    use windows::Win32::Networking::NetworkListManager::{
        INetworkListManager, INetworkListManagerEvents, INetworkListManagerEvents_Impl, NLM_CONNECTIVITY,
        NetworkListManager,
    };
    use windows::Win32::Networking::WinSock::AF_UNSPEC;
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
        IConnectionPointContainer,
    };
    use windows::core::{GUID, Interface, Result, implement};

    /// Safety net for events nothing signals (e.g. an SSID change on a roaming network).
    const RESCAN: Duration = Duration::from_secs(30);
    /// Network events come in bursts: wait for them to settle before reading.
    const DEBOUNCE: Duration = Duration::from_millis(400);

    #[implement(INetworkListManagerEvents)]
    struct Events(Sender<()>);

    impl INetworkListManagerEvents_Impl for Events_Impl {
        fn ConnectivityChanged(&self, _connectivity: NLM_CONNECTIVITY) -> Result<()> {
            let _ = self.0.send(());
            Ok(())
        }
    }

    /// `(IfType, up)` for every adapter. "Up" means operationally up AND with a gateway,
    /// so virtual switches (WSL, Hyper-V, VPN stubs) without a route don't count.
    #[cfg(test)]
    pub(crate) fn adapters_for_test() -> Vec<(u32, bool)> {
        adapters()
    }

    fn adapters() -> Vec<(u32, bool)> {
        // Without INCLUDE_GATEWAYS, FirstGatewayAddress is always null and every adapter
        // would read as "no network".
        let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size = 16 * 1024u32;
        // u64 backing keeps the buffer 8-aligned for IP_ADAPTER_ADDRESSES_LH.
        let mut buf: Vec<u64> = Vec::new();
        for _ in 0..4 {
            buf.resize((size as usize).div_ceil(8), 0);
            let p = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
            let r = unsafe { GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(p), &mut size) };
            if r == ERROR_BUFFER_OVERFLOW.0 {
                continue;
            }
            if r != ERROR_SUCCESS.0 {
                return Vec::new();
            }
            let mut out = Vec::new();
            let mut cur = p as *const IP_ADAPTER_ADDRESSES_LH;
            while let Some(a) = unsafe { cur.as_ref() } {
                let up = a.OperStatus == IfOperStatusUp && !a.FirstGatewayAddress.is_null();
                out.push((a.IfType, up));
                cur = a.Next;
            }
            return out;
        }
        Vec::new()
    }

    /// SSID of the first connected Wi-Fi interface; None when no adapter / service off.
    fn ssid() -> Option<String> {
        unsafe {
            let mut negotiated = 0u32;
            let mut handle = HANDLE::default();
            if WlanOpenHandle(2, None, &mut negotiated, &mut handle) != 0 {
                return None;
            }
            let name = (|| {
                let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
                if WlanEnumInterfaces(handle, None, &mut list) != 0 || list.is_null() {
                    return None;
                }
                let infos = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), (*list).dwNumberOfItems as usize);
                let guids: Vec<GUID> = infos
                    .iter()
                    .filter(|i| i.isState == wlan_interface_state_connected)
                    .map(|i| i.InterfaceGuid)
                    .collect();
                WlanFreeMemory(list as *const _);
                for guid in guids {
                    let mut len = 0u32;
                    let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
                    let r = WlanQueryInterface(handle, &guid, wlan_intf_opcode_current_connection, None, &mut len, &mut data, None);
                    if r != 0 || data.is_null() {
                        continue;
                    }
                    let attrs = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let s = &attrs.wlanAssociationAttributes.dot11Ssid;
                    let n = (s.uSSIDLength as usize).min(s.ucSSID.len());
                    let name = String::from_utf8_lossy(&s.ucSSID[..n]).into_owned();
                    WlanFreeMemory(data as *const _);
                    if !name.is_empty() {
                        return Some(name);
                    }
                }
                None
            })();
            WlanCloseHandle(handle, None);
            name
        }
    }

    fn current() -> NetworkState {
        let kind = network_kind(&adapters());
        // Wi-Fi interface type constant sanity: keep the pure logic's value in step.
        debug_assert_eq!(IF_TYPE_IEEE80211, super::IF_TYPE_WIFI);
        let name = if kind == NetKind::Wifi { ssid() } else { None };
        NetworkState { kind, name }
    }

    pub fn start(app: AppHandle) {
        let spawned = std::thread::Builder::new().name("lucent-network".into()).spawn(move || {
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            run(&app);
            if com {
                unsafe { CoUninitialize() };
            }
        });
        if let Err(e) = spawned {
            eprintln!("lucent: cannot start network watcher: {e}");
        }
    }

    fn advise(tx: Sender<()>) -> Result<(IConnectionPointContainer, u32)> {
        unsafe {
            let nlm: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)?;
            let container: IConnectionPointContainer = nlm.cast()?;
            let point = container.FindConnectionPoint(&INetworkListManagerEvents::IID)?;
            let sink: INetworkListManagerEvents = Events(tx).into();
            let cookie = point.Advise(&sink)?;
            Ok((container, cookie))
        }
    }

    /// IP Helper: interface / link changes (cable unplugged while Wi-Fi stays online
    /// leaves the aggregate connectivity unchanged). The callback only sends.
    unsafe extern "system" fn on_ip_change(ctx: *const core::ffi::c_void, _row: *const MIB_IPINTERFACE_ROW, kind: MIB_NOTIFICATION_TYPE) {
        if ip_change_triggers_refresh(kind.0)
            && let Some(tx) = unsafe { (ctx as *const Sender<()>).as_ref() }
        {
            let _ = tx.send(());
        }
    }

    /// Owns the registration; the context sender is freed after the cancel (which waits
    /// for in-flight callbacks).
    struct IpWatch {
        handle: HANDLE,
        ctx: *mut Sender<()>,
    }

    impl IpWatch {
        fn new(tx: Sender<()>) -> Option<Self> {
            let ctx = Box::into_raw(Box::new(tx));
            let mut handle = HANDLE::default();
            let r = unsafe { NotifyIpInterfaceChange(AF_UNSPEC, Some(on_ip_change), Some(ctx as *const _), false, &mut handle) };
            if r != ERROR_SUCCESS {
                eprintln!("lucent: cannot watch IP interfaces: {r:?}");
                drop(unsafe { Box::from_raw(ctx) });
                return None;
            }
            Some(Self { handle, ctx })
        }
    }

    impl Drop for IpWatch {
        fn drop(&mut self) {
            unsafe {
                let _ = CancelMibChangeNotify2(self.handle);
                drop(Box::from_raw(self.ctx));
            }
        }
    }

    unsafe extern "system" fn on_wlan(data: *mut L2_NOTIFICATION_DATA, ctx: *mut core::ffi::c_void) {
        let Some(d) = (unsafe { data.as_ref() }) else { return };
        if d.NotificationSource == WLAN_NOTIFICATION_SOURCE_ACM
            && wlan_code_triggers_refresh(d.NotificationCode)
            && let Some(tx) = unsafe { (ctx as *const Sender<()>).as_ref() }
        {
            let _ = tx.send(());
        }
    }

    /// WLAN auto-config notifications (SSID roaming / switching). Silent no-op without a
    /// WLAN service.
    struct WlanWatch {
        handle: HANDLE,
        ctx: *mut Sender<()>,
    }

    impl WlanWatch {
        fn new(tx: Sender<()>) -> Option<Self> {
            unsafe {
                let mut negotiated = 0u32;
                let mut handle = HANDLE::default();
                if WlanOpenHandle(2, None, &mut negotiated, &mut handle) != 0 {
                    return None;
                }
                let ctx = Box::into_raw(Box::new(tx));
                let r = WlanRegisterNotification(handle, WLAN_NOTIFICATION_SOURCE_ACM, true, Some(on_wlan), Some(ctx as *const _), None, None);
                if r != 0 {
                    WlanCloseHandle(handle, None);
                    drop(Box::from_raw(ctx));
                    return None;
                }
                Some(Self { handle, ctx })
            }
        }
    }

    impl Drop for WlanWatch {
        fn drop(&mut self) {
            unsafe {
                // Unregister first, then close (waits for callbacks), then free the context.
                let _ = WlanRegisterNotification(self.handle, WLAN_NOTIFICATION_SOURCES(0), true, None, None, None, None);
                WlanCloseHandle(self.handle, None);
                drop(Box::from_raw(self.ctx));
            }
        }
    }

    fn run(app: &AppHandle) {
        let (tx, rx) = mpsc::channel::<()>();
        let _ip = IpWatch::new(tx.clone());
        let _wlan = WlanWatch::new(tx.clone());
        // Keep the container alive for the whole run; without events the rescan still works.
        let _subscription = advise(tx).map_err(|e| eprintln!("lucent: no network events: {e}")).ok();
        let mut last: Option<NetworkState> = None;
        loop {
            let now = current();
            if last.as_ref() != Some(&now) {
                crate::system::publish_network(app, now.clone());
                last = Some(now);
            }
            match rx.recv_timeout(RESCAN) {
                Ok(()) => {
                    std::thread::sleep(DEBOUNCE);
                    while rx.try_recv().is_ok() {}
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    // The sink was not registered (sender dropped): rescan only.
                    std::thread::sleep(RESCAN);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Live: `cargo test system::network -- --ignored --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_adapters_report_a_routed_link() {
        let a = win::adapters_for_test();
        println!("{a:?} -> {:?}", network_kind(&a));
        assert_ne!(network_kind(&a), NetKind::None);
    }

    #[test]
    fn ethernet_preferred_over_wifi() {
        assert_eq!(network_kind(&[(71, true), (6, true)]), NetKind::Ethernet);
    }
    #[test]
    fn down_adapters_ignored() {
        assert_eq!(network_kind(&[(6, false)]), NetKind::None);
    }
    #[test]
    fn refresh_triggers() {
        assert!(ip_change_triggers_refresh(0) && ip_change_triggers_refresh(1) && ip_change_triggers_refresh(2));
        assert!(!ip_change_triggers_refresh(3));
        assert!(wlan_code_triggers_refresh(10) && wlan_code_triggers_refresh(21));
        assert!(!wlan_code_triggers_refresh(8));
    }
    #[test]
    fn wifi_when_ethernet_down() {
        assert_eq!(network_kind(&[(6, false), (71, true)]), NetKind::Wifi);
    }
}
