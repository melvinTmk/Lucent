//! Win32 Recycle Bin query and empty operations.
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrashStatus {
    pub count: usize,
    pub bytes: u64,
}

#[cfg(windows)]
pub fn query_recycle_bin() -> Result<TrashStatus, String> {
    use windows::Win32::UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO};
    unsafe {
        let mut info = SHQUERYRBINFO {
            cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
            i64Size: 0,
            i64NumItems: 0,
        };
        let hr = SHQueryRecycleBinW(None, &mut info);
        if hr.is_ok() {
            Ok(TrashStatus {
                count: info.i64NumItems.max(0) as usize,
                bytes: info.i64Size.max(0) as u64,
            })
        } else {
            Err(format!("SHQueryRecycleBin failed: {hr:?}"))
        }
    }
}

#[cfg(not(windows))]
pub fn query_recycle_bin() -> Result<TrashStatus, String> {
    Ok(TrashStatus { count: 0, bytes: 0 })
}

#[cfg(windows)]
pub fn empty_recycle_bin() -> Result<(), String> {
    use windows::Win32::UI::Shell::{SHEmptyRecycleBinW, SHERB_NOCONFIRMATION};
    unsafe {
        let hr = SHEmptyRecycleBinW(None, None, SHERB_NOCONFIRMATION);
        if hr.is_ok() {
            Ok(())
        } else {
            Err(format!("SHEmptyRecycleBin failed: {hr:?}"))
        }
    }
}

#[cfg(not(windows))]
pub fn empty_recycle_bin() -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_status_serialization() {
        let st = TrashStatus { count: 4, bytes: 1024 };
        let json = serde_json::to_string(&st).unwrap();
        assert_eq!(json, r#"{"count":4,"bytes":1024}"#);
    }

    #[test]
    fn query_recycle_bin_runs_without_panic() {
        let res = query_recycle_bin();
        assert!(res.is_ok());
    }
}
