//! Win32 icon extraction: shell image factory, AppX asset fallback, Riot .ico special case.
use super::{IconError, IconSource, bgra_premul_to_png, encode_png};
use crate::settings::AppId;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, ReleaseDC,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY,
};
use windows::core::{HSTRING, PCWSTR};

pub struct WinIconSource;

fn err(ctx: &str, e: impl std::fmt::Display) -> IconError {
    IconError(format!("{ctx}: {e}"))
}

/// Shell icon for a parsing name as (png, width). Fails if the shell has no bitmap.
fn shell_icon(parsing_name: &str, size: u32) -> Result<(Vec<u8>, u32), IconError> {
    unsafe {
        // Per-thread COM init; S_FALSE / RPC_E_CHANGED_MODE are fine to ignore.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let wide = HSTRING::from(parsing_name);
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).map_err(|e| err("SHCreateItemFromParsingName", e))?;
        // The shell answers E_PENDING while it loads the icon in the background (first
        // request for an app, cold icon cache): ask again instead of giving up.
        let hbmp = retry_pending(PENDING_TRIES, PENDING_DELAY, || {
            factory.GetImage(SIZE { cx: size as i32, cy: size as i32 }, SIIGBF_ICONONLY)
        })
        .map_err(|e| err("GetImage", e))?;
        let r = hbitmap_to_png(hbmp);
        let _ = DeleteObject(hbmp.into());
        r
    }
}

/// `E_PENDING` (0x8000000A): "the data necessary to complete this operation is not yet available".
const E_PENDING: windows::core::HRESULT = windows::core::HRESULT(0x8000000Au32 as i32);
const PENDING_TRIES: u32 = 20;
const PENDING_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

/// Calls `f` until it returns something other than `E_PENDING`, at most `tries` times,
/// sleeping `delay` between attempts. Returns the last result.
pub(super) fn retry_pending<T>(
    tries: u32,
    delay: std::time::Duration,
    mut f: impl FnMut() -> windows::core::Result<T>,
) -> windows::core::Result<T> {
    let mut last = f();
    for _ in 1..tries.max(1) {
        match &last {
            Err(e) if e.code() == E_PENDING => {
                std::thread::sleep(delay);
                last = f();
            }
            _ => break,
        }
    }
    last
}

unsafe fn hbitmap_to_png(hbmp: HBITMAP) -> Result<(Vec<u8>, u32), IconError> {
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(hbmp.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
            return Err(IconError("GetObject failed".into()));
        }
        let (w, h) = (bm.bmWidth as u32, bm.bmHeight.unsigned_abs());
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(dc, hbmp, 0, h, Some(buf.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        if lines == 0 {
            return Err(IconError("GetDIBits failed".into()));
        }
        Ok((bgra_premul_to_png(&buf, w, h)?, w))
    }
}

/// Largest frame of an .ico, re-encoded as PNG (the image crate keeps the best entry).
fn ico_to_png(path: &Path) -> Result<Vec<u8>, IconError> {
    let bytes = std::fs::read(path).map_err(|e| err("read ico", e))?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Ico)
        .map_err(|e| err("decode ico", e))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    encode_png(img.into_raw(), w, h)
}

/// Spec 4.1: Riot exes carry tiny icons; the launcher metadata has a 256 px .ico.
/// `...\Riot Games\VALORANT\live\VALORANT.exe` -> `%PROGRAMDATA%\Riot Games\Metadata\valorant.live\valorant.live.ico`
fn riot_ico(exe: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = exe.split(['\\', '/']).collect();
    let i = parts.iter().position(|p| p.eq_ignore_ascii_case("Riot Games"))?;
    let product = format!("{}.{}", parts.get(i + 1)?, parts.get(i + 2)?).to_lowercase();
    if parts.len() < i + 4 {
        return None; // exe sits directly in the product folder: no variant segment
    }
    let base = std::env::var_os("PROGRAMDATA")?;
    let ico = PathBuf::from(base)
        .join("Riot Games")
        .join("Metadata")
        .join(&product)
        .join(format!("{product}.ico"));
    ico.exists().then_some(ico)
}

/// Rank of an AppX logo file name (lower is better); None if not a candidate.
pub(super) fn asset_rank(file_name: &str) -> Option<u32> {
    let n = file_name.to_lowercase();
    if !n.ends_with(".png") || n.contains("contrast-") {
        return None;
    }
    let unplated = n.contains("_altform-unplated") as u32 ^ 1; // 0 when unplated
    let base = if n.starts_with("square44x44logo.targetsize-256") {
        0
    } else if n.starts_with("square150x150logo.scale-400") {
        2
    } else if n.starts_with("square44x44logo.targetsize-96") {
        4
    } else if n.starts_with("square150x150logo.scale-200") {
        6
    } else {
        return None;
    };
    Some(base + unplated)
}

fn best_asset(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(u32, PathBuf)> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(r) = p.file_name().and_then(|n| n.to_str()).and_then(asset_rank) {
                if best.as_ref().map_or(true, |(b, _)| r < *b) {
                    best = Some((r, p));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Install folder of the package behind an AUMID (`<family>!<app>`), via PackageManager.
fn package_install_dir(aumid: &str) -> Result<PathBuf, IconError> {
    use windows::Management::Deployment::PackageManager;
    let family = aumid.split('!').next().unwrap_or(aumid);
    let pm = PackageManager::new().map_err(|e| err("PackageManager", e))?;
    let pkgs = pm
        .FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(family))
        .map_err(|e| err("FindPackages", e))?;
    for p in pkgs {
        if let Ok(path) = p.InstalledPath() {
            return Ok(PathBuf::from(path.to_string()));
        }
    }
    Err(IconError(format!("no package for {family}")))
}

fn appx_asset_png(aumid: &str) -> Result<Vec<u8>, IconError> {
    let dir = package_install_dir(aumid)?;
    let asset = best_asset(&dir).ok_or_else(|| IconError("no logo asset in package".into()))?;
    std::fs::read(&asset).map_err(|e| err("read asset", e))
}

impl IconSource for WinIconSource {
    fn extract(&self, app: &AppId, size: u32) -> Result<Vec<u8>, IconError> {
        match app {
            AppId::Exe(path) => {
                if let Some(ico) = riot_ico(path) {
                    if let Ok(png) = ico_to_png(&ico) {
                        return Ok(png);
                    }
                }
                shell_icon(path, size).map(|(png, _)| png)
            }
            AppId::Aumid(aumid) => {
                let shell = shell_icon(&format!("shell:AppsFolder\\{aumid}"), size);
                if let Ok((png, w)) = &shell {
                    if *w >= 128 {
                        return Ok(png.clone());
                    }
                }
                match appx_asset_png(aumid) {
                    Ok(png) => Ok(png),
                    Err(e) => shell
                        .map(|(png, _)| png)
                        .map_err(|s| IconError(format!("{e}; shell: {}", s.0))),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_rank_prefers_unplated_256_and_skips_contrast() {
        let plated = asset_rank("Square44x44Logo.targetsize-256.png").unwrap();
        let unplated = asset_rank("Square44x44Logo.targetsize-256_altform-unplated.png").unwrap();
        assert!(unplated < plated);
        assert!(asset_rank("Square44x44Logo.targetsize-256_altform-unplated_contrast-white.png").is_none());
        assert!(asset_rank("Square150x150Logo.scale-400.png").unwrap() > unplated);
        assert!(asset_rank("StoreLogo.png").is_none());
    }

    #[test]
    fn retry_pending_retries_only_e_pending() {
        use windows::Win32::Foundation::E_FAIL;
        let d = std::time::Duration::ZERO;
        let mut n = 0;
        let r = retry_pending(5, d, || {
            n += 1;
            if n < 3 { Err(E_PENDING.into()) } else { Ok(n) }
        });
        assert_eq!(r.unwrap(), 3);
        let mut n = 0;
        let r: windows::core::Result<()> = retry_pending(5, d, || {
            n += 1;
            Err(E_FAIL.into())
        });
        assert_eq!((r.unwrap_err().code(), n), (E_FAIL, 1));
        let mut n = 0;
        let r: windows::core::Result<()> = retry_pending(4, d, || {
            n += 1;
            Err(E_PENDING.into())
        });
        assert_eq!((r.unwrap_err().code(), n), (E_PENDING, 4));
    }

    #[test]
    fn riot_path_without_metadata_yields_none() {
        assert!(riot_ico(r"c:\games\foo.exe").is_none());
    }

    /// Live smoke test against the real shell: `cargo test icons::win -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_extraction_smoke() {
        for app in [
            AppId::Exe(r"c:\windows\explorer.exe".into()),
            AppId::Aumid("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()),
        ] {
            let png = WinIconSource.extract(&app, 256).unwrap();
            let img = image::load_from_memory(&png).unwrap();
            println!("{app:?}: {}x{}", img.width(), img.height());
            assert!(img.width() >= 128);
        }
    }
}

