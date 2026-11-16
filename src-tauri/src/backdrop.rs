//! Native glass under a webview (dock, search window): one rounded, anti-aliased sprite per island, filled
//! with the DWM host backdrop (live blur of what is behind the window). Built on
//! `Windows.UI.Composition` through a `DesktopWindowTarget` rendered below the webview.
//!
//! Why not the window-level effects: both the DWM system backdrop (Tauri `Effect::Acrylic`)
//! and the blur-behind accent ignore the window region, so they paint the whole window
//! rectangle; the system backdrop also turns flat grey on a window that never takes focus.
//! See `docs/decisions.md`.
//!
//! Composition objects live on the main thread (the one owning our windows and
//! pumping their messages). Every function here must be called on that thread.
#![cfg(windows)]
use crate::monitors::Rect;
use std::cell::RefCell;
use std::collections::HashMap;
use windows::System::DispatcherQueueController;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    CompositionBackdropBrush, CompositionColorBrush, CompositionRoundedRectangleGeometry, Compositor, ContainerVisual,
    SpriteVisual,
};
use windows::UI::Color;
use windows::Win32::Foundation::HWND;
use windows::core::BOOL;
use windows::Win32::Graphics::Dwm::{DWMWA_USE_HOSTBACKDROPBRUSH, DwmSetWindowAttribute};
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT, DispatcherQueueOptions,
};
use windows::core::{Interface, Result};
use windows_numerics::{Vector2, Vector3};

struct Shared {
    _queue: DispatcherQueueController,
    compositor: Compositor,
    backdrop: CompositionBackdropBrush,
}

struct Island {
    visual: SpriteVisual,
    geometry: CompositionRoundedRectangleGeometry,
}

struct DockGlass {
    _target: DesktopWindowTarget,
    root: ContainerVisual,
    islands: Vec<Island>,
    /// `Some` while the glass is static (Game Mode): islands use this flat brush instead
    /// of the live blur.
    static_brush: Option<CompositionColorBrush>,
    /// Tone of the static fill (`set_tone`).
    dark: bool,
}

/// Static fill: the dock tone, ~85 % opaque (no blur, so it must read as glass on its own).
fn static_color(dark: bool) -> Color {
    if dark { Color { A: 217, R: 30, G: 30, B: 36 } } else { Color { A: 217, R: 242, G: 242, B: 246 } }
}

fn set_host_backdrop(hwnd: isize, on: bool) -> Result<()> {
    let v = BOOL::from(on);
    unsafe {
        DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_USE_HOSTBACKDROPBRUSH,
            &v as *const _ as _,
            std::mem::size_of::<BOOL>() as u32,
        )
    }
}

thread_local! {
    static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) };
    static DOCKS: RefCell<HashMap<isize, DockGlass>> = RefCell::new(HashMap::new());
}

fn with_shared<T>(f: impl FnOnce(&Shared) -> Result<T>) -> Result<T> {
    SHARED.with(|s| {
        let mut s = s.borrow_mut();
        if s.is_none() {
            // The main thread already pumps Win32 messages (tao): a queue bound to it is
            // all the compositor needs.
            let queue = unsafe {
                CreateDispatcherQueueController(DispatcherQueueOptions {
                    dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                    threadType: DQTYPE_THREAD_CURRENT,
                    apartmentType: DQTAT_COM_NONE,
                })?
            };
            let compositor = Compositor::new()?;
            let backdrop = compositor.CreateHostBackdropBrush()?;
            *s = Some(Shared { _queue: queue, compositor, backdrop });
        }
        f(s.as_ref().expect("initialised above"))
    })
}

/// Puts an (empty) glass layer under `hwnd`'s content. Errors mean no native glass: the
/// caller falls back to the accent blur.
pub fn attach(hwnd: isize) -> Result<()> {
    let h = HWND(hwnd as *mut _);
    let on = BOOL::from(true);
    unsafe {
        DwmSetWindowAttribute(h, DWMWA_USE_HOSTBACKDROPBRUSH, &on as *const _ as _, std::mem::size_of::<BOOL>() as u32)?;
    }
    let glass = with_shared(|s| {
        let interop: ICompositorDesktopInterop = s.compositor.cast()?;
        // Not topmost: rendered below the window's own content (the webview).
        let target = unsafe { interop.CreateDesktopWindowTarget(h, false)? };
        let root = s.compositor.CreateContainerVisual()?;
        root.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
        target.SetRoot(&root)?;
        Ok(DockGlass { _target: target, root, islands: Vec::new(), static_brush: None, dark: false })
    })?;
    DOCKS.with(|d| d.borrow_mut().insert(hwnd, glass));
    Ok(())
}

/// Moves the whole glass of `hwnd` to `to` (px, relative to its rest place). `ms == 0`:
/// jump. Otherwise animate from wherever it is now, on the compositor thread, with the
/// CSS slide curve (`--slide-ease`, cubic-bezier(0.2, 0.8, 0.2, 1)) so glass and icons
/// move together without any per-frame round trip.
pub fn slide(hwnd: isize, to: (f32, f32), ms: u32) -> Result<()> {
    DOCKS.with(|d| {
        let docks = d.borrow();
        let Some(glass) = docks.get(&hwnd) else { return Ok(()) };
        let target = Vector3 { X: to.0, Y: to.1, Z: 0.0 };
        let prop = windows::core::HSTRING::from("Offset");
        glass.root.StopAnimation(&prop)?;
        if ms == 0 {
            return glass.root.SetOffset(target);
        }
        with_shared(|s| {
            let ease = s.compositor.CreateCubicBezierEasingFunction(Vector2 { X: 0.2, Y: 0.8 }, Vector2 { X: 0.2, Y: 1.0 })?;
            let anim = s.compositor.CreateVector3KeyFrameAnimation()?;
            // No key frame at 0: the animation starts from the current offset, so a
            // reversal mid-slide does not jump.
            anim.InsertKeyFrameWithEasingFunction(1.0, target, &ease)?;
            anim.SetDuration(windows::Foundation::TimeSpan { Duration: ms as i64 * 10_000 })?;
            glass.root.SetOffset(target)?;
            glass.root.StartAnimation(&prop, &anim)
        })
    })
}

/// `attach`, falling back to the accent blur (0xAABBGGRR `tint`) when native glass is
/// unavailable. Main thread only, like everything here.
pub fn attach_or_accent(hwnd: isize, tint: u32) {
    if let Err(e) = attach(hwnd) {
        eprintln!("lucent: native glass unavailable ({e}), using the accent blur");
        if !crate::region::set_acrylic_accent(HWND(hwnd as *mut _), tint) {
            eprintln!("lucent: SetWindowCompositionAttribute failed, window has no blur");
        }
    }
}

pub fn detach(hwnd: isize) {
    DOCKS.with(|d| d.borrow_mut().remove(&hwnd));
}

/// Reshapes the glass of `hwnd` to `islands` (client coordinates, physical px), each with
/// corners of `radius` px. Reuses the existing sprites: this runs every frame while icons
/// magnify or the dock slides.
pub fn set_islands(hwnd: isize, islands: &[Rect], radius: f32) -> Result<()> {
    DOCKS.with(|d| {
        let mut docks = d.borrow_mut();
        let Some(glass) = docks.get_mut(&hwnd) else { return Ok(()) };
        with_shared(|s| {
            while glass.islands.len() < islands.len() {
                let visual = s.compositor.CreateSpriteVisual()?;
                match &glass.static_brush {
                    Some(b) => visual.SetBrush(b)?,
                    None => visual.SetBrush(&s.backdrop)?,
                }
                let geometry = s.compositor.CreateRoundedRectangleGeometry()?;
                visual.SetClip(&s.compositor.CreateGeometricClipWithGeometry(&geometry)?)?;
                glass.root.Children()?.InsertAtTop(&visual)?;
                glass.islands.push(Island { visual, geometry });
            }
            while glass.islands.len() > islands.len() {
                if let Some(old) = glass.islands.pop() {
                    glass.root.Children()?.Remove(&old.visual)?;
                }
            }
            for (isl, r) in glass.islands.iter().zip(islands) {
                let size = Vector2 { X: r.w.max(0) as f32, Y: r.h.max(0) as f32 };
                let rad = radius.min(size.X / 2.0).min(size.Y / 2.0).max(0.0);
                isl.visual.SetOffset(Vector3 { X: r.x as f32, Y: r.y as f32, Z: 0.0 })?;
                isl.visual.SetSize(size)?;
                isl.geometry.SetSize(size)?;
                isl.geometry.SetCornerRadius(Vector2 { X: rad, Y: rad })?;
            }
            Ok(())
        })
    })
}

/// Static glass (Game Mode, on monitors other than the game's): the islands get a flat,
/// tone-matched `CompositionColorBrush` and the window stops asking DWM for the host
/// backdrop, so no live blur is computed behind it while a game runs. `false` puts the
/// live glass back.
pub fn set_static(hwnd: isize, on: bool) -> Result<()> {
    DOCKS.with(|d| {
        let mut docks = d.borrow_mut();
        let Some(glass) = docks.get_mut(&hwnd) else { return Ok(()) };
        if on == glass.static_brush.is_some() {
            return Ok(());
        }
        if on {
            with_shared(|s| {
                let brush = s.compositor.CreateColorBrushWithColor(static_color(glass.dark))?;
                for isl in &glass.islands {
                    isl.visual.SetBrush(&brush)?;
                }
                glass.static_brush = Some(brush);
                Ok(())
            })?;
            set_host_backdrop(hwnd, false)
        } else {
            // Back to the live blur even if DWM refuses (the brush then renders empty
            // glass, which the CSS fill still tints): never stay stuck on static.
            if let Err(e) = set_host_backdrop(hwnd, true) {
                eprintln!("lucent: cannot re-enable the host backdrop: {e}");
            }
            with_shared(|s| {
                for isl in &glass.islands {
                    isl.visual.SetBrush(&s.backdrop)?;
                }
                glass.static_brush = None;
                Ok(())
            })
        }
    })
}

/// Tone of the static fill (light/dark, as sent to the page with `dock://tone`).
pub fn set_tone(hwnd: isize, dark: bool) -> Result<()> {
    DOCKS.with(|d| {
        let mut docks = d.borrow_mut();
        let Some(glass) = docks.get_mut(&hwnd) else { return Ok(()) };
        glass.dark = dark;
        match &glass.static_brush {
            Some(b) => b.SetColor(static_color(dark)),
            None => Ok(()),
        }
    })
}
