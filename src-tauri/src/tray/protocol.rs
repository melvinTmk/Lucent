//! Parse `Shell_NotifyIcon` traffic received as `WM_COPYDATA` on the Shell_TrayWnd.
//!
//! Wire layout (ManagedShell `SHELLTRAYDATA` / `NOTIFYICONDATA`): all handles are 32-bit, even
//! when the sender is a 64-bit process. Offsets below are relative to the start of the
//! `NOTIFYICONDATAW32` payload, which follows `dwSignature` and `dwMessage` (8 bytes).

pub const SHELLTRAYDATA_SIGNATURE: u32 = 0x3475_3423;
pub const NIF_MESSAGE: u32 = 0x01;
pub const NIF_ICON: u32 = 0x02;
pub const NIF_TIP: u32 = 0x04;
pub const NIF_STATE: u32 = 0x08;
pub const NIF_GUID: u32 = 0x20;
pub const NIS_HIDDEN: u32 = 0x01;

/// Size of `dwSignature` + `dwMessage`.
const HEADER_LEN: usize = 8;
// NOTIFYICONDATAW32 field offsets (relative to the payload start).
const OFF_CBSIZE: usize = 0;
const OFF_HWND: usize = 4;
const OFF_UID: usize = 8;
const OFF_FLAGS: usize = 12;
const OFF_CALLBACK: usize = 16;
const OFF_HICON: usize = 20;
const OFF_TIP: usize = 24; // WCHAR[128] = 256 bytes
const TIP_BYTES: usize = 256;
const OFF_STATE: usize = 280;
const OFF_STATE_MASK: usize = 284;
#[allow(dead_code)]
const OFF_INFO: usize = 288; // WCHAR[256] = 512 bytes
const OFF_VERSION: usize = 800; // union { uTimeout, uVersion }
#[allow(dead_code)]
const OFF_INFO_TITLE: usize = 804; // WCHAR[64] = 128 bytes
#[allow(dead_code)]
const OFF_INFO_FLAGS: usize = 932;
const OFF_GUID: usize = 936; // 16 bytes
#[allow(dead_code)]
const OFF_BALLOON_ICON: usize = 952;
/// Full payload size (cbSize of the modern struct).
#[allow(dead_code)]
const NID_LEN: usize = 956;
/// Minimum payload: fields up to and including hIcon.
const MIN_PAYLOAD: usize = OFF_HICON + 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NimMessage {
    Add = 0,
    Modify = 1,
    Delete = 2,
    SetFocus = 3,
    SetVersion = 4,
}

impl NimMessage {
    fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::Add,
            1 => Self::Modify,
            2 => Self::Delete,
            3 => Self::SetFocus,
            4 => Self::SetVersion,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotifyIconData {
    pub hwnd: u32,
    pub uid: u32,
    pub flags: u32,
    pub callback_message: u32,
    pub hicon: u32,
    pub tip: String,
    pub state: u32,
    pub state_mask: u32,
    pub version: u32,
    pub guid: Option<[u8; 16]>,
}

fn u32_at(p: &[u8], off: usize) -> u32 {
    p.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .unwrap_or(0)
}

pub fn parse_copydata(dw_data: usize, bytes: &[u8]) -> Option<(NimMessage, NotifyIconData)> {
    if dw_data != 1 || bytes.len() < HEADER_LEN + MIN_PAYLOAD {
        return None;
    }
    if u32_at(bytes, 0) != SHELLTRAYDATA_SIGNATURE {
        return None;
    }
    let msg = NimMessage::from_u32(u32_at(bytes, 4))?;
    let mut p = &bytes[HEADER_LEN..];
    // Legacy structs declare a shorter cbSize: fields beyond it are absent.
    let cb = u32_at(p, OFF_CBSIZE) as usize;
    if cb >= MIN_PAYLOAD && cb < p.len() {
        p = &p[..cb];
    }
    let flags = u32_at(p, OFF_FLAGS);
    let tip = match p.get(OFF_TIP..) {
        Some(rest) => {
            let rest = &rest[..rest.len().min(TIP_BYTES)];
            let units: Vec<u16> = rest
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&u| u != 0)
                .collect();
            String::from_utf16_lossy(&units)
        }
        None => String::new(),
    };
    let guid = match p.get(OFF_GUID..OFF_GUID + 16) {
        Some(g) if flags & NIF_GUID != 0 => {
            let mut a = [0u8; 16];
            a.copy_from_slice(g);
            Some(a)
        }
        _ => None,
    };
    Some((
        msg,
        NotifyIconData {
            hwnd: u32_at(p, OFF_HWND),
            uid: u32_at(p, OFF_UID),
            flags,
            callback_message: u32_at(p, OFF_CALLBACK),
            hicon: u32_at(p, OFF_HICON),
            tip,
            state: u32_at(p, OFF_STATE),
            state_mask: u32_at(p, OFF_STATE_MASK),
            version: u32_at(p, OFF_VERSION),
            guid,
        },
    ))
}

#[cfg(test)]
pub(crate) mod fixture {
    //! Independent serializer: writes fields by literal offset (does not use the parser consts).
    use super::*;

    pub fn build(msg: u32, d: &NotifyIconData) -> Vec<u8> {
        let mut b = vec![0u8; 8 + 956];
        let put = |b: &mut Vec<u8>, off: usize, v: u32| b[off..off + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 0, 0x3475_3423);
        put(&mut b, 4, msg);
        put(&mut b, 8, 956);
        put(&mut b, 8 + 4, d.hwnd);
        put(&mut b, 8 + 8, d.uid);
        put(&mut b, 8 + 12, d.flags);
        put(&mut b, 8 + 16, d.callback_message);
        put(&mut b, 8 + 20, d.hicon);
        for (i, u) in d.tip.encode_utf16().take(127).enumerate() {
            let o = 8 + 24 + i * 2;
            b[o..o + 2].copy_from_slice(&u.to_le_bytes());
        }
        put(&mut b, 8 + 280, d.state);
        put(&mut b, 8 + 284, d.state_mask);
        put(&mut b, 8 + 800, d.version);
        if let Some(g) = d.guid {
            b[8 + 936..8 + 952].copy_from_slice(&g);
        }
        b
    }

    pub fn discord_add() -> Vec<u8> {
        build(
            0,
            &NotifyIconData {
                hwnd: 0x0001_0A2C,
                uid: 7,
                flags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                callback_message: 0x8001,
                hicon: 0x00AB_CDEF,
                tip: "Discord".into(),
                version: 4,
                ..Default::default()
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_wrong_dwdata_or_signature() {
        let b = fixture::discord_add();
        assert!(parse_copydata(2, &b).is_none());
        let mut bad = b.clone();
        bad[0] ^= 0xFF;
        assert!(parse_copydata(1, &bad).is_none());
    }

    #[test]
    fn parse_nim_add_fixture() {
        let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/nim_add_v4.bin"))
            .unwrap();
        assert_eq!(b, fixture::discord_add(), "committed fixture out of sync with helper");
        let (m, d) = parse_copydata(1, &b).unwrap();
        assert_eq!(m, NimMessage::Add);
        assert_eq!((d.hwnd, d.uid, d.callback_message, d.hicon), (0x10A2C, 7, 0x8001, 0xABCDEF));
        assert_eq!(d.tip, "Discord");
        assert_eq!(d.version, 4);
        assert_eq!(d.guid, None);
    }

    /// Regenerate: `cargo test tray::protocol::tests::regen_fixture -- --ignored`.
    #[test]
    #[ignore]
    fn regen_fixture() {
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/nim_add_v4.bin"), fixture::discord_add()).unwrap();
    }

    #[test]
    fn parse_truncated_buffer_returns_none() {
        let b = fixture::discord_add();
        assert!(parse_copydata(1, &b[..8 + 23]).is_none());
        assert!(parse_copydata(1, &b[..10]).is_none());
        assert!(parse_copydata(1, &[]).is_none());
    }

    #[test]
    fn parse_legacy_short_cbsize_tolerated() {
        let mut b = fixture::discord_add();
        b[8..12].copy_from_slice(&88u32.to_le_bytes()); // cbSize up to dwStateMask+... (legacy)
        b.truncate(8 + 88);
        let (_, d) = parse_copydata(1, &b).unwrap();
        assert_eq!(d.tip, "Discord");
        assert_eq!(d.version, 0);
        assert_eq!(d.guid, None);
    }

    #[test]
    fn parse_guid_when_flagged() {
        let d = NotifyIconData { uid: 1, flags: NIF_GUID, guid: Some([9; 16]), ..Default::default() };
        let b = fixture::build(1, &d);
        let (m, p) = parse_copydata(1, &b).unwrap();
        assert_eq!(m, NimMessage::Modify);
        assert_eq!(p.guid, Some([9; 16]));
    }
}
