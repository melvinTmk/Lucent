//! WSL detection. NEVER starts the VM: only the registry and `wsl.exe -l --running -q` are
//! used to learn state; `\\wsl.localhost\...` and `wsl.exe -d` are touched only for a distro
//! that is already running.
use std::path::PathBuf;

/// Decodes `wsl.exe` list output (UTF-16LE, usually with a BOM, CRLF lines) into names.
pub fn parse_list_output(bytes: &[u8]) -> Vec<String> {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
        .trim_start_matches('\u{feff}')
        .lines()
        .map(|l| l.trim().trim_matches('\0').to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// What the project logic needs to know about WSL (a trait so it is testable).
pub trait Wsl {
    fn default_distro(&self) -> Option<String>;
    fn is_running(&self, distro: &str) -> bool;
    /// Only called for a running distro.
    fn home_user(&self, distro: &str) -> Option<String>;
}

/// `\\wsl.localhost\<distro>` + a Linux path.
pub fn unc_path(distro: &str, linux_path: &str) -> PathBuf {
    let tail = linux_path.replace('/', "\\");
    PathBuf::from(format!(r"\\wsl.localhost\{distro}{tail}"))
}

pub struct RealWsl;

impl Wsl for RealWsl {
    fn default_distro(&self) -> Option<String> {
        default_distro()
    }
    fn is_running(&self, distro: &str) -> bool {
        is_running(distro)
    }
    fn home_user(&self, distro: &str) -> Option<String> {
        home_user(distro)
    }
}

pub fn default_distro() -> Option<String> {
    #[cfg(windows)]
    return win::default_distro();
    #[cfg(not(windows))]
    None
}

pub fn is_running(distro: &str) -> bool {
    running_distros().iter().any(|d| d.eq_ignore_ascii_case(distro))
}

/// Distros currently running (`wsl.exe -l --running -q`, which never starts the VM).
/// Empty when none runs or `wsl.exe` is missing/fails.
pub fn running_distros() -> Vec<String> {
    #[cfg(windows)]
    {
        win::run_hidden("wsl.exe", &["-l", "--running", "-q"]).map(|out| parse_list_output(&out)).unwrap_or_default()
    }
    #[cfg(not(windows))]
    Vec::new()
}

/// The single directory under `\\wsl.localhost\<d>\home\`, else `wsl.exe -d <d> whoami`.
/// Caller guarantees the distro is running.
pub fn home_user(distro: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let dirs: Vec<String> = std::fs::read_dir(unc_path(distro, "/home"))
            .ok()?
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        if dirs.len() == 1 {
            return dirs.into_iter().next();
        }
        let out = win::run_hidden("wsl.exe", &["-d", distro, "whoami"])?;
        let s = String::from_utf8_lossy(&out).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
    #[cfg(not(windows))]
    {
        let _ = distro;
        None
    }
}

#[cfg(windows)]
pub mod win {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::{PCWSTR, w};

    pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Runs a program with no console window; stdout on success.
    pub fn run_hidden(program: &str, args: &[&str]) -> Option<Vec<u8>> {
        let out = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        out.status.success().then_some(out.stdout)
    }

    fn reg_string(sub: &str, value: PCWSTR) -> Option<String> {
        let sub: Vec<u16> = sub.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let mut len = 0u32;
            if RegGetValueW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), value, RRF_RT_REG_SZ, None, None, Some(&mut len)).0
                != 0
                || len == 0
            {
                return None;
            }
            let mut buf = vec![0u16; (len as usize).div_ceil(2)];
            let r = RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(sub.as_ptr()),
                value,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut _),
                Some(&mut len),
            );
            if r.0 != 0 {
                return None;
            }
            let n = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..n]))
        }
    }

    pub fn default_distro() -> Option<String> {
        const LXSS: &str = r"Software\Microsoft\Windows\CurrentVersion\Lxss";
        let guid = reg_string(LXSS, w!("DefaultDistribution"))?;
        reg_string(&format!(r"{LXSS}\{guid}"), w!("DistributionName"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le(s: &str, bom: bool) -> Vec<u8> {
        let mut v = Vec::new();
        if bom {
            v.extend([0xff, 0xfe]);
        }
        for u in s.encode_utf16() {
            v.extend(u.to_le_bytes());
        }
        v
    }

    #[test]
    fn running_check_parses_utf16() {
        assert_eq!(parse_list_output(&utf16le("Ubuntu\r\n", true)), ["Ubuntu"]);
        assert_eq!(parse_list_output(&utf16le("Ubuntu\r\nDebian\r\n", false)), ["Ubuntu", "Debian"]);
        assert!(parse_list_output(&[]).is_empty());
    }

    #[test]
    fn unc_path_uses_backslashes() {
        assert_eq!(unc_path("Ubuntu", "/home/u/x").to_string_lossy(), r"\\wsl.localhost\Ubuntu\home\u\x");
    }
}
