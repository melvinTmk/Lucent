# Lucent: design decisions

## Dock glass: native composition glass (decided 2026-11-09, after the first real test)

The first test on the real desktop showed a flat grey rectangle around the islands:
Tauri `Effect::Acrylic` (DWM system backdrop) ignores the window region and renders its
inactive fallback colour, since the dock never takes focus. Fallback B (blur-behind
accent) gave a live blur but still ignored the region, and so did
`DwmEnableBlurBehindWindow` with a blur region. Chosen with the user: **each island is a
`Windows.UI.Composition` sprite filled with the host backdrop brush, clipped to an
anti-aliased rounded rectangle**, rendered below the webview through a
`DesktopWindowTarget` (`src-tauri/src/backdrop.rs`). The GDI region is grown 2 px past
the islands (plus the magnified-icons area) so it only cuts transparent pixels. The
accent stays as a fallback if composition is unavailable.

Proportions follow the user's reference (slim islands, airy icons): icon 36 px, padding
11/14 px, gap 12 px, radius 18 px, floating 6 px above the edge. Magnified icons rise out
of the glass (macOS) instead of growing the island.

The sections below are the original (M1) analysis, kept for history.

## Dock glass shape: rounded window region (Task 6), pending visual check

The dock window is one transparent, undecorated webview per monitor with Tauri's native
Acrylic effect underneath. Acrylic applies to the whole window, so the window must be cut
to the shape of the islands.

- **Primary (implemented): rounded union region.** On every `report_dock_layout` the dock
  page sends its island rects (physical px); Rust builds the union of rounded rectangles
  (`CreateRoundRectRgn`, radius 26 logical px × monitor scale) and applies it with
  `SetWindowRgn` (`src-tauri/src/region.rs`). Acrylic and hit-testing then only exist inside
  the islands; clicks between islands go to the windows below.
  Known risk: GDI regions are not anti-aliased, so the corners may look jagged, mostly at
  100 % scaling.
- **Fallback A: one window per island** with
  `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND)`. DWM rounds the corners
  with anti-aliasing (Windows 11 only; its radius is fixed by the system, about 8 px, not
  26 px), at the cost of N windows per monitor to keep in sync.
- **Fallback B: accent through `SetWindowCompositionAttribute(ACCENT_ENABLE_ACRYLICBLURBEHIND)`**
  instead of Tauri `effects`, to use if the Tauri/DWM backdrop is not clipped by the
  window region at all (then re-check the region with this accent).

Status: **undecided.** The user picks after the visual check in `docs/verification-m1.md`,
"Task 6". Record the choice and the date here.

Related points for the check:

- The slide animation runs in CSS inside the window (`dock://reveal`), while the Acrylic
  backdrop is tied to the window and its region. Rust moves the window on-screen at
  reveal time and parks it past the edge once the slide-out is over, so during the slide
  the glass shape stays still while the content moves. If that looks wrong, the fix is to
  slide the window itself from Rust (`SetWindowPos` steps on the 16 ms poller tick).
- A hidden dock is parked just past its edge. When a neighbouring monitor sits on that
  side, it is instead hidden in place (`SWP_HIDEWINDOW`, kept at its shown rect) and shown
  again on reveal: the window never crosses onto another monitor, because a different DPI
  there would fire `WM_DPICHANGED` and make the page re-report its layout at the wrong
  scale (`monitors::park_strategy`).
- If the selected monitors are all unplugged (`monitors` = a list of ids that are gone),
  a dock stays on the primary monitor instead of disappearing.

## Liquid Glass refraction: rim-only CSS approximation (Task 7)

Apple's Liquid Glass bends what is behind it near its edges. A webview cannot read what
is behind its own window, and capturing the screen continuously is out (perf budget,
games). So the refraction is **approximated on the glass's own rim only**:

- `.island::before`: a 1 px ring painted with a conic gradient whose brightness varies
  around the perimeter (brightest top-left and bottom-right, as if light enters and leaves
  a thick lens), masked to the border;
- `.island::after`: the mockup's fill and inner highlights, plus a soft radial "lens band"
  that brightens the inside of the edge.

Nothing samples the desktop. The blur stays the native Acrylic under the webview.
(`src/dock/glass.css`.)

## Magnification inside the glass (Task 7)

The window is clipped to the islands (region, see above), so nothing can be drawn
outside the glass: on macOS the magnified icons rise above the dock, here **the apps
island grows with them** (wider and taller, like a glass bubble following the cursor).

- The page sizes its window box once per render for the fully magnified row
  (`maxRowGrowth`), so the window never resizes while magnifying.
- Every frame the icons change size, the page re-reports the island rects; when the window
  size is unchanged `dock_host` only rebuilds the region (`SetWindowRgn`), no
  `SetWindowPos`. If this flickers on real hardware, fallback: cap the magnification to
  what fits in a taller resting island.
- Rest icon centres are measured at render time; the cursor is compared to them (no
  feedback loop when the island grows).
- Same constraint for the launch animation: the "bounce" is a short hop of 8 px that stays
  inside the island padding (Spring: 3 hops of 0.6 s / speed; Sober: pulse), stopped when
  a window of the app appears or after 5 s.
- No outer drop shadow under the islands (it would be outside the window); a 0.5 px inner
  hairline keeps light glass readable over light wallpapers.

## Automatic glass tone (Task 7)

`src-tauri/src/glass.rs`. The tone is computed from the **wallpaper file**
(`SystemParametersInfoW(SPI_GETDESKWALLPAPER)`), assuming the "Fill" fit, over the area
behind each dock window on its monitor: average relative luminance (sRGB linearised,
Rec. 709), light above 0.6, dark below 0.4, unchanged in between (hysteresis).

- A dedicated worker thread decodes the wallpaper once per change (path + mtime + size),
  downscales it to 640 px on the long side and caches it; layout changes only re-average
  the cached image.
- Recomputed on `WM_SETTINGCHANGE` (shell listener window) and when a dock window moves or
  is resized (not on magnification frames). `dock://tone` is emitted to each dock window
  only. `GlassMode::Light/Dark` overrides the automatic tone (Rust and page both apply it).
- Limits: per-monitor wallpapers (`IDesktopWallpaper`), slideshow intermediate frames and
  fits other than Fill are not modelled; a solid-colour background uses the desktop colour
  (`GetSysColor(COLOR_DESKTOP)`); windows covering the desktop are ignored (as specified:
  wallpaper only, no screen capture).
