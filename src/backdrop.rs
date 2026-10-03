//! Backdrop generation for the dock glass panel.
//!
//! Implements the Apple `.materialrecipe` pipeline as a static render:
//! sample the wallpaper behind the panel, downscale by `backdropScale`,
//! gaussian blur with `blurRadius`, boost `saturation`, lift `brightness`,
//! then round the corners. The result is drawn as the panel's backdrop so
//! the glass tints real desktop content instead of a flat color.
//!
//! GTK 4.21+ gained the `backdrop-filter` CSS property, but it can only
//! blur content inside the same window. Blur of what lies behind a
//! transparent window is compositor work (`ext-background-effect-v1`,
//! Mutter 49+; WSLg's Weston has nothing), so until TontooCompositor
//! provides it, the backdrop is baked here.
//!
//! A guaranteed fallback gradient is used when no wallpaper is found
//! or transcoding fails – never a black / empty panel.

use image::imageops;
use image::{DynamicImage, Rgba, RgbaImage};

/// Recipe values (dockLight / dockDark.materialrecipe).
pub const BLUR_RADIUS: f32 = 24.0;
pub const BACKDROP_SCALE: f32 = 0.25;
/// Extra pixels cropped around the panel so the blur has bleed material.
const BLUR_MARGIN: i32 = 60;

/// Recipe color grading per appearance: (saturation, brightness add in
/// 0..1 units). dockLight: saturation 1.8, brightness 0.08.
/// dockDark: saturation 1.6, no brightness lift (the dark tint darkens).
pub fn grading(appearance: crate::theme::Appearance) -> (f32, f32) {
    match appearance {
        crate::theme::Appearance::Light => (1.8, 0.08),
        crate::theme::Appearance::Dark => (1.6, 0.0),
    }
}

/// Locate a wallpaper image: `DOCK_WALLPAPER` override, otherwise the
/// current Windows desktop wallpaper (readable from WSL through /mnt/c).
pub fn find_wallpaper() -> Option<String> {
    if let Ok(p) = std::env::var("DOCK_WALLPAPER") {
        if std::path::Path::new(&p).is_file() {
            return Some(p);
        }
    }
    // Derive the Windows user from the crate path
    // (/mnt/c/Users/<name>/Documents/...).
    let manifest = env!("CARGO_MANIFEST_DIR");
    let idx = manifest.find("/Users/")? + "/Users/".len();
    let user = manifest[idx..].split('/').next()?;
    let p = format!(
        "/mnt/c/Users/{user}/AppData/Roaming/Microsoft/Windows/Themes/TranscodedWallpaper"
    );
    std::path::Path::new(&p)
        .is_file()
        .then_some(p)
}

/// Scale `img` to cover `mw x mh` completely, center-cropped.
fn cover(img: &DynamicImage, mw: u32, mh: u32) -> DynamicImage {
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let scale = (mw as f32 / iw).max(mh as f32 / ih);
    let scaled = img.resize_exact(
        (iw * scale).ceil() as u32,
        (ih * scale).ceil() as u32,
        imageops::FilterType::Lanczos3,
    );
    let x = scaled.width().saturating_sub(mw) / 2;
    let y = scaled.height().saturating_sub(mh) / 2;
    scaled.crop_imm(x, y, mw, mh)
}

/// Apply the recipe color grading: saturation boost around luma, then
/// additive brightness.
fn grade(img: &RgbaImage, saturation: f32, brightness: f32) -> RgbaImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
        let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0;
        for (i, c) in [r, g, b].iter().enumerate() {
            let c = c / 255.0;
            let v = luma + (c - luma) * saturation;
            let v = (v + brightness).clamp(0.0, 1.0);
            p[i] = (v * 255.0).round() as u8;
        }
    }
    out
}

/// Multiply the alpha channel with a rounded-rect coverage mask (1 px
/// antialiased edge).
fn round_corners(img: &mut RgbaImage, radius: f32) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    for y in 0..img.height() {
        for x in 0..img.width() {
            let dx = (radius - x as f32)
                .max(x as f32 - (w - radius))
                .max(0.0);
            let dy = (radius - y as f32)
                .max(y as f32 - (h - radius))
                .max(0.0);
            let dist = (dx * dx + dy * dy).sqrt();
            let coverage = (radius - dist + 0.5).clamp(0.0, 1.0);
            let p = img.get_pixel_mut(x, y);
            p.0[3] = ((p.0[3] as f32) * coverage).round() as u8;
        }
    }
}

/// Fallback gradient when wallpaper is missing or generation fails.
/// Generates a blurred two-stop gradient matching the material tint,
/// rounded and blurred so the dock never appears black/empty.
pub fn generate_fallback(
    panel_w: i32,
    panel_h: i32,
    corner_radius: i32,
    appearance: crate::theme::Appearance,
    out_path: &str,
) -> Option<String> {
    // Appearance-matched gradient stops (opaque, will be blurred)
    let (top, bottom) = match appearance {
        crate::theme::Appearance::Light => (
            Rgba([58u8, 58u8, 61u8, 255u8]),
            Rgba([30u8, 30u8, 33u8, 255u8]),
        ),
        crate::theme::Appearance::Dark => (
            Rgba([42u8, 42u8, 44u8, 255u8]),
            Rgba([22u8, 22u8, 24u8, 255u8]),
        ),
    };
    let mut img = RgbaImage::new(panel_w as u32, panel_h as u32);
    for y in 0..panel_h {
        let t = y as f32 / (panel_h.max(1) as f32);
        // linear mix
        let r = (top[0] as f32 * (1.0 - t) + bottom[0] as f32 * t) as u8;
        let g = (top[1] as f32 * (1.0 - t) + bottom[1] as f32 * t) as u8;
        let b = (top[2] as f32 * (1.0 - t) + bottom[2] as f32 * t) as u8;
        // add subtle noise-like variation for depth
        let bright_adjust = match appearance {
            crate::theme::Appearance::Light => (t * 6.0) as u8,
            crate::theme::Appearance::Dark => 0,
        };
        for x in 0..panel_w {
            let mut px = Rgba([r, g, b, 255]);
            // slight horizontal vignette
            let hx = (x as f32 / panel_w as f32 - 0.5).abs() * 8.0;
            px[0] = px[0].saturating_sub(hx as u8).saturating_add(bright_adjust);
            px[1] = px[1].saturating_sub(hx as u8).saturating_add(bright_adjust);
            px[2] = px[2].saturating_sub(hx as u8).saturating_add(bright_adjust);
            img.put_pixel(x as u32, y as u32, px);
        }
    }
    // blur at quarter res like main pipeline for consistency
    let sw = ((panel_w as f32 * BACKDROP_SCALE).round() as u32).max(1);
    let sh = ((panel_h as f32 * BACKDROP_SCALE).round() as u32).max(1);
    let small = imageops::resize(&img, sw, sh, imageops::FilterType::Triangle);
    let blurred = imageops::blur(&small, BLUR_RADIUS * 0.5);
    let mut up = imageops::resize(&blurred, panel_w as u32, panel_h as u32, imageops::FilterType::Triangle);
    // grade like real pipeline
    let (sat, bright) = grading(appearance);
    up = grade(&up, sat, bright);
    round_corners(&mut up, corner_radius as f32);
    if let Err(e) = up.save(out_path) {
        eprintln!("[dock] backdrop fallback save failed: {e}");
        return None;
    }
    Some(out_path.to_string())
}

/// Render the blurred backdrop for the panel and save it as PNG.
///
/// `monitor` is the primary monitor rect in pixels, the panel is placed
/// bottom-center inside it (same layout as the GTK panel). Returns the
/// written file path. Falls back to a gradient if wallpaper load fails.
pub fn generate(
    wallpaper: &str,
    monitor: (i32, i32, i32, i32),
    panel_w: i32,
    panel_h: i32,
    corner_radius: i32,
    bottom_gap: i32,
    appearance: crate::theme::Appearance,
    out_path: &str,
) -> Option<String> {
    let img = match image::open(wallpaper) {
        Ok(img) => img,
        Err(first) => {
            // Windows TranscodedWallpaper has no extension; force JPEG.
            match std::fs::read(wallpaper)
                .ok()
                .and_then(|data| image::load_from_memory_with_format(&data, image::ImageFormat::Jpeg).ok())
                .or_else(|| std::fs::read(wallpaper).ok().and_then(|data| image::load_from_memory(&data).ok()))
            {
                Some(img) => img,
                None => {
                    eprintln!("[dock] backdrop: open failed: {first} -> using fallback gradient");
                    return generate_fallback(panel_w, panel_h, corner_radius, appearance, out_path);
                }
            }
        }
    };
    let (_mx, _my, mw, mh) = (monitor.0 as u32, monitor.1 as u32, monitor.2 as u32, monitor.3 as u32);

    // Wallpaper scaled to the monitor, panel region cut out (with bleed).
    // Positions are relative to the monitor: the desktop image only covers
    // this one screen, so the monitor offset plays no role here.
    let desktop = cover(&img, mw, mh);
    let px = ((mw as i32 - panel_w) / 2).max(0) as u32;
    let py = (mh as i32 - bottom_gap - panel_h).max(0) as u32;
    let cx = px.saturating_sub(BLUR_MARGIN as u32);
    let cy = py.saturating_sub(BLUR_MARGIN as u32);
    let cw = ((panel_w + 2 * BLUR_MARGIN) as u32).min(desktop.width() - cx);
    let ch = ((panel_h + 2 * BLUR_MARGIN) as u32).min(desktop.height() - cy);
    if cw == 0 || ch == 0 {
        eprintln!("[dock] backdrop: crop zero -> fallback");
        return generate_fallback(panel_w, panel_h, corner_radius, appearance, out_path);
    }
    let crop = desktop.crop_imm(cx, cy, cw, ch);

    // backdropScale: blur at quarter resolution, then upscale.
    let sw = ((cw as f32 * BACKDROP_SCALE).round() as u32).max(1);
    let sh = ((ch as f32 * BACKDROP_SCALE).round() as u32).max(1);
    let small = crop.resize_exact(sw, sh, imageops::FilterType::Triangle);
    let blurred = imageops::blur(&small, BLUR_RADIUS);
    let up = imageops::resize(&blurred, cw, ch, imageops::FilterType::Triangle);

    // Recipe color grading.
    let (sat, bright) = grading(appearance);
    let graded = grade(&up, sat, bright);

    // Cut the bleed margin away and round the corners.
    let mut panel = imageops::crop_imm(
        &graded,
        BLUR_MARGIN as u32,
        BLUR_MARGIN as u32,
        panel_w as u32,
        panel_h as u32,
    )
    .to_image();
    round_corners(&mut panel, corner_radius as f32);

    if let Err(e) = panel.save(out_path) {
        eprintln!("[dock] backdrop: save failed: {e} -> fallback");
        return generate_fallback(panel_w, panel_h, corner_radius, appearance, out_path);
    }
    Some(out_path.to_string())
}
