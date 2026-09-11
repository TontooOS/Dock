//! TontooOS Dock
//!
//! macOS Sonoma / Sequoia-accurate glass dock with
//! running dots, bounce, separator and HiDPI-correct X11 placement.

use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

sdk::preinclude!();
use UIKit::prelude::*;
use UIKit::widget::apply_css;
use CoreIcon::generator::*;
use CoreIcon::{
    Color as IconColor, Gradient, GradientDirection, GradientStop, SFSymbol,
};

mod apps;
mod backdrop;
mod bridge;
mod launchpad;
mod launcher;
mod pins;

/// Border width / color
const BORDER_WIDTH: i32 = 1;
const BORDER_COLOR: (u8, u8, u8) = (255, 255, 255);

// ── Panel shape (macOS continuous squircle, 18-24 px) ─────────────────
/// Corner radius – macOS dock uses ~18-24 px squircle; 20 is TontooOS default.
const PANEL_RADIUS: i32 = 20;

// ── Appearance & Materials (macOS Glass) ───────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    fn resolve() -> Self {
        if let Ok(val) = std::env::var("DOCK_APPEARANCE") {
            match val.as_str() {
                "light" => return Self::Light,
                "dark" => return Self::Dark,
                _ => {}
            }
        }
        Self::from_color_scheme(ColorScheme::detect_system())
    }
    fn from_color_scheme(cs: ColorScheme) -> Self {
        match cs {
            ColorScheme::Light => Self::Light,
            ColorScheme::Dark => Self::Dark,
        }
    }
    /// Cache key suffix: every icon is cached twice (`-light` / `-dark`)
    /// so switching appearance never reuses the wrong variant.
    fn cache_suffix(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
    fn to_color_scheme(self) -> ColorScheme {
        match self {
            Self::Light => ColorScheme::Light,
            Self::Dark => ColorScheme::Dark,
        }
    }
    /// macOS-accurate materials:
    /// Light: dunkles Schwarz, wenig Blur – mehr Hintergrund kommt durch (28% statt 36%)
    /// Dark:  rgba(30,30,30,0.60) + border rgba(255,255,255,0.10) 1px
    fn material(self) -> Material {
        match self {
            Self::Light => Material {
                panel_fill: rgba((32, 32, 34), 0.26),
                border_color: rgba((255, 255, 255), 0.12),
                inset_highlight: rgba((255, 255, 255), 0.06),
                drop_shadow: String::from("0 16px 48px rgba(0,0,0,0.22), 0 4px 16px rgba(0,0,0,0.14), inset 0 1px 0 rgba(255,255,255,0.06)"),
                tile_shadow: rgba((0, 0, 0), 0.12),
                dot_color: rgba((0, 0, 0), 0.85),
                dot_glow: rgba((0, 0, 0), 0.25),
                separator: rgba((255, 255, 255), 0.08),
                bg_fallback_top: String::from("rgba(55,55,58,0.30)"),
                bg_fallback_bottom: String::from("rgba(28,28,30,0.24)"),
            },
            Self::Dark => Material {
                panel_fill: rgba((30, 30, 30), 0.60),
                border_color: rgba(BORDER_COLOR, 0.10),
                inset_highlight: rgba((255, 255, 255), 0.08),
                drop_shadow: String::from("0 16px 48px rgba(0,0,0,0.45), 0 6px 20px rgba(0,0,0,0.38), inset 0 1px 0 rgba(255,255,255,0.06)"),
                tile_shadow: rgba((0, 0, 0), 0.40),
                dot_color: rgba((255, 255, 255), 0.92),
                dot_glow: rgba((255, 255, 255), 0.45),
                separator: rgba((255, 255, 255), 0.14),
                bg_fallback_top: String::from("rgba(42,42,44,0.65)"),
                bg_fallback_bottom: String::from("rgba(28,28,30,0.55)"),
            },
        }
    }
}

struct Material {
    panel_fill: String,
    border_color: String,
    inset_highlight: String,
    drop_shadow: String,
    tile_shadow: String,
    dot_color: String,
    dot_glow: String,
    separator: String,
    bg_fallback_top: String,
    bg_fallback_bottom: String,
}

// ── Icon layout ────────────────────────────────────────────────────────
// 10% smaller dock overall, icons only 5% smaller, icons centered in panel
const ICON_SIZE: i32 = 53; // 56 * 0.95 = 53.2
const ICON_GAP: i32 = 9; // 10 * 0.90
/// macOS bottom gap 8-12 px, we use 8 px like Sonoma.
const BOTTOM_GAP: i32 = 8;
const ROW_PAD: i32 = 20; // more breathing room left/right (was 12)
const GHOST_SIZE: i32 = (ICON_SIZE as f32 * 1.5) as i32; // 79
const GROW_DELAY_MS: u64 = 500;
const GROW_MS: u64 = 150;
const POP_SCALE: f32 = 1.65;
const POP_MS: u64 = 180;
const PANEL_SCALE: f32 = 1.2;
const DOT_SIZE: i32 = 4;
const DOT_MARGIN: i32 = 4;
// Panel width follows the live tile count (LaunchPad tile + pinned programs):
// `n * ICON_SIZE + (n - 1) * ICON_GAP + 2 * ROW_PAD`.
static TILE_COUNT: AtomicUsize = AtomicUsize::new(6);

/// Current panel width in logical pixels for the live tile count.
pub fn panel_width() -> i32 {
    let n = TILE_COUNT.load(Ordering::Relaxed).max(1) as i32;
    n * ICON_SIZE + (n - 1) * ICON_GAP + 2 * ROW_PAD
}

fn set_tile_count(n: usize) {
    TILE_COUNT.store(n.max(1), Ordering::Relaxed);
}
// Panel height is icon-driven and independent of the tile count: 90 px
// logical with the icon vertically centered (dot sits in bottom padding).
pub const PANEL_HEIGHT: i32 = ((ICON_SIZE + 2 * 11) as f32 * PANEL_SCALE).round() as i32; // 90
// Separator after Nth tile (0-indexed) – only used in demo fallback mode;
// with real programs there is no system area, so no separator is shown.

// ── i18n (SF Pro + lang/) ──────────────────────────────────────────────
const SF_FAMILY: &str = "SF Pro Display";

fn load_lang() -> HashMap<String, String> {
    let lang_code = std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .unwrap_or_else(|_| "en_US".to_string())
        .to_lowercase();
    let use_de = lang_code.starts_with("de");
    let fname = if use_de { "de_de.json" } else { "en_us.json" };
    let candidates = [
        format!("{}/lang/{}", env!("CARGO_MANIFEST_DIR"), fname),
        format!("./lang/{}", fname),
        format!("/usr/share/tontoo/dock/lang/{}", fname),
        format!("/mnt/c/Users/arlo1/Documents/TontooProgramms/Dock/lang/{}", fname),
    ];
    for p in &candidates {
        if let Ok(content) = std::fs::read_to_string(p) {
            if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&content) {
                return map;
            }
        }
    }
    let mut fallback = HashMap::new();
    for (k, v) in apps::DEMO_PROGRAMS.iter().map(|(n, _, _, _)| (*n, *n)) {
        fallback.insert(format!("dock.app.{}", k.to_lowercase()), v.to_string());
    }
    fallback.insert("dock.launchpad".to_string(), "Launchpad".to_string());
    fallback
}
fn tr(map: &HashMap<String, String>, key: &str, fallback: &str) -> String {
    map.get(key).cloned().unwrap_or_else(|| fallback.to_string())
}

// ── Bounce physics ─────────────────────────────────────────────────────
fn calculate_y_displacement(t: f32, duration: f32, max_h: f32) -> f32 {
    let progress = (t / duration).clamp(0.0, 1.0);
    if progress >= 1.0 {
        return 0.0;
    }
    let p = progress % 1.0;
    if p < 0.42 {
        let norm_t = p / 0.42;
        -max_h * 4.0 * norm_t * (1.0 - norm_t)
    } else if p < 0.76 {
        let norm_t = (p - 0.42) / 0.34;
        -max_h * 0.35 * 4.0 * norm_t * (1.0 - norm_t)
    } else {
        let norm_t = (p - 0.76) / 0.24;
        -max_h * 0.09 * 4.0 * norm_t * (1.0 - norm_t)
    }
}

fn bounce_tile_dynamics(btn: &gtk::Button) {
    let max_h: f32 = 52.0;
    let duration: f32 = 900.0;
    let start = std::time::Instant::now();
    btn.set_overflow(gtk::Overflow::Visible);
    if let Some(p) = btn.parent() {
        p.set_overflow(gtk::Overflow::Visible);
        if let Some(pp) = p.parent() {
            pp.set_overflow(gtk::Overflow::Visible);
            if let Some(ppp) = pp.parent() {
                ppp.set_overflow(gtk::Overflow::Visible);
            }
        }
    }
    let provider = Rc::new(RefCell::new(gtk::CssProvider::new()));
    {
        let p = provider.borrow();
        p.load_from_string("* { transform: translateY(0); }");
        btn.style_context()
            .add_provider(&*p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 30);
    }
    let btn_c = btn.clone();
    let prov_c = provider.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        let t = start.elapsed().as_millis() as f32;
        let y = calculate_y_displacement(t, duration, max_h);
        prov_c
            .borrow()
            .load_from_string(&format!("* {{ transform: translateY({y}px); }}"));
        if t >= duration {
            prov_c
                .borrow()
                .load_from_string("* { transform: translateY(0); }");
            let prov2 = prov_c.clone();
            let btn2 = btn_c.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                btn2.style_context().remove_provider(&*prov2.borrow());
            });
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

// ── Temp icon generation ───────────────────────────────────────────────
const COREICON_ASSETS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../TontooLibs/CoreIcon/assets/icons"
);
const ICON_CORNER_RADIUS: f32 = 250.0;
const ICON_GEN_VERSION: u32 = 3;
/// Cache version of the Launchpad tile. Bumped when the dark pipeline changed
/// (flatten-over-white + flood fill): v5 `-dark-` files still contain the
/// broken all-white render and must never be reused.
const LAUNCHPAD_GEN_VERSION: u32 = 6;
const LAUNCHPAD_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Resources/launchpad.png");

// NOTE: the pinned programs come from CoreWindows (`apps::load()`); the demo
// fallback table lives in `apps::DEMO_PROGRAMS`. There is no hardcoded APPS
// list here anymore.

fn rgba(c: (u8, u8, u8), a: f64) -> String {
    format!("rgba({}, {}, {}, {})", c.0, c.1, c.2, a)
}
fn shade(c: IconColor, amount: f32) -> IconColor {
    let mix = |v: f32| {
        if amount >= 0.0 {
            v + (1.0 - v) * amount
        } else {
            v * (1.0 + amount)
        }
    };
    IconColor::new(mix(c.r), mix(c.g), mix(c.b), 1.0)
}

/// Generate one fallback tile keyed by `key` (bundle id or demo name).
fn generate_named_icon(
    key: &str,
    hex: &str,
    symbol: SFSymbol,
    appearance: Appearance,
) -> Option<String> {
    let safe: String = key
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let base = IconColor::from_hex(hex)?;
    let path = std::env::temp_dir()
        .join(format!(
            "tontoo-dock-{}-{}-v{}.png",
            safe,
            appearance.cache_suffix(),
            ICON_GEN_VERSION
        ))
        .into_os_string()
        .into_string()
        .ok()?;
    if std::path::Path::new(&path).is_file() {
        if std::env::var("DOCK_DEBUG").is_ok() {
            println!("[dock] icon cached ({}): {path}", appearance.cache_suffix());
        }
        return Some(path);
    }
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
    match canvas.save(&path) {
        Ok(()) => Some(path),
        Err(err) => {
            eprintln!("[dock] icon generation failed for {key}: {err}");
            None
        }
    }
}

/// Render the Launchpad tile for `appearance` without touching the cache.
///
/// Light uses `dark_light_mode(Light)` (source background is already white).
/// Dark must NOT use `dark_light_mode(Dark)`: its flood fill derives the
/// background color from the outermost border pixels, but
/// `Resources/launchpad.png` has a transparent border (RGBA 0,0,0,0), so the
/// reference color becomes near-black, no pixel matches, the mask stays empty
/// and the tile silently keeps its white background. Instead the source is
/// first flattened over opaque white (its antialiased edge is white-on-
/// transparent, so this matches its visible shape) and then run through the
/// same flood-fill background swap – the depth finish is identical to light.
fn build_launchpad_image(
    appearance: Appearance,
) -> Result<image::RgbaImage, Box<dyn std::error::Error>> {
    use CoreIcon::generator::{DepthOptions, ProcessOptions, Shadow};
    use image::imageops::FilterType;
    match appearance {
        Appearance::Light => IconCanvas::dark_light_mode(
            LAUNCHPAD_SRC,
            CoreIcon::generator::IconMode::Light,
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
                    CoreIcon::generator::CANVAS_SIZE,
                    CoreIcon::generator::CANVAS_SIZE,
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
                    background_replace: Some(CoreIcon::generator::DARK_BACKGROUND),
                    depth,
                    ..Default::default()
                },
            ))
        }
    }
}

fn generate_launchpad_icon(appearance: Appearance) -> Option<String> {
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
        if std::env::var("DOCK_DEBUG").is_ok() {
            println!("[dock] icon cached ({}): {out}", appearance.cache_suffix());
        }
        return Some(out);
    }
    let res = build_launchpad_image(appearance);
    match res {
        Ok(img) => match img.save(&out) {
            Ok(()) => Some(out),
            Err(e) => {
                eprintln!("[dock] launchpad save failed: {e}");
                None
            }
        },
        Err(e) => {
            eprintln!("[dock] launchpad generate failed: {e}");
            None
        }
    }
}

/// Resolve one dock tile icon: the bundle icon file when the program ships
/// one, otherwise a generated CoreIcon fallback tile (cached per bundle id
/// and appearance).
fn resolve_tile_icon(item: &apps::AppItem, appearance: Appearance) -> Option<String> {
    if let Some(path) = item.icon_path.as_deref() {
        if path.is_file() {
            return path.to_str().map(str::to_owned);
        }
    }
    generate_named_icon(&item.bundle_id, item.color, item.symbol, appearance)
}

fn generate_temp_icons(appearance: Appearance, dock_apps: &[apps::AppItem]) -> Vec<Option<String>> {
    unsafe { ASSETS_DIR = COREICON_ASSETS };
    let mut icons = Vec::with_capacity(dock_apps.len() + 1);
    icons.push(generate_launchpad_icon(appearance));
    icons.extend(
        dock_apps
            .iter()
            .map(|item| resolve_tile_icon(item, appearance)),
    );
    icons
}

// ── Click toggling (minimize / restore / launch) ───────────────────────
// Single click on a running app minimizes all its visible windows;
// clicking again restores all minimized ones (macOS behavior). Closed
// apps launch out-of-process via the launcher.
fn toggle_app_windows(item: &apps::AppItem) {
    let snapshot = apps::open_snapshot();
    if !snapshot.daemon_ok {
        // No daemon: fall back to launching.
        return launch_item(item);
    }
    let windows = apps::app_windows(item, &snapshot);
    if windows.is_empty() {
        return launch_item(item);
    }
    let provider = crate::CoreWindows::WindowsProvider::from_env();
    let visible: Vec<u64> = windows
        .iter()
        .filter(|(_, minimized)| !minimized)
        .map(|(id, _)| *id)
        .collect();
    if !visible.is_empty() {
        for id in visible {
            match provider.minimize_window(id) {
                Ok(()) => println!("[dock] minimized {} (window {id})", item.display_name),
                Err(err) => eprintln!("[dock] minimize window {id} failed: {err}"),
            }
        }
        return;
    }
    for (id, _) in &windows {
        match provider.restore_window(*id) {
            Ok(()) => println!("[dock] restored {} (window {id})", item.display_name),
            Err(err) => eprintln!("[dock] restore window {id} failed: {err}"),
        }
    }
}

fn launch_item(item: &apps::AppItem) {
    match item.bundle_path.as_deref() {
        Some(bundle_path) => launcher::launch_app(bundle_path, &item.display_name),
        None => println!("[dock] launch request: {}", item.display_name),
    }
}

/// Dots-refresh keys for a tile row: `None` for the LaunchPad opener,
// shared so pin rebuilds stay exact.
fn tile_keys_for(dock_apps: &[apps::AppItem]) -> Vec<Option<apps::AppItem>> {
    let mut keys = Vec::with_capacity(dock_apps.len() + 1);
    keys.push(None);
    keys.extend(dock_apps.iter().cloned().map(Some));
    keys
}

// ── Dock panel view ────────────────────────────────────────────────────
/// Shared dock content, reloaded live when pins change (see the pins-watch
/// timeout in `render`). Everything tile-related reads from here.
struct DockData {
    /// Pinned programs (without the LaunchPad tile).
    dock_apps: Vec<apps::AppItem>,
    /// All installed programs, opened in the LaunchPad.
    all_apps: Vec<apps::AppItem>,
    /// Tile icons aligned with the tiles: `[launchpad, app0, app1, ...]`.
    icons: Vec<Option<String>>,
    /// Open-program snapshot for the running dots.
    running: apps::OpenSnapshot,
    /// Separator tile index (`Some(4)` in demo mode, `None` with real data).
    separator_after: Option<usize>,
    appearance: Appearance,
    backdrop: Option<String>,
    lang: HashMap<String, String>,
}

struct DockPanelView {
    data: Rc<RefCell<DockData>>,
}

#[derive(Clone)]
struct DragState {
    src_pos: usize,
    src_index: usize,
}

struct DockState {
    order: Vec<usize>,
    wrappers: Vec<gtk::Overlay>,
    buttons: Vec<gtk::Button>,
    dots: Vec<gtk::Box>,
    row: gtk::Box,
    ghost: gtk::Box,
    overlay: gtk::Overlay,
    top_overlay: gtk::Overlay,
    placeholder: gtk::Box,
    placeholder_above: gtk::Box,
    dragging: Option<DragState>,
    suppress_click: bool,
}

/// What a click on a tile does (captured per tile in `build_row`).
#[derive(Clone)]
enum TileAction {
    Launchpad,
    // Real installed program: click toggles minimize/restore,
    // launches when not running.
    Real { item: apps::AppItem },
    // Demo fallback entry: legacy log/spawn behavior.
    Demo {
        name: String,
        cmd: Option<&'static str>,
    },
}

struct TileDef {
    name: String,
    fallback_letter: String,
    item: Option<apps::AppItem>,
    action: TileAction,
}

impl ViewContent for DockPanelView {
    fn render(&self, _frame: Rect) -> gtk::Widget {
        let top_overlay = gtk::Overlay::new();
        top_overlay.set_hexpand(true);
        top_overlay.set_vexpand(true);
        let bg = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bg.add_css_class("dock-bg");
        bg.set_hexpand(true);
        bg.set_vexpand(true);
        bg.set_overflow(gtk::Overflow::Visible);
        top_overlay.set_overflow(gtk::Overflow::Visible);
        top_overlay.set_child(Some(&bg));

        let overlay = gtk::Overlay::new();
        overlay.set_size_request(panel_width(), PANEL_HEIGHT);
        overlay.set_halign(gtk::Align::Center);
        overlay.set_valign(gtk::Align::End);
        overlay.set_margin_bottom(BOTTOM_GAP);
        overlay.set_overflow(gtk::Overflow::Visible);

        if let Some(path) = &self.data.borrow().backdrop {
            let img = gtk::Image::from_file(path);
            img.set_halign(gtk::Align::Fill);
            img.set_valign(gtk::Align::Fill);
            img.set_overflow(gtk::Overflow::Visible);
            overlay.set_child(Some(&img));
        }

        let glass = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        glass.add_css_class("dock-panel");
        glass.set_overflow(gtk::Overflow::Visible);
        let left_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        left_spacer.set_hexpand(true);
        let right_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        right_spacer.set_hexpand(true);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, ICON_GAP);
        row.set_halign(gtk::Align::Center);
        row.set_valign(gtk::Align::Center);
        row.set_overflow(gtk::Overflow::Visible);

        let ghost = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        ghost.set_size_request(GHOST_SIZE, GHOST_SIZE);
        ghost.set_halign(gtk::Align::Start);
        ghost.set_valign(gtk::Align::Start);
        ghost.set_visible(false);
        ghost.add_css_class("ghost-tile");
        // overlay order fixed later – ghost on top of dock

        let placeholder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        placeholder.set_size_request(ICON_SIZE, ICON_SIZE);
        placeholder.add_css_class("placeholder-tile");
        placeholder.set_visible(false);
        // placeholder shown ABOVE the dock during drag (not inside/under it)
        let placeholder_above = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        placeholder_above.set_size_request(ICON_SIZE, ICON_SIZE);
        placeholder_above.add_css_class("placeholder-tile");
        placeholder_above.set_halign(gtk::Align::Start);
        placeholder_above.set_valign(gtk::Align::Start);
        placeholder_above.set_visible(false);
        // overlay order fixed later – placeholder_above on top of dock

        let tile_count = self.data.borrow().dock_apps.len() + 1;
        let state = Rc::new(RefCell::new(DockState {
            order: (0..tile_count).collect(),
            wrappers: Vec::new(),
            buttons: Vec::new(),
            dots: Vec::new(),
            row: row.clone(),
            ghost: ghost.clone(),
            overlay: overlay.clone(),
            top_overlay: top_overlay.clone(),
            placeholder: placeholder.clone(),
            placeholder_above: placeholder_above.clone(),
            dragging: None,
            suppress_click: false,
        }));

        // Native GTK tooltip (`tile.set_tooltip_text`) shows the app name
        // after a short delay with system styling – no custom hover label.

        let material = self.data.borrow().appearance.material();
        // Decide panel background: if backdrop PNG exists use its baked image (tint already inside),
        // otherwise use fallback gradient with blur (guaranteed not black).
        let has_backdrop = self.data.borrow().backdrop.is_some();
        let panel_bg = if has_backdrop {
            material.panel_fill.clone()
        } else {
            format!("linear-gradient(to bottom, {}, {})", material.bg_fallback_top, material.bg_fallback_bottom)
        };
        // Separation border color
        let sep_border = material.separator.clone();
        let css = format!(
            r#"
            * {{ font-family: '{sf}', 'SF Pro Text', sans-serif; }}
            .dock-panel {{
                background: {bg};
                background-color: {bg_color};
                border-radius: {radius}px;
                border: {bw}px solid {border};
                box-shadow: {shadow};
                backdrop-filter: blur(24px) saturate(180%);
            }}
            .dock-item {{
                background: transparent;
                border: none;
                padding: 0;
                margin: 0;
            }}
            .dock-item-sep {{
                border-right: 1px solid {sep};
                padding-right: 10px;
                margin-right: 4px;
            }}
            .placeholder-tile {{
                background-color: rgba(255,255,255,0.12);
                border: 2px dashed rgba(255,255,255,0.28);
                border-radius: 14px;
                min-width: {isz}px;
                min-height: {isz}px;
                transition: all 220ms cubic-bezier(0.2, 0, 0, 1);
            }}
            .ghost-tile {{
                background: transparent;
                border-radius: 14px;
                box-shadow: 0 12px 32px rgba(0,0,0,0.45);
            }}
            .ghost-tile image {{
                transition: all 90ms ease-out;
            }}
            box {{
                transition: all 220ms cubic-bezier(0.2, 0, 0, 1);
            }}
            button.dock-tile {{
                background-image: none;
                background-color: transparent;
                border-style: none;
                border-width: 0px;
                outline-style: none;
                border-radius: {tile_r}px;
                box-shadow: 0 2px 8px {tile_shadow};
                min-width: {isz}px;
                min-height: {isz}px;
                padding: 0px;
                transition: transform 140ms cubic-bezier(0.2, 0, 0, 1);
                transform-origin: bottom center;
            }}
            button.dock-tile > label {{
                font-family: '{sf}';
                font-size: 20px;
                font-weight: bold;
                color: #ffffff;
                text-shadow: 0 1px 6px rgba(0, 0, 0, 0.6);
            }}
            .dock-dot {{
                background-color: {dot_color};
                border-radius: 9999px;
                min-width: {dot}px;
                min-height: {dot}px;
                max-width: {dot}px;
                max-height: {dot}px;
                box-shadow: 0 0 5px {dot_glow}, 0 1px 2px rgba(0,0,0,0.45);
            }}
            .dock-dot-hidden {{
                opacity: 0;
            }}
            overlay, .dock-panel, box, button.dock-tile {{
                overflow: visible;
            }}
            separator, .separator, Separator {{
                background: transparent;
                background-color: transparent;
                border: none;
                min-width: 0;
                min-height: 0;
                opacity: 0;
            }}
            * {{
                outline: none;
                -gtk-outline-style: none;
            }}
            button:focus, button:active {{
                outline: none;
                box-shadow: none;
            }}
            @keyframes dock-spring {{
                0% {{ transform: translateY(0) scale(1); }}
                18% {{ transform: translateY(-52px) scale(1.03); }}
                35% {{ transform: translateY(0) scale(1); }}
                52% {{ transform: translateY(-26px) scale(1.015); }}
                68% {{ transform: translateY(0) scale(1); }}
                80% {{ transform: translateY(-10px) scale(1.005); }}
                90% {{ transform: translateY(0) scale(1); }}
                100% {{ transform: translateY(0) scale(1); }}
            }}
            .dock-spring {{
                animation: dock-spring 1300ms cubic-bezier(0.28, 1.4, 0.55, 1) forwards;
                transform-origin: bottom center;
                will-change: transform;
            }}"#,
            sf = SF_FAMILY,
            bg = panel_bg,
            bg_color = if has_backdrop { material.panel_fill.clone() } else { "transparent".to_string() },
            radius = PANEL_RADIUS,
            bw = BORDER_WIDTH,
            border = material.border_color,
            shadow = material.drop_shadow,
            sep = sep_border,
            isz = ICON_SIZE,
            tile_r = ICON_SIZE / 4,
            tile_shadow = material.tile_shadow,
            dot = DOT_SIZE,
            dot_color = material.dot_color,
            dot_glow = material.dot_glow,
        );
        apply_css(&glass, &css);

        let sliders_launching = Rc::new(RefCell::new(false));
        let sliders_bounce_count = Rc::new(RefCell::new(0u32));

        /// (Re)build all dock tiles from `data` into `state` (wrappers,
        /// buttons, dots). Runs for the initial row and again on every pin
        /// change, so the dock updates live without restarting. Callers
        /// clear `state` vectors and refill `row` afterwards.
        fn build_row(
            data: &DockData,
            state: &Rc<RefCell<DockState>>,
            sliders_launching: &Rc<RefCell<bool>>,
            sliders_bounce_count: &Rc<RefCell<u32>>,
        ) {
        let launchpad_name = tr(&data.lang, "dock.app.launchpad", "Launchpad");
        let mut tiles: Vec<TileDef> = Vec::with_capacity(data.dock_apps.len() + 1);
        tiles.push(TileDef {
            name: launchpad_name,
            fallback_letter: String::from("L"),
            item: None,
            action: TileAction::Launchpad,
        });
        for item in &data.dock_apps {
            let action = match &item.bundle_path {
                Some(_) => TileAction::Real { item: item.clone() },
                None => TileAction::Demo {
                    name: item.display_name.clone(),
                    cmd: item.demo_cmd,
                },
            };
            tiles.push(TileDef {
                name: item.display_name.clone(),
                fallback_letter: item
                    .display_name
                    .chars()
                    .next()
                    .unwrap_or('A')
                    .to_string(),
                item: Some(item.clone()),
                action,
            });
        }

        for (index, tile_def) in tiles.iter().enumerate() {
            let name = tile_def.name.as_str();
            // wrapper = overlay so icon is perfectly centered in dock, dot is overlay below (does not affect centering)
            let wrapper = gtk::Overlay::new();
            wrapper.set_size_request(ICON_SIZE, ICON_SIZE);
            wrapper.add_css_class("dock-item");
            wrapper.set_halign(gtk::Align::Center);
            wrapper.set_valign(gtk::Align::Center);
            wrapper.set_overflow(gtk::Overflow::Visible);
            if data.separator_after == Some(index) {
                wrapper.add_css_class("dock-item-sep");
            }

            let tile = gtk::Button::new();
            tile.add_css_class("dock-tile");
            tile.set_size_request(ICON_SIZE, ICON_SIZE);
            tile.set_can_focus(false);
            tile.set_halign(gtk::Align::Center);
            tile.set_valign(gtk::Align::Center);
            let localized = tr(&data.lang, &format!("dock.app.{}", name.to_lowercase()), name);
            tile.set_tooltip_text(Some(&localized));
            apply_css(
                &tile,
                &format!(
                    "button {{ background-image: none; background-color: transparent; border-style: none; border-width: 0px; outline-style: none; padding: 0px; min-width: {ICON_SIZE}px; min-height: {ICON_SIZE}px; }}"
                ),
            );

            match data.icons.get(index).and_then(|p| p.as_ref()) {
                Some(path) => {
                    let image = gtk::Image::from_file(path);
                    image.set_pixel_size(ICON_SIZE);
                    tile.set_child(Some(&image));
                }
                None => {
                    tile.set_child(Some(&gtk::Label::new(Some(tile_def.fallback_letter.as_str()))));
                    apply_css(
                        &tile,
                        &format!(
                            "button.dock-tile {{ background-color: {}; }}",
                            ["#2979FF", "#4CAF50", "#6C5CE7"][index % 3]
                        ),
                    );
                }
            }

            wrapper.set_child(Some(&tile));

            {
                let mut s = state.borrow_mut();
                s.wrappers.push(wrapper.clone());
                s.buttons.push(tile.clone());
            }

            // dot indicator – overlay below icon, does not shift icon center
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("dock-dot");
            dot.set_halign(gtk::Align::Center);
            dot.set_valign(gtk::Align::Start);
            // place dot 4px below icon bottom (icon 53 + 4 margin = 57)
            dot.set_margin_top(ICON_SIZE + DOT_MARGIN);
            // keep dot layout stable even when hidden (opacity 0)
            // Tile 0 is the LaunchPad opener and never shows a dot.
            let running = tile_def
                .item
                .as_ref()
                .map(|item| apps::is_running(item, &data.running))
                .unwrap_or(false);
            if !running {
                dot.add_css_class("dock-dot-hidden");
            }
            dot.set_size_request(DOT_SIZE, DOT_SIZE);
            {
                let mut s = state.borrow_mut();
                s.dots.push(dot.clone());
            }

            wrapper.add_overlay(&dot);

            // Click + long-press drag
            let press_gesture = gtk::GestureClick::new();
            press_gesture.set_button(1);
            press_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
            press_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
            let timeout_id: Rc<RefCell<Option<glib::SourceId>>> =
                Rc::new(RefCell::new(None));
            let state_lp = state.clone();
            let tile_weak = tile.downgrade();
            let wrapper_weak = wrapper.downgrade();
            let ghost_path = data.icons.get(index).and_then(|p| p.clone());
            let letter_c = tile_def.fallback_letter.clone();
            let idx_c = index;
            let action_click = tile_def.action.clone();
            let timeout_id_pressed = timeout_id.clone();
            let lang_lp = data.lang.clone();
            press_gesture.connect_pressed({
                let state_lp = state_lp.clone();
                let timeout_id = timeout_id_pressed.clone();
                let tile_weak = tile_weak.clone();
                let wrapper_weak = wrapper_weak.clone();
                move |gesture, n_press, x, y| {
                    if n_press != 1 {
                        return;
                    }
                    if let Some(id) = timeout_id.borrow_mut().take() {
                        id.remove();
                    }
                    let state_anim = state_lp.clone();
                    let tile_weak2 = tile_weak.clone();
                    let wrapper_weak2 = wrapper_weak.clone();
                    let ghost_path2 = ghost_path.clone();
                    let gesture_weak = gesture.downgrade();
                    {
                        let mut s = state_anim.borrow_mut();
                        while let Some(child) = s.ghost.first_child() {
                            s.ghost.remove(&child);
                        }
                        if let Some(path) = &ghost_path2 {
                            let img = gtk::Image::from_file(path);
                            img.set_pixel_size(ICON_SIZE);
                            s.ghost.append(&img);
                        } else {
                            let lbl = gtk::Label::new(Some(letter_c.as_str()));
                            s.ghost.append(&lbl);
                        }
                        let (tx, ty) = if let Some(btn) = tile_weak2.upgrade() {
                            btn.translate_coordinates(&s.top_overlay, x, y)
                                .unwrap_or((x, y))
                        } else {
                            (x, y)
                        };
                        s.ghost.set_visible(false);
                        s.ghost
                            .set_margin_start((tx as i32) - ICON_SIZE / 2);
                        s.ghost.set_margin_top((ty as i32) - ICON_SIZE / 2);
                    }
                    let start = std::time::Instant::now();
                    let state_timeout = state_lp.clone();
                    let timeout_id_clone = timeout_id.clone();
                    let id = glib::timeout_add_local(
                        std::time::Duration::from_millis(16),
                        move || {
                            let elapsed_ms = start.elapsed().as_millis() as u64;
                            let (progress, _eased, cur_size) = if elapsed_ms < GROW_DELAY_MS {
                                (0.0, 0.0, ICON_SIZE)
                            } else if elapsed_ms < GROW_DELAY_MS + GROW_MS {
                                let p = (elapsed_ms - GROW_DELAY_MS) as f64
                                    / GROW_MS as f64;
                                let e = 1.0 - (1.0 - p).powi(3);
                                let sz = (ICON_SIZE as f64
                                    + (GHOST_SIZE as f64 - ICON_SIZE as f64) * e)
                                    as i32;
                                (p, e, sz)
                            } else {
                                (1.0, 1.0, GHOST_SIZE)
                            };
                            let mut s = match state_timeout.try_borrow_mut() {
                                Ok(s) => s,
                                Err(_) => return glib::ControlFlow::Continue,
                            };
                            if elapsed_ms >= GROW_DELAY_MS && !s.ghost.is_visible() {
                                s.ghost.set_visible(true);
                            }
                            if elapsed_ms < GROW_DELAY_MS {
                                if let Some(btn) = tile_weak2.upgrade() {
                                    if let Some(gesture) = gesture_weak.upgrade() {
                                        let (tx, ty) = if let Some(event) =
                                            gesture.current_event()
                                        {
                                            if let Some((ex, ey)) = event.position() {
                                                btn.translate_coordinates(
                                                    &s.top_overlay, ex, ey,
                                                )
                                                .unwrap_or((x, y))
                                            } else {
                                                btn.translate_coordinates(
                                                    &s.top_overlay, x, y,
                                                )
                                                .unwrap_or((x, y))
                                            }
                                        } else {
                                            btn.translate_coordinates(&s.top_overlay, x, y)
                                                .unwrap_or((x, y))
                                        };
                                        s.ghost.set_margin_start(
                                            (tx as i32) - ICON_SIZE / 2,
                                        );
                                        s.ghost
                                            .set_margin_top((ty as i32) - ICON_SIZE / 2);
                                    }
                                }
                                return glib::ControlFlow::Continue;
                            }
                            if let Some(child) = s.ghost.first_child() {
                                if let Some(img) =
                                    child.downcast_ref::<gtk::Image>()
                                {
                                    img.set_pixel_size(cur_size);
                                }
                            }
                            if let Some(btn) = tile_weak2.upgrade() {
                                if let Some(gesture) = gesture_weak.upgrade() {
                                    let (tx, ty) = if let Some(event) =
                                        gesture.current_event()
                                    {
                                        if let Some((ex, ey)) = event.position() {
                                            btn.translate_coordinates(
                                                &s.top_overlay, ex, ey,
                                            )
                                            .unwrap_or((x, y))
                                        } else {
                                            btn.translate_coordinates(
                                                &s.top_overlay, x, y,
                                            )
                                            .unwrap_or((x, y))
                                        }
                                    } else {
                                        btn.translate_coordinates(&s.top_overlay, x, y)
                                            .unwrap_or((x, y))
                                    };
                                    s.ghost.set_margin_start(
                                        (tx as i32) - cur_size / 2,
                                    );
                                    s.ghost
                                        .set_margin_top((ty as i32) - cur_size / 2);
                                }
                            }
                            if progress >= 1.0 {
                                *timeout_id_clone.borrow_mut() = None;
                                let ghost_clone = s.ghost.clone();
                                let pop_size =
                                    (ICON_SIZE as f32 * POP_SCALE) as i32;
                                if let Some(child) = ghost_clone.first_child() {
                                    if let Some(img) =
                                        child.downcast_ref::<gtk::Image>()
                                    {
                                        img.set_pixel_size(pop_size);
                                    }
                                }
                                let ghost_pop = ghost_clone.clone();
                                glib::timeout_add_local_once(
                                    std::time::Duration::from_millis(POP_MS / 2),
                                    move || {
                                        if let Some(child) =
                                            ghost_pop.first_child()
                                        {
                                            if let Some(img) =
                                                child.downcast_ref::<gtk::Image>()
                                            {
                                                img.set_pixel_size(GHOST_SIZE);
                                            }
                                        }
                                    },
                                );
                                let src_pos = s
                                    .order
                                    .iter()
                                    .position(|&i| i == idx_c)
                                    .unwrap_or(0);
                                s.dragging = Some(DragState {
                                    src_pos,
                                    src_index: idx_c,
                                });
                                s.suppress_click = true;
                                // remove wrapper (not just button)
                                if let Some(w) = wrapper_weak2.upgrade() {
                                    s.row.remove(&w);
                                } else {
                                    s.row.remove(&s.buttons[idx_c]);
                                }
                                // ensure button opacity reset (button inside wrapper still)
                                s.buttons[idx_c].set_opacity(1.0);
                                // placeholder both INSIDE row (for gap animation) and ABOVE dock (as requested)
                                let (row_x, _) = s.row.translate_coordinates(&s.top_overlay, 0.0, 0.0).unwrap_or((0.0, 0.0));
                                let slot_w = ICON_SIZE + ICON_GAP;
                                let placeholder_pos = src_pos.min(s.row.observe_children().n_items() as usize);
                                if placeholder_pos == 0 {
                                    s.row.prepend(&s.placeholder);
                                } else if let Some(sib) = s
                                    .row
                                    .observe_children()
                                    .item((placeholder_pos - 1) as u32)
                                    .and_then(|o| o.downcast::<gtk::Widget>().ok())
                                {
                                    s.row.insert_child_after(&s.placeholder, Some(&sib));
                                } else {
                                    s.row.append(&s.placeholder);
                                }
                                s.placeholder.set_visible(true);
                                s.placeholder_above.set_margin_start((row_x as i32) + (placeholder_pos as i32)*slot_w);
                                s.placeholder_above.set_margin_top(GHOST_SIZE - ICON_SIZE - 10);
                                s.placeholder_above.set_visible(true);
                                if let Some(win) = s
                                    .overlay
                                    .root()
                                    .and_then(|r| r.downcast::<gtk::Window>().ok())
                                {
                                    let scale = win.scale_factor();
                                    if let Some(surface) = win.surface() {
                                        if let Some(xid) =
                                            x11_place::xid_of(&surface)
                                        {
                                            let win_w = panel_width() + GHOST_SIZE;
                                            let win_h =
                                                PANEL_HEIGHT + GHOST_SIZE + BOTTOM_GAP;
                                            x11_place::set_input_region(
                                                xid,
                                                0,
                                                0,
                                                (win_w * scale) as u16,
                                                (win_h * scale) as u16,
                                            );
                                            let region =
                                                gtk::cairo::Region::create_rectangle(
                                                    &gtk::cairo::RectangleInt::new(
                                                        0, 0, win_w, win_h,
                                                    ),
                                                );
                                            surface.set_input_region(&region);
                                        }
                                    }
                                }
                                return glib::ControlFlow::Break;
                            }
                            if let Some(gesture) = gesture_weak.upgrade() {
                                if !gesture.is_active() {
                                    s.ghost.set_visible(false);
                                    while let Some(child) = s.ghost.first_child() {
                                        s.ghost.remove(&child);
                                    }
                                    *timeout_id_clone.borrow_mut() = None;
                                    return glib::ControlFlow::Break;
                                }
                            }
                            glib::ControlFlow::Continue
                        },
                    );
                    *timeout_id.borrow_mut() = Some(id);
                }
            });
            let state_released = state.clone();
            let timeout_id_released = timeout_id.clone();
            let tile_click = tile.clone();
            let launching_click = sliders_launching.clone();
            let cnt_click = sliders_bounce_count.clone();
            let action_click = action_click.clone();
            let all_apps_click = data.all_apps.clone();
            let lang_click = lang_lp.clone();
            press_gesture.connect_released(move |gesture, n_press, x, y| {
                if n_press != 1 {
                    return;
                }
                let is_dragging = state_released.borrow().dragging.is_some();
                if is_dragging {
                    let mut s = state_released.borrow_mut();
                    let drag = match s.dragging.take() {
                        Some(d) => d,
                        None => return,
                    };
                    s.ghost.set_visible(false);
                    while let Some(child) = s.ghost.first_child() {
                        s.ghost.remove(&child);
                    }
                    s.buttons[drag.src_index].set_opacity(1.0);
                    // compute drop position from placeholder ABOVE dock
                    let (row_x_f, _) = s.row.translate_coordinates(&s.top_overlay, 0.0, 0.0).unwrap_or((0.0, 0.0));
                    let row_x = row_x_f as i32;
                    let slot_w = ICON_SIZE + ICON_GAP;
                    let n = s.order.len() as i32;
                    let new_pos = ((s.placeholder_above.margin_start() - row_x).div_euclid(slot_w)).clamp(0, n) as usize;
                    s.placeholder_above.set_visible(false);
                    if s.placeholder.parent().is_some() {
                        s.row.remove(&s.placeholder);
                    }
                    s.placeholder.set_visible(false);
                    let old_pos = drag.src_pos;
                    if new_pos != old_pos {
                        let app_idx = s.order.remove(old_pos);
                        let insert_at = new_pos.min(s.order.len());
                        // adjust if dragging forward: placeholder above counts gap, no row placeholder offset needed
                        s.order.insert(insert_at, app_idx);
                        while let Some(child) = s.row.first_child() {
                            s.row.remove(&child);
                        }
                        for &idx in &s.order.clone() {
                            s.row.append(&s.wrappers[idx]);
                        }
                    } else {
                        // re-insert at same spot (row currently missing one)
                        let w = &s.wrappers[drag.src_index];
                        // find insert position in compact row
                        let insert_at = new_pos.min(s.row.observe_children().n_items() as usize);
                        if insert_at == 0 {
                            s.row.prepend(w);
                        } else if let Some(sib) = s
                            .row
                            .observe_children()
                            .item((insert_at - 1) as u32)
                            .and_then(|o| o.downcast::<gtk::Widget>().ok())
                        {
                            s.row.insert_child_after(w, Some(&sib));
                        } else {
                            s.row.append(w);
                        }
                    }
                    if let Some(win) = s
                        .overlay
                        .root()
                        .and_then(|r| r.downcast::<gtk::Window>().ok())
                    {
                        let scale = win.scale_factor();
                        let win_w = panel_width() + GHOST_SIZE;
                        let win_h = PANEL_HEIGHT + GHOST_SIZE + BOTTOM_GAP;
                        let panel_x = (win_w - panel_width()) / 2;
                        let panel_y = win_h - PANEL_HEIGHT - BOTTOM_GAP;
                        if let Some(surface) = win.surface() {
                            if let Some(xid) = x11_place::xid_of(&surface) {
                                x11_place::set_input_region(
                                    xid,
                                    panel_x * scale,
                                    panel_y * scale,
                                    (panel_width() * scale) as u16,
                                    (PANEL_HEIGHT * scale) as u16,
                                );
                            }
                            let region = gtk::cairo::Region::create_rectangle(
                                &gtk::cairo::RectangleInt::new(
                                    panel_x, panel_y, panel_width(), PANEL_HEIGHT,
                                ),
                            );
                            surface.set_input_region(&region);
                        }
                    }
                    s.suppress_click = true;
                    let state_delayed = state_released.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(50),
                        move || {
                            state_delayed.borrow_mut().suppress_click = false;
                        },
                    );
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    return;
                }
                let was_pending = if let Some(id) = timeout_id_released.borrow_mut().take() {
                    id.remove();
                    true
                } else {
                    false
                };
                if was_pending {
                    {
                        let mut s = state_released.borrow_mut();
                        if s.ghost.is_visible() {
                            s.ghost.set_visible(false);
                            while let Some(child) = s.ghost.first_child() {
                                s.ghost.remove(&child);
                            }
                        }
                        if s.suppress_click {
                            s.suppress_click = false;
                            return;
                        }
                    }
                    if matches!(action_click, TileAction::Launchpad) {
                        launchpad::show_launchpad(all_apps_click.clone());
                        return;
                    }
                    // Bounce on click for every app (2-3 bounces)
                    let do_bounce = |t: &gtk::Button| {
                        bounce_tile_dynamics(t);
                    };
                    match &action_click {
                        TileAction::Launchpad => unreachable!(),
                        TileAction::Real { item } => {
                            // Real installed program: bounce, then toggle.
                            // Visible windows minimize, minimized ones
                            // restore, closed apps launch out-of-process.
                            do_bounce(&tile_click);
                            toggle_app_windows(item);
                        }
                        TileAction::Demo { name, cmd } => {
                            // Demo fallback entries keep the legacy behavior.
                            // Sliders keeps its special launch loop; others just bounce + launch
                            if name == "Sliders" {
                        if *launching_click.borrow() {
                            do_bounce(&tile_click);
                            return;
                        }
                        let already_open = x11_place::is_app_running()
                            || x11_place::has_window_with_title("Sliders")
                            || x11_place::has_window_with_title("VLC")
                            || x11_place::has_window_with_title("vlc");
                        if already_open {
                            do_bounce(&tile_click);
                            return;
                        }
                        do_bounce(&tile_click);
                        *launching_click.borrow_mut() = true;
                        *cnt_click.borrow_mut() = 0;
                        let start_cnt = x11_place::client_count().unwrap_or(0);
                        glib::timeout_add_local_once(
                            std::time::Duration::from_millis(300),
                            {
                                let launching_c = launching_click.clone();
                                move || {
                                    let vlc_ok =
                                        std::process::Command::new("vlc").spawn().is_ok();
                                    if !vlc_ok {
                                        let _ = std::process::Command::new("wsl")
                                            .args([
                                                "-d",
                                                "archlinux",
                                                "--",
                                                "bash",
                                                "-c",
                                                "cd /mnt/c/Users/arlo1/Documents/TontooLibs/TontooUI && cargo run --example sliders",
                                            ])
                                            .spawn();
                                    }
                                    let _ = launching_c;
                                }
                            },
                        );
                        let tile_loop = tile_click.clone();
                        let launching_loop = launching_click.clone();
                        let cnt_loop = cnt_click.clone();
                        let start_cnt_loop = start_cnt;
                        glib::timeout_add_local(
                            std::time::Duration::from_millis(1400),
                            move || {
                                let open = x11_place::is_app_running()
                                    || x11_place::has_window_with_title("Sliders")
                                    || x11_place::has_window_with_title("VLC")
                                    || x11_place::has_window_with_title("vlc")
                                    || x11_place::client_count()
                                        .map(|c| c > start_cnt_loop)
                                        .unwrap_or(false);
                                if open {
                                    *launching_loop.borrow_mut() = false;
                                    return glib::ControlFlow::Break;
                                }
                                let mut c = cnt_loop.borrow_mut();
                                *c += 1;
                                if *c > 20 {
                                    *launching_loop.borrow_mut() = false;
                                    return glib::ControlFlow::Break;
                                }
                                bounce_tile_dynamics(&tile_loop);
                                glib::ControlFlow::Continue
                            },
                        );
                        return;
                    }
                    // Generic demo apps: bounce then spawn
                            do_bounce(&tile_click);
                            match cmd {
                                Some(exe) => match std::process::Command::new(exe).spawn() {
                                    Ok(_) => println!("[dock] launched {exe} ({})", tr(&lang_click, &format!("dock.app.{}", name.to_lowercase()), name)),
                                    Err(e) => eprintln!("[dock] spawn {exe} failed: {e}"),
                                },
                                None => println!("[dock] launch request: {} ({})", name, tr(&lang_click, &format!("dock.app.{}", name.to_lowercase()), name)),
                            }
                        }
                    }
                }
            });
            tile.add_controller(press_gesture);
        }
        }

        build_row(
            &self.data.borrow(),
            &state,
            &sliders_launching,
            &sliders_bounce_count,
        );

        // Fill row in initial order
        {
            let s = state.borrow();
            for &idx in &s.order {
                row.append(&s.wrappers[idx]);
            }
        }

        // Tile keys for the dots refresh, shared so pin rebuilds stay exact.
        // Tile 0 is the LaunchPad opener and never shows a dot.
        let tile_keys: Rc<RefCell<Vec<Option<apps::AppItem>>>> = Rc::new(RefCell::new(
            tile_keys_for(&self.data.borrow().dock_apps),
        ));

        // Running dots follow the open-program list (CoreWindows "list opened
        // programs"): re-query every 2 s and toggle the dot per tile.
        {
            let state_refresh = state.clone();
            let keys_refresh = tile_keys.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(2000), move || {
                let snapshot = apps::open_snapshot();
                let s = state_refresh.borrow();
                let keys = keys_refresh.borrow();
                for (dot, key) in s.dots.iter().zip(keys.iter()) {
                    let running = key
                        .as_ref()
                        .map(|item| apps::is_running(item, &snapshot))
                        .unwrap_or(false);
                    if running {
                        dot.remove_css_class("dock-dot-hidden");
                    } else {
                        dot.add_css_class("dock-dot-hidden");
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        // Live pins: rebuild the row when pins change (LaunchPad context
        // menu), without restarting the dock. New pins land far right.
        {
            let data_watch = self.data.clone();
            let state_watch = state.clone();
            let keys_watch = tile_keys.clone();
            let sliders_l = sliders_launching.clone();
            let sliders_n = sliders_bounce_count.clone();
            let mut last_version = crate::pins::version();
            glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                let current = crate::pins::version();
                if current == last_version {
                    return glib::ControlFlow::Continue;
                }
                // Defer while a drag is in flight; the version stays new.
                if state_watch.borrow().dragging.is_some() {
                    return glib::ControlFlow::Continue;
                }
                last_version = current;
                // Reload pins + icons + dots.
                let loaded = apps::load();
                let appearance = data_watch.borrow().appearance;
                set_tile_count(loaded.dock.len() + 1);
                let icons = generate_temp_icons(appearance, &loaded.dock);
                let running = apps::open_snapshot();
                let separator_after = if loaded.demo_mode { Some(4) } else { None };
                {
                    let mut data = data_watch.borrow_mut();
                    data.dock_apps = loaded.dock;
                    data.all_apps = loaded.all;
                    data.icons = icons;
                    data.running = running;
                    data.separator_after = separator_after;
                }
                // Clear row + drag artifacts, reset state vectors.
                {
                    let mut s = state_watch.borrow_mut();
                    while let Some(child) = s.row.first_child() {
                        s.row.remove(&child);
                    }
                    if s.placeholder.parent().is_some() {
                        s.row.remove(&s.placeholder);
                    }
                    s.wrappers.clear();
                    s.buttons.clear();
                    s.dots.clear();
                    s.order = (0..data_watch.borrow().dock_apps.len() + 1).collect();
                    s.dragging = None;
                    s.suppress_click = false;
                    s.ghost.set_visible(false);
                    while let Some(child) = s.ghost.first_child() {
                        s.ghost.remove(&child);
                    }
                    s.placeholder.set_visible(false);
                    s.placeholder_above.set_visible(false);
                }
                // Rebuild tiles + refill + fix shared dots keys.
                build_row(&data_watch.borrow(), &state_watch, &sliders_l, &sliders_n);
                *keys_watch.borrow_mut() = tile_keys_for(&data_watch.borrow().dock_apps);
                {
                    let s = state_watch.borrow();
                    for &idx in &s.order {
                        s.row.append(&s.wrappers[idx]);
                    }
                }
                // Panel size may have changed: re-place + fix input region.
                if let Some(win) = state_watch
                    .borrow()
                    .overlay
                    .root()
                    .and_then(|root| root.downcast::<gtk::Window>().ok())
                {
                    place_dock(&win);
                }
                glib::ControlFlow::Continue
            });
        }

        glass.append(&left_spacer);
        glass.append(&row);
        glass.append(&right_spacer);
        overlay.add_overlay(&glass);
        top_overlay.add_overlay(&overlay);
        // correct stacking: dock at bottom, then placeholder above dock, then ghost on very top
        top_overlay.add_overlay(&placeholder_above);
        top_overlay.add_overlay(&ghost);

        // Follow cursor while dragging – gap animation INSIDE dock + placeholder ABOVE dock
        let motion2 = gtk::EventControllerMotion::new();
        let state_motion = state.clone();
        motion2.connect_motion(move |_, x, y| {
            let mut s = state_motion.borrow_mut();
            let dragging = match &s.dragging {
                Some(d) => d.clone(),
                None => return,
            };
            s.ghost.set_margin_start((x as i32) - GHOST_SIZE / 2);
            s.ghost.set_margin_top((y as i32) - GHOST_SIZE / 2);

            let ghost_cx = x as i32;
            let (row_x_f, _) = s
                .row
                .translate_coordinates(&s.top_overlay, 0.0, 0.0)
                .unwrap_or((0.0, 0.0));
            let row_x = row_x_f as i32;
            let slot_w = ICON_SIZE + ICON_GAP;
            let n = s.order.len() as i32;
            let raw_gap = ((ghost_cx - row_x + slot_w / 2) / slot_w).clamp(0, n) as usize;
            let new_gap = raw_gap;
            // cur gap inside row (for icon shift animation)
            let cur_gap_row = (0..s.row.observe_children().n_items())
                .find_map(|i| {
                    s.row
                        .observe_children()
                        .item(i)
                        .and_then(|o| o.downcast::<gtk::Widget>().ok())
                        .filter(|w| w == &s.placeholder)
                        .map(|_| i as usize)
                })
                .unwrap_or(dragging.src_pos);
            if new_gap != cur_gap_row {
                let slot = ICON_SIZE + ICON_GAP;
                let mut affected: Vec<gtk::Widget> = Vec::new();
                let n_row = s.row.observe_children().n_items() as i32;
                if new_gap > cur_gap_row {
                    for i in (cur_gap_row + 1)..=new_gap {
                        if i < n_row as usize {
                            if let Some(w) = s.row.observe_children().item(i as u32).and_then(|o| o.downcast::<gtk::Widget>().ok()) {
                                if w != s.placeholder {
                                    affected.push(w);
                                }
                            }
                        }
                    }
                } else {
                    for i in new_gap..cur_gap_row {
                        if let Some(w) = s.row.observe_children().item(i as u32).and_then(|o| o.downcast::<gtk::Widget>().ok()) {
                            if w != s.placeholder {
                                affected.push(w);
                            }
                        }
                    }
                }
                let animate = |widget: &gtk::Widget, delta: i32| {
                    let p0 = gtk::CssProvider::new();
                    p0.load_from_string(&format!("* {{ transform: translate({}px,0); }}", delta));
                    widget.style_context().add_provider(&p0, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 5);
                    let w = widget.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(10), move || {
                        w.style_context().remove_provider(&p0);
                        let p1 = gtk::CssProvider::new();
                        p1.load_from_string("* { transform: translate(0px,0); transition: transform 220ms cubic-bezier(0.2,0,0,1); }");
                        w.style_context().add_provider(&p1, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 5);
                        let w2 = w.clone();
                        glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                            w2.style_context().remove_provider(&p1);
                        });
                    });
                };
                let delta_icon = if new_gap > cur_gap_row { slot } else { -slot };
                for w in &affected {
                    animate(w, delta_icon);
                }
                if new_gap == 0 {
                    s.row.reorder_child_after(&s.placeholder, None::<&gtk::Widget>);
                } else if let Some(sib) = s.row.observe_children().item((new_gap - 1) as u32).and_then(|o| o.downcast::<gtk::Widget>().ok()) {
                    s.row.reorder_child_after(&s.placeholder, Some(&sib));
                } else {
                    s.row.reorder_child_after(&s.placeholder, None::<&gtk::Widget>);
                }
            }
            // also move placeholder ABOVE dock to same gap
            let cur_gap_above = ((s.placeholder_above.margin_start() - row_x).div_euclid(slot_w)).clamp(0, n) as usize;
            if new_gap != cur_gap_above {
                s.placeholder_above.set_margin_start(row_x + (new_gap as i32) * slot_w);
                s.placeholder_above.set_margin_top(GHOST_SIZE - ICON_SIZE - 10);
            }
        });
        top_overlay.add_controller(motion2);

        let drop_gesture = gtk::GestureClick::new();
        drop_gesture.set_button(1);
        drop_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let state_drop = state.clone();
        drop_gesture.connect_released(move |gesture, n_press, _x, _y| {
            if n_press != 1 {
                return;
            }
            if state_drop.borrow().dragging.is_none() {
                return;
            }
            let mut s = state_drop.borrow_mut();
            let drag = match s.dragging.take() {
                Some(d) => d,
                None => return,
            };
            s.ghost.set_visible(false);
            while let Some(child) = s.ghost.first_child() {
                s.ghost.remove(&child);
            }
            s.buttons[drag.src_index].set_opacity(1.0);
            let (row_x_f, _) = s.row.translate_coordinates(&s.top_overlay, 0.0, 0.0).unwrap_or((0.0, 0.0));
            let row_x = row_x_f as i32;
            let slot_w = ICON_SIZE + ICON_GAP;
            let n = s.order.len() as i32;
            let new_pos = ((s.placeholder_above.margin_start() - row_x).div_euclid(slot_w)).clamp(0, n) as usize;
            s.placeholder_above.set_visible(false);
            s.placeholder.set_visible(false);
            let old_pos = drag.src_pos;
            if new_pos != old_pos {
                let app_idx = s.order.remove(old_pos);
                let insert_at = new_pos.min(s.order.len());
                s.order.insert(insert_at, app_idx);
                while let Some(child) = s.row.first_child() {
                    s.row.remove(&child);
                }
                for &idx in &s.order.clone() {
                    s.row.append(&s.wrappers[idx]);
                }
            } else {
                let w = &s.wrappers[drag.src_index].clone();
                let insert_at = new_pos.min(s.row.observe_children().n_items() as usize);
                if insert_at == 0 {
                    s.row.prepend(w);
                } else if let Some(sib) = s
                    .row
                    .observe_children()
                    .item((insert_at - 1) as u32)
                    .and_then(|o| o.downcast::<gtk::Widget>().ok())
                {
                    s.row.insert_child_after(w, Some(&sib));
                } else {
                    s.row.append(w);
                }
            }
            if let Some(win) = s
                .overlay
                .root()
                .and_then(|r| r.downcast::<gtk::Window>().ok())
            {
                let scale = win.scale_factor();
                let win_w = panel_width() + GHOST_SIZE;
                let win_h = PANEL_HEIGHT + GHOST_SIZE + BOTTOM_GAP;
                let panel_x = (win_w - panel_width()) / 2;
                let panel_y = win_h - PANEL_HEIGHT - BOTTOM_GAP;
                if let Some(surface) = win.surface() {
                    if let Some(xid) = x11_place::xid_of(&surface) {
                        x11_place::set_input_region(
                            xid,
                            panel_x * scale,
                            panel_y * scale,
                            (panel_width() * scale) as u16,
                            (PANEL_HEIGHT * scale) as u16,
                        );
                    }
                    let region = gtk::cairo::Region::create_rectangle(
                        &gtk::cairo::RectangleInt::new(
                            panel_x, panel_y, panel_width(), PANEL_HEIGHT,
                        ),
                    );
                    surface.set_input_region(&region);
                }
            }
            s.suppress_click = true;
            let state_delayed = state_drop.clone();
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(50),
                move || {
                    state_delayed.borrow_mut().suppress_click = false;
                },
            );
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        top_overlay.add_controller(drop_gesture);

        top_overlay.upcast()
    }

    fn size_that_fits(&self, _available: Size) -> Size {
        Size::new(panel_width() as f32, PANEL_HEIGHT as f32)
    }
}

struct DockRoot(DockPanelView);

impl UIKit::widget::Widget for DockRoot {
    fn id(&self) -> UIKit::widget::WidgetId {
        0
    }
    fn to_gtk(&self) -> gtk::Widget {
        self.0.render(Rect::ZERO)
    }
}

// ── Window setup ───────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
pub mod x11_place {
    use std::sync::OnceLock;
    use x11rb::connection::Connection;

    type Conn = x11rb::rust_connection::RustConnection;

    fn connection() -> Option<&'static (Conn, usize)> {
        static CONN: OnceLock<Option<(Conn, usize)>> = OnceLock::new();
        CONN.get_or_init(|| x11rb::connect(None).ok()).as_ref()
    }

    pub fn xid_of(surface: &gtk::gdk::Surface) -> Option<u32> {
        use gtk::prelude::*;
        // Only X11 surfaces have an XID. On the Wayland backend
        // gdk_x11_surface_get_xid would return garbage, so report None
        // and let callers skip the move (compositor default spot).
        let backend_is_x11 = gtk::gdk::Display::default()
            .map(|d| d.type_().name().to_string().contains("X11"))
            .unwrap_or(false);
        if !backend_is_x11 {
            return None;
        }
        extern "C" {
            fn gdk_x11_surface_get_xid(surface: *const std::ffi::c_void) -> u32;
        }
        use glib::translate::{Stash, ToGlibPtr};
        let ptr: Stash<'_, *const gtk::gdk::ffi::GdkSurface, _> = surface.to_glib_none();
        Some(unsafe { gdk_x11_surface_get_xid(ptr.0.cast()) })
    }

    pub fn move_window(xid: u32, x: i32, y: i32) -> bool {
        use x11rb::protocol::xproto::ConfigureWindowAux;
        use x11rb::protocol::xproto::ConnectionExt as _;
        let Some((conn, _)) = connection() else {
            return false;
        };
        let r = conn.configure_window(xid, &ConfigureWindowAux::new().x(x).y(y));
        let _ = conn.flush();
        r.is_ok()
    }

    #[allow(dead_code)]
    pub fn geometry(xid: u32) -> Option<(i32, i32, u32, u32)> {
        use x11rb::protocol::xproto::ConnectionExt as _;
        let (conn, _) = connection()?;
        let reply = conn.get_geometry(xid).ok()?.reply().ok()?;
        Some((
            reply.x as i32,
            reply.y as i32,
            reply.width as u32,
            reply.height as u32,
        ))
    }

    #[allow(dead_code)]
    pub fn screen_size() -> Option<(i32, i32)> {
        let (conn, screen) = connection()?;
        let root = &conn.setup().roots[*screen];
        Some((root.width_in_pixels as i32, root.height_in_pixels as i32))
    }

    pub fn primary_monitor() -> Option<(i32, i32, i32, i32)> {
        use x11rb::protocol::randr::ConnectionExt as _;
        let (conn, screen) = connection()?;
        let root = conn.setup().roots[*screen].root;
        let primary = conn
            .randr_get_output_primary(root)
            .ok()?
            .reply()
            .ok()?
            .output;
        let info = conn
            .randr_get_output_info(primary, 0)
            .ok()?
            .reply()
            .ok()?;
        let crtc = conn
            .randr_get_crtc_info(info.crtc, 0)
            .ok()?
            .reply()
            .ok()?;
        Some((
            crtc.x as i32,
            crtc.y as i32,
            crtc.width as i32,
            crtc.height as i32,
        ))
    }

    pub fn set_position_hints(xid: u32, x: i32, y: i32) -> bool {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
        let Some((conn, _screen)) = connection() else {
            return false;
        };
        const USPOSITION: i32 = 1 << 0;
        const PPOSITION: i32 = 1 << 1;
        let mut hints = match conn
            .get_property(
                false,
                xid,
                AtomEnum::WM_NORMAL_HINTS,
                AtomEnum::WM_SIZE_HINTS,
                0,
                18,
            )
            .ok()
            .and_then(|r| r.reply().ok())
        {
            Some(reply) if reply.value_len > 0 => {
                let mut v = [0i32; 18];
                for (i, chunk) in reply.value.chunks(4).take(18).enumerate() {
                    if chunk.len() == 4 {
                        v[i] = i32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    }
                }
                v
            }
            _ => [0i32; 18],
        };
        hints[0] |= USPOSITION | PPOSITION;
        hints[2] = x;
        hints[3] = y;
        let mut value = Vec::with_capacity(72);
        for v in hints {
            value.extend_from_slice(&v.to_ne_bytes());
        }
        let r = conn.change_property(
            PropMode::REPLACE,
            xid,
            AtomEnum::WM_NORMAL_HINTS,
            AtomEnum::WM_SIZE_HINTS,
            32,
            18,
            &value,
        );
        let _ = conn.flush();
        r.is_ok()
    }

    pub fn set_input_region(xid: u32, x: i32, y: i32, w: u16, h: u16) -> bool {
        use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
        use x11rb::protocol::xproto;
        let Some((conn, _)) = connection() else {
            return false;
        };
        let rect = xproto::Rectangle {
            x: x as i16,
            y: y as i16,
            width: w,
            height: h,
        };
        let r = conn.shape_rectangles(
            SO::SET,
            SK::INPUT,
            xproto::ClipOrdering::UNSORTED,
            xid,
            0,
            0,
            std::slice::from_ref(&rect),
        );
        let _ = conn.flush();
        r.is_ok()
    }

    pub fn raise_window(xid: u32) -> bool {
        use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt as _, StackMode};
        let Some((conn, _)) = connection() else {
            return false;
        };
        let r = conn.configure_window(
            xid,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        );
        let _ = conn.flush();
        r.is_ok()
    }

    pub fn set_dock_type(xid: u32) -> bool {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
        let Some((conn, _)) = connection() else {
            return false;
        };
        let type_atom = match conn
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE")
            .ok()
            .and_then(|c| c.reply().ok())
        {
            Some(r) => r.atom,
            None => return false,
        };
        let dock_atom = match conn
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE_DOCK")
            .ok()
            .and_then(|c| c.reply().ok())
        {
            Some(r) => r.atom,
            None => return false,
        };
        let data = dock_atom.to_ne_bytes();
        let frame_atom = conn
            .intern_atom(false, b"_GTK_FRAME_EXTENTS")
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.atom);
        if let Some(atom) = frame_atom {
            let extents = [0u32, 0, 0, 0];
            let mut bytes = Vec::with_capacity(16);
            for v in extents {
                bytes.extend_from_slice(&v.to_ne_bytes());
            }
            let _ = conn.change_property(
                PropMode::REPLACE,
                xid,
                atom,
                AtomEnum::CARDINAL,
                32,
                4,
                &bytes,
            );
        }
        let r = conn.change_property(
            PropMode::REPLACE,
            xid,
            type_atom,
            AtomEnum::ATOM,
            32,
            1,
            &data,
        );
        let _ = conn.flush();
        r.is_ok()
    }

    pub fn has_window_with_title(substr: &str) -> bool {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
        let Some((conn, screen)) = connection() else {
            return false;
        };
        let client_list_atom = match conn
            .intern_atom(false, b"_NET_CLIENT_LIST")
            .ok()
            .and_then(|c| c.reply().ok())
        {
            Some(r) => r.atom,
            None => return false,
        };
        let net_wm_name_atom = match conn
            .intern_atom(false, b"_NET_WM_NAME")
            .ok()
            .and_then(|c| c.reply().ok())
        {
            Some(r) => r.atom,
            None => return false,
        };
        let utf8_atom = match conn
            .intern_atom(false, b"UTF8_STRING")
            .ok()
            .and_then(|c| c.reply().ok())
        {
            Some(r) => r.atom,
            None => return false,
        };
        let root = conn.setup().roots[*screen].root;
        let list = match conn
            .get_property(false, root, client_list_atom, AtomEnum::WINDOW, 0, 4096)
            .ok()
            .and_then(|c| c.reply().ok())
        {
            Some(r) => r,
            None => return false,
        };
        if list.value_len == 0 {
            return false;
        }
        for chunk in list.value.chunks_exact(4) {
            let xid = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            if let Ok(cookie) =
                conn.get_property(false, xid, net_wm_name_atom, utf8_atom, 0, 1024)
            {
                if let Ok(reply) = cookie.reply() {
                    if reply.value_len > 0 {
                        let title = String::from_utf8_lossy(&reply.value);
                        if title.contains(substr) {
                            return true;
                        }
                    }
                }
            }
            if let Ok(cookie) =
                conn.get_property(false, xid, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
            {
                if let Ok(reply) = cookie.reply() {
                    if reply.value_len > 0 {
                        let title = String::from_utf8_lossy(&reply.value);
                        if title.contains(substr) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    pub fn client_count() -> Option<usize> {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
        let Some((conn, screen)) = connection() else {
            return None;
        };
        let client_list_atom = conn
            .intern_atom(false, b"_NET_CLIENT_LIST")
            .ok()?
            .reply()
            .ok()?
            .atom;
        let root = conn.setup().roots[*screen].root;
        let list = conn
            .get_property(false, root, client_list_atom, AtomEnum::WINDOW, 0, 4096)
            .ok()?
            .reply()
            .ok()?;
        Some(list.value_len as usize)
    }

    pub fn is_app_running() -> bool {
        if let Ok(out) = std::process::Command::new("pgrep")
            .args(["-x", "vlc"])
            .output()
        {
            if out.status.success() && !out.stdout.is_empty() {
                return true;
            }
        }
        if let Ok(out) = std::process::Command::new("pgrep")
            .args(["-f", "sliders"])
            .output()
        {
            if out.status.success() && !out.stdout.is_empty() {
                return true;
            }
        }
        false
    }
}

#[cfg(not(target_os = "linux"))]
pub mod x11_place {
    pub fn xid_of(_surface: &gtk::gdk::Surface) -> Option<u32> {
        None
    }
    pub fn move_window(_xid: u32, _x: i32, _y: i32) -> bool {
        false
    }
    pub fn screen_size() -> Option<(i32, i32)> {
        None
    }
    pub fn primary_monitor() -> Option<(i32, i32, i32, i32)> {
        None
    }
    pub fn set_position_hints(_xid: u32, _x: i32, _y: i32) -> bool {
        false
    }
    pub fn geometry(_xid: u32) -> Option<(i32, i32, u32, u32)> {
        None
    }
    pub fn set_input_region(_xid: u32, _x: i32, _y: i32, _w: u16, _h: u16) -> bool {
        false
    }
    pub fn has_window_with_title(_substr: &str) -> bool {
        false
    }
    pub fn client_count() -> Option<usize> {
        None
    }
    pub fn is_app_running() -> bool {
        false
    }
}

fn install_transparent_background(display: &gtk::gdk::Display) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "window { background-color: transparent; }
        overlay { background: transparent; }
        scrolledwindow { background: transparent; }
        viewport { background: transparent; }
        separator { background: transparent; opacity: 0; }
        .dock-bg { background: transparent; }",
    );
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 10,
    );
}

fn find_panel(root: &gtk::Widget) -> Option<gtk::Box> {
    if let Ok(box_) = root.clone().downcast::<gtk::Box>() {
        if box_.has_css_class("dock-panel") {
            return Some(box_);
        }
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_panel(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

fn primary_monitor_rect(win: &gtk::Window) -> (i32, i32, i32, i32) {
    if let Some(rect) = x11_place::primary_monitor() {
        return rect;
    }
    let scale = win.scale_factor().max(1);
    let display = gtk::prelude::WidgetExt::display(win);
    display
        .monitors()
        .item(0)
        .and_then(|o| o.downcast::<gtk::gdk::Monitor>().ok())
        .map(|m| {
            let g = m.geometry();
            (g.x() * scale, g.y() * scale, g.width() * scale, g.height() * scale)
        })
        .unwrap_or((0, 0, 1920, 1080))
}

fn place_dock(win: &gtk::Window) {
    win.set_resizable(false);
    win.set_decorated(false);
    if let Some(display) = gtk::gdk::Display::default() {
        println!("[dock] display backend: {}", display.type_().name());
    }
    let scale = win.scale_factor().max(1);
    let (mx, my, mw, mh) = primary_monitor_rect(win);

    // Window includes panel + ghost overflow + bottom gap (logical)
    let win_w: i32 = panel_width() + GHOST_SIZE;
    let win_h: i32 = PANEL_HEIGHT + GHOST_SIZE + BOTTOM_GAP;
    let px = mx + (mw - win_w * scale) / 2;
    let py = my + mh - win_h * scale;
    win.set_size_request(win_w, win_h);
    win.set_default_size(win_w, win_h);
    install_transparent_background(&gtk::prelude::WidgetExt::display(win));

    if let Some(surface) = win.surface() {
        if let Some(xid) = x11_place::xid_of(&surface) {
            x11_place::set_position_hints(xid, px, py);
            let _ = x11_place::move_window(xid, px, py);
            // Input region: only the glass panel (physical pixels) – click-through outside
            let panel_x = (win_w - panel_width()) / 2;
            let panel_y = win_h - PANEL_HEIGHT - BOTTOM_GAP;
            let _ = x11_place::set_input_region(
                xid,
                panel_x * scale,
                panel_y * scale,
                (panel_width() * scale) as u16,
                (PANEL_HEIGHT * scale) as u16,
            );
            let _ = x11_place::set_dock_type(xid);
            let _ = x11_place::raise_window(xid);
        }
        // GDK input region is logical
        let panel_x = (win_w - panel_width()) / 2;
        let panel_y = win_h - PANEL_HEIGHT - BOTTOM_GAP;
        let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(
            panel_x, panel_y, panel_width(), PANEL_HEIGHT,
        ));
        surface.set_input_region(&region);
    }
}

fn main() {
    println!(
        "TontooOS Dock v{}.{}, TontooUIKit v{}.{}.{}",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        UITKIT_VERSION.0,
        UITKIT_VERSION.1,
        UITKIT_VERSION.2
    );

    if std::env::var("GDK_BACKEND").is_err() {
        std::env::set_var("GDK_BACKEND", "x11");
    }

    let lang = load_lang();
    // Resolve appearance FIRST: icons are cached per appearance
    // (`tontoo-dock-<app>-<light|dark>-vN.png`), so they must be
    // generated with the active mode. Generating before resolving
    // would reuse the wrong variant after a light/dark switch.
    let appearance = Appearance::resolve();
    // Real programs via CoreWindows: first N pinned in the dock, everything
    // in the LaunchPad. Empty install -> demo fallback tiles.
    let loaded = apps::load();
    if std::env::var("DOCK_DEBUG").is_ok() {
        println!(
            "[dock] programs: {} installed ({} pinned){}",
            loaded.all.len(),
            loaded.dock.len(),
            if loaded.demo_mode { " [demo fallback]" } else { "" }
        );
    }
    set_tile_count(loaded.dock.len() + 1);
    let icons = generate_temp_icons(appearance, &loaded.dock);
    let running = apps::open_snapshot();
    // Prewarm Launchpad icon cache on the main loop (idle, one icon per tick)
    // so the first open is fast even after /tmp was cleared. Main thread only:
    // CoreIcon ASSETS_DIR is `static mut`, a background thread would race.
    glib::timeout_add_local_once(std::time::Duration::from_millis(2000), || {
        glib::timeout_add_local(std::time::Duration::from_millis(50), || {
            if launchpad::prewarm_step() {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    });
    let monitor = x11_place::primary_monitor().unwrap_or((0, 0, 1920, 1080));
    // Try wallpaper backdrop, fallback to gradient (never empty)
    let backdrop = {
        let variant = match appearance {
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        };
        let out = std::env::temp_dir()
            .join(format!("tontoo-dock-backdrop-{variant}-v11.png"))
            .into_os_string()
            .into_string()
            .expect("temp path");
        if let Some(wp) = backdrop::find_wallpaper() {
            match backdrop::generate(
                &wp,
                monitor,
                panel_width(),
                PANEL_HEIGHT,
                PANEL_RADIUS,
                BOTTOM_GAP,
                appearance,
                &out,
            ) {
                Some(p) => Some(p),
                None => {
                    eprintln!("[dock] backdrop generation failed for {wp} -> fallback");
                    backdrop::generate_fallback(panel_width(), PANEL_HEIGHT, PANEL_RADIUS, appearance, &out)
                }
            }
        } else {
            eprintln!("[dock] no wallpaper found -> fallback gradient");
            backdrop::generate_fallback(panel_width(), PANEL_HEIGHT, PANEL_RADIUS, appearance, &out)
                .or_else(|| {
                    eprintln!("[dock] fallback also failed, using CSS gradient only");
                    None
                })
        }
    };

    let mut app = App::new("Dock", 320, 240);
    let separator_after = if loaded.demo_mode { Some(4) } else { None };
    let data = Rc::new(RefCell::new(DockData {
        dock_apps: loaded.dock,
        all_apps: loaded.all,
        icons,
        running,
        separator_after,
        appearance,
        backdrop,
        lang,
    }));
    app.set_root(DockRoot(DockPanelView { data }));
    app.no_window_bar();
    app.no_window_frame();
    app.set_color_scheme(appearance.to_color_scheme());
    // Native tile tooltips (`set_tooltip_text`) use the GTK4 default hover
    // delay. NOTE: GTK3 `gtk-tooltip-timeout` / `gtk-tooltip-browse-timeout`
    // do NOT exist on GTK4 `GtkSettings` - setting them panics with
    // "property 'gtk-tooltip-timeout' of type 'GtkSettings' not found".
    // So no explicit timeout is pinned here.

    let mut ticks = 0u32;
    glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
        ticks += 1;
        if gtk::Window::list_toplevels().is_empty() {
            if ticks > 100 {
                eprintln!("[dock] window never appeared; keeping defaults");
                return glib::ControlFlow::Break;
            }
            return glib::ControlFlow::Continue;
        }
        for widget in gtk::Window::list_toplevels() {
            if let Ok(win) = widget.clone().downcast::<gtk::Window>() {
                place_dock(&win);

                if ticks == 1 && std::env::var("DOCK_DEBUG").is_ok() {
                    println!(
                        "[dock] covering primary monitor, panel bottom-center (gap={} scale-correct, click-through outside)",
                        BOTTOM_GAP
                    );
                }
                if ticks == 25 && std::env::var("DOCK_DEBUG").is_ok() {
                    let scale = win.scale_factor();
                    println!(
                        "[dock] debug alloc={}x{} scale={scale}",
                        win.width(),
                        win.height()
                    );
                    if let Some(panel) = find_panel(&widget) {
                        println!(
                            "[dock] panel alloc={}x{}",
                            panel.allocated_width(),
                            panel.allocated_height()
                        );
                    }
                }
            }
        }
        if ticks < 50 {
            return glib::ControlFlow::Continue;
        }
        glib::ControlFlow::Break
    });

    app.run();
}

#[cfg(test)]
mod icon_mode_tests {
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
        let img =
            build_launchpad_image(Appearance::Dark).expect("dark launchpad render");
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
        let img =
            build_launchpad_image(Appearance::Light).expect("light launchpad render");
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
}
