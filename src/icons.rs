//! Tile artwork: generated CoreIcon tiles, the LaunchPad icon and the cache
//! layout the dock prewarms.
//!
//! Every tile is rendered once per appearance into `/tmp` and served from
//! there afterwards, so a mode switch never reuses the wrong variant and the
//! first paint costs nothing. Generation is main-thread only:
//! `crate::CoreIcon::generator::ASSETS_DIR` is a `static mut`, so a second thread
//! touching `IconCanvas` is a data race.

use crate::CoreIcon::generator::*;
use crate::CoreIcon::octopus::OctopusVariant;
use crate::CoreIcon::{Color as IconColor, Gradient, GradientDirection, GradientStop, SFSymbol};
use crate::apps::{fallback_style, AppItem};
use crate::theme::Appearance;

const COREICON_ASSETS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../TontooLibs/CoreIcon/assets/icons"
);
const ICON_CORNER_RADIUS: f32 = 250.0;

/// Cache version of the generated fallback tiles
/// (`tontoo-dock-<key>-<mode>-v3.png`).
const ICON_GEN_VERSION: u32 = 3;
/// Cache version of the dock LaunchPad tile. Bumped when the dark pipeline
/// changed (flatten-over-white + flood fill): v5 `-dark-` files still
/// contain the broken all-white render and must never be reused.
const LAUNCHPAD_GEN_VERSION: u32 = 6;
/// Cache version of the legacy demo tiles (`tontoo-launchpad-<name>-v2.png`).
const DEMO_ICON_GEN_VERSION: u32 = 2;
/// Cache version of tiles generated for real programs
/// (`tontoo-launchpad-<bundle-id>-v3.png`).
const REAL_ICON_GEN_VERSION: u32 = 3;

const LAUNCHPAD_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Resources/launchpad.png");

fn debug() -> bool {
    std::env::var("DOCK_DEBUG").is_ok()
}

/// Point CoreIcon at the build tree while running from a checkout.
pub fn use_build_assets() {
    unsafe { ASSETS_DIR = COREICON_ASSETS };
}

pub fn shade(c: IconColor, amount: f32) -> IconColor {
    let mix = |v: f32| {
        if amount >= 0.0 {
            v + (1.0 - v) * amount
        } else {
            v * (1.0 + amount)
        }
    };
    IconColor::new(mix(c.r), mix(c.g), mix(c.b), 1.0)
}

fn safe_key(key: &str) -> String {
    key.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

// ── Dock tiles ──────────────────────────────────────────────────────────

/// Generate one fallback tile keyed by `key` (bundle id or demo name).
fn generate_named_icon(
    key: &str,
    hex: &str,
    symbol: SFSymbol,
    appearance: Appearance,
) -> Option<String> {
    let base = IconColor::from_hex(hex)?;
    let path = std::env::temp_dir()
        .join(format!(
            "tontoo-dock-{}-{}-v{}.png",
            safe_key(key),
            appearance.cache_suffix(),
            ICON_GEN_VERSION
        ))
        .into_os_string()
        .into_string()
        .ok()?;
    if std::path::Path::new(&path).is_file() {
        if debug() {
            println!("[dock] icon cached ({}): {path}", appearance.cache_suffix());
        }
        return Some(path);
    }
    match render_tile(&path, base, symbol) {
        Ok(()) => Some(path),
        Err(err) => {
            eprintln!("[dock] icon generation failed for {key}: {err}");
            None
        }
    }
}

/// Render one generated tile PNG to `path`.
fn render_tile(path: &str, base: IconColor, symbol: SFSymbol) -> Result<(), String> {
    let canvas = IconCanvas::new()
        .background(Background::gradient(Gradient::new(
            GradientDirection::TopToBottom,
            vec![
                GradientStop::new(shade(base, 0.28), 0.0),
                GradientStop::new(shade(base, -0.10), 1.0),
            ],
        )))
        .corner_radius(ICON_CORNER_RADIUS)
        .specular(0.15)
        .inner_depth(30.0, 0.30)
        .layer(
            Layer::new(LayerContent::icon(symbol))
                .position(212.0, 212.0)
                .size(600.0, 600.0)
                .tint(IconColor::WHITE),
        );
    canvas.save(path).map_err(|err| err.to_string())
}

/// Resolve one dock tile icon: the bundle icon file when the program ships
/// one, otherwise a generated CoreIcon fallback tile (cached per bundle id
/// and appearance).
pub fn resolve_tile_icon(item: &AppItem, appearance: Appearance) -> Option<String> {
    if let Some(path) = item.icon_path.as_deref() {
        if path.is_file() {
            return path.to_str().map(str::to_owned);
        }
    }
    generate_named_icon(&item.bundle_id, item.color, item.symbol, appearance)
}

/// Icons for the whole dock row: the LaunchPad tile first, then one per app.
pub fn dock_icons(appearance: Appearance, dock_apps: &[AppItem]) -> Vec<Option<String>> {
    use_build_assets();
    let mut icons = Vec::with_capacity(dock_apps.len() + 1);
    icons.push(launchpad_icon(appearance));
    icons.extend(dock_apps.iter().map(|item| resolve_tile_icon(item, appearance)));
    icons
}

// ── LaunchPad tile ──────────────────────────────────────────────────────

/// Render the LaunchPad tile for `appearance` without touching the cache.
///
/// Light uses `dark_light_mode(Light)` (source background is already white).
/// Dark must NOT use `dark_light_mode(Dark)`: its flood fill derives the
/// background color from the outermost border pixels, but
/// `Resources/launchpad.png` has a transparent border (RGBA 0,0,0,0), so the
/// reference color becomes near-black, no pixel matches, the mask stays empty
/// and the tile silently keeps its white background. Instead the source is
/// first flattened over opaque white (its antialiased edge is white-on-
/// transparent, so this matches its visible shape) and then run through the
/// same flood-fill background swap -- the depth finish is identical to light.
fn build_launchpad_image(
    appearance: Appearance,
) -> Result<image::RgbaImage, Box<dyn std::error::Error>> {
    use crate::CoreIcon::generator::{DepthOptions, ProcessOptions, Shadow};
    use image::imageops::FilterType;
    match appearance {
        Appearance::Light => IconCanvas::dark_light_mode(
            LAUNCHPAD_SRC,
            crate::CoreIcon::generator::IconMode::Light,
            ICON_CORNER_RADIUS,
            Some(0.0),
            Some(8.0),
            Some(18.0),
            Some(0.28),
            Some(28.0),
            Some(0.28),
            Some(0.15),
            Some(6.0),
            Some(0.32),
        ),
        Appearance::Dark => {
            // Same glass finish as the light tile, only the background differs.
            let depth = DepthOptions::new(ICON_CORNER_RADIUS)
                .shadow(Shadow::new().offset(0.0, 8.0).blur(18.0).opacity(0.28))
                .inner_depth(28.0, 0.28)
                .specular(0.15)
                .edge_highlight(6.0, 0.32);
            let src = image::open(LAUNCHPAD_SRC)?;
            let scaled = src
                .resize(
                    crate::CoreIcon::generator::CANVAS_SIZE,
                    crate::CoreIcon::generator::CANVAS_SIZE,
                    FilterType::Lanczos3,
                )
                .to_rgba8();
            // Flatten over white so the flood fill sees the opaque shape
            // `dark_light_mode` would see on a white canvas.
            let mut flat = image::RgbaImage::from_pixel(
                scaled.width(),
                scaled.height(),
                image::Rgba([255, 255, 255, 255]),
            );
            for (x, y, p) in scaled.enumerate_pixels() {
                if p[3] == 0 {
                    continue;
                }
                if p[3] == 255 {
                    flat.put_pixel(x, y, *p);
                } else {
                    let a = p[3] as f32 / 255.0;
                    flat.put_pixel(
                        x,
                        y,
                        image::Rgba([
                            (p[0] as f32 * a + 255.0 * (1.0 - a)).round() as u8,
                            (p[1] as f32 * a + 255.0 * (1.0 - a)).round() as u8,
                            (p[2] as f32 * a + 255.0 * (1.0 - a)).round() as u8,
                            255,
                        ]),
                    );
                }
            }
            Ok(IconCanvas::process_image(
                &flat,
                &ProcessOptions {
                    recolor: None,
                    background_replace: Some(crate::CoreIcon::generator::DARK_BACKGROUND),
                    depth,
                    ..Default::default()
                },
            ))
        }
    }
}

/// Cached LaunchPad tile path for `appearance`, generating it on first use.
pub fn launchpad_icon(appearance: Appearance) -> Option<String> {
    let out = std::env::temp_dir()
        .join(format!(
            "tontoo-dock-launchpad-{}-v{}.png",
            appearance.cache_suffix(),
            LAUNCHPAD_GEN_VERSION
        ))
        .into_os_string()
        .into_string()
        .ok()?;
    if std::path::Path::new(&out).is_file() {
        if debug() {
            println!("[dock] launchpad icon cached ({}): {out}", appearance.cache_suffix());
        }
        return Some(out);
    }
    use_build_assets();
    match build_launchpad_image(appearance) {
        Ok(img) => match img.save(&out) {
            Ok(()) => Some(out),
            Err(err) => {
                eprintln!("[dock] launchpad save failed: {err}");
                None
            }
        },
        Err(err) => {
            eprintln!("[dock] launchpad generate failed: {err}");
            None
        }
    }
}

// ── LaunchPad grid icons ────────────────────────────────────────────────

fn demo_icon_cache_path(name: &str) -> Option<String> {
    std::env::temp_dir()
        .join(format!(
            "tontoo-launchpad-{}-v{}.png",
            name.to_lowercase().replace(' ', "-"),
            DEMO_ICON_GEN_VERSION
        ))
        .into_os_string()
        .into_string()
        .ok()
}

/// Cache path for tiles generated for real programs (keyed by bundle id so
/// renames never collide and demo tiles are never reused).
fn real_icon_cache_path(bundle_id: &str) -> Option<String> {
    std::env::temp_dir()
        .join(format!(
            "tontoo-launchpad-{}-v{}.png",
            safe_key(bundle_id),
            REAL_ICON_GEN_VERSION
        ))
        .into_os_string()
        .into_string()
        .ok()
}

/// Icon path of one LaunchPad grid entry, generating it when missing.
///
/// Order: the bundle icon file (used as is), then the generated cache, then
/// `None` for the letter placeholder the caller draws.
pub fn grid_icon(item: &AppItem) -> Option<String> {
    if let Some(path) = item.icon_path.as_deref() {
        if path.is_file() {
            return path.to_str().map(str::to_owned);
        }
    }
    let cached = if item.bundle_path.is_some() {
        real_icon_cache_path(&item.bundle_id)
    } else {
        demo_icon_cache_path(&item.display_name)
    };
    if let Some(cached) = cached {
        if std::path::Path::new(&cached).is_file() {
            return Some(cached);
        }
    }
    None
}

/// Generate and cache the LaunchPad tile of one entry.
pub fn generate_grid_icon(item: &AppItem) -> Option<String> {
    let (path, base, symbol) = if item.bundle_path.is_some() {
        (
            real_icon_cache_path(&item.bundle_id)?,
            IconColor::from_hex(item.color)?,
            item.symbol,
        )
    } else {
        let (color, symbol) = fallback_style(&item.display_name);
        (
            demo_icon_cache_path(&item.display_name)?,
            IconColor::from_hex(color)?,
            symbol,
        )
    };
    if std::path::Path::new(&path).is_file() {
        return Some(path);
    }
    match render_tile(&path, base, symbol) {
        Ok(()) => Some(path),
        Err(err) => {
            eprintln!("[launchpad] icon generation failed for {}: {err}", item.display_name);
            None
        }
    }
}

/// Tontoo octopus icon for the LaunchPad header, cached at 64 px.
pub fn tontoo_icon(is_light: bool) -> Option<String> {
    let variant = if is_light { OctopusVariant::Black } else { OctopusVariant::White };
    let out = std::env::temp_dir()
        .join(format!(
            "tontoo-launchpad-tontoo-{}-64-v2.png",
            if is_light { "black" } else { "white" }
        ))
        .to_string_lossy()
        .to_string();
    if std::path::Path::new(&out).exists() {
        return Some(out);
    }
    use image::imageops::FilterType;
    let resize = |img: image::RgbaImage| -> Option<String> {
        let thumb = image::imageops::resize(&img, 64, 64, FilterType::Lanczos3);
        thumb.save(&out).ok().map(|_| out.clone())
    };
    let load_octopus_abs = |var: OctopusVariant| -> Option<image::RgbaImage> {
        let candidates = [
            format!(
                "{}/../../TontooLibs/CoreIcon/assets/TontooOS/{}",
                env!("CARGO_MANIFEST_DIR"),
                var.file_name()
            ),
            format!(
                "/mnt/c/Users/arlo1/Documents/TontooLibs/CoreIcon/assets/TontooOS/{}",
                var.file_name()
            ),
            format!(
                "C:/Users/arlo1/Documents/TontooLibs/CoreIcon/assets/TontooOS/{}",
                var.file_name()
            ),
            format!("/Library/System/CoreIcon/assets/TontooOS/{}", var.file_name()),
        ];
        for p in candidates {
            if let Ok(img) = image::open(&p) {
                return Some(img.to_rgba8());
            }
        }
        crate::CoreIcon::octopus::OctopusIcon::new(var).load().ok()
    };
    if let Some(img) = load_octopus_abs(variant) {
        if let Some(path) = resize(img) {
            return Some(path);
        }
    }
    let os_candidates = [
        format!(
            "{}/../../TontooLibs/CoreIcon/assets/TontooOS/OSVersionAssets/27.0.0/TontooOS_Icon.png",
            env!("CARGO_MANIFEST_DIR")
        ),
        "/mnt/c/Users/arlo1/Documents/TontooLibs/CoreIcon/assets/TontooOS/OSVersionAssets/27.0.0/TontooOS_Icon.png".to_string(),
        "C:/Users/arlo1/Documents/TontooLibs/CoreIcon/assets/TontooOS/OSVersionAssets/27.0.0/TontooOS_Icon.png".to_string(),
    ];
    for p in os_candidates {
        if let Ok(img) = image::open(&p) {
            if let Some(path) = resize(img.to_rgba8()) {
                return Some(path);
            }
        }
    }
    if let Ok(img) = crate::CoreIcon::os_version::use_osversionicons("27.0.0", "TontooOS_Icon.png") {
        if let Some(path) = resize(img) {
            return Some(path);
        }
    }
    let p = variant.path();
    if std::path::Path::new(&p).exists() {
        return Some(p);
    }
    eprintln!("[launchpad] tontoo icon not found for variant {variant:?}");
    None
}

// ── Cache prewarm ───────────────────────────────────────────────────────

/// The list the prewarm step walks: all installed programs, or the demo grid
/// when nothing is installed. Loaded once, then served index by index.
fn prewarm_list() -> &'static [AppItem] {
    use std::sync::OnceLock;
    static LIST: OnceLock<Vec<AppItem>> = OnceLock::new();
    LIST.get_or_init(|| {
        let all = crate::apps::load_all();
        if all.is_empty() {
            crate::apps::demo_launchpad()
        } else {
            all
        }
    })
}

/// One prewarm step for the dock main loop. Returns `false` when the cache is
/// fully warm. Skips files that already exist; main-thread only (see the
/// module docs on `ASSETS_DIR`).
pub fn prewarm_step() -> bool {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static IDX: AtomicUsize = AtomicUsize::new(0);
    let list = prewarm_list();
    // 0,1 = tontoo black/white, then list indices + 2
    let total = list.len() + 2;
    let i = IDX.fetch_add(1, Ordering::Relaxed);
    if i >= total {
        return false;
    }
    if i == 0 {
        let _ = tontoo_icon(true);
    } else if i == 1 {
        let _ = tontoo_icon(false);
    } else if let Some(item) = list.get(i - 2) {
        // Bundle icons need no warming (loaded straight from the bundle).
        let has_bundle_icon = item
            .icon_path
            .as_deref()
            .map(|p| p.is_file())
            .unwrap_or(false);
        if !has_bundle_icon && grid_icon(item).is_none() {
            let _ = generate_grid_icon(item);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(img: &image::RgbaImage) -> (f64, f64, u32) {
        let total = (img.width() * img.height()) as f64;
        let mut white = 0u64;
        let mut dark_bg = 0u64;
        let mut saturated = 0u32;
        for (_, _, p) in img.enumerate_pixels() {
            if p[3] < 200 {
                continue;
            }
            let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
            if r > 200 && g > 200 && b > 200 {
                white += 1;
            }
            if (r - 33).abs() < 30 && (g - 33).abs() < 30 && (b - 38).abs() < 30 {
                dark_bg += 1;
            }
            if (r - g).abs() > 60 || (r - b).abs() > 60 || (g - b).abs() > 60 {
                saturated += 1;
            }
        }
        (white as f64 / total, dark_bg as f64 / total, saturated)
    }

    #[test]
    fn dark_launchpad_tile_has_dark_background() {
        // Regression: `dark_light_mode(Dark)` flood fill derives the background
        // from transparent border pixels of `Resources/launchpad.png`, matches
        // nothing and silently keeps the white background.
        let img = build_launchpad_image(Appearance::Dark).expect("dark launchpad render");
        assert_eq!((img.width(), img.height()), (1024, 1024));
        let (white_frac, dark_frac, saturated) = stats(&img);
        assert!(
            white_frac < 0.05,
            "dark tile still has white background (white_frac={white_frac:.3})"
        );
        assert!(
            dark_frac > 0.20,
            "dark background missing (dark_frac={dark_frac:.3})"
        );
        assert!(
            saturated > 5000,
            "artwork colors lost in dark tile (saturated={saturated})"
        );
    }

    #[test]
    fn light_launchpad_tile_stays_light() {
        let img = build_launchpad_image(Appearance::Light).expect("light launchpad render");
        assert_eq!((img.width(), img.height()), (1024, 1024));
        let (white_frac, _, saturated) = stats(&img);
        assert!(
            white_frac > 0.15,
            "light tile lost its white background (white_frac={white_frac:.3})"
        );
        assert!(
            saturated > 5000,
            "artwork colors lost in light tile (saturated={saturated})"
        );
    }

    #[test]
    fn cache_keys_are_filesystem_safe() {
        assert_eq!(safe_key("com.tontoo.Terminal"), "com-tontoo-terminal");
        assert_eq!(safe_key("System Settings"), "system-settings");
    }
}