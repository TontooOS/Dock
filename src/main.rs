//! TontooOS Dock
//!
//! macOS Sonoma / Sequoia-accurate glass dock with running dots, bounce,
//! separator and drag reordering, drawn as a bottom-anchored TontooUI
//! layer surface. The LaunchPad grid opens as a second surface of the same
//! process; the panel itself never leaves Wayland.

sdk::preinclude!();

use crate::TontooUI::renderer::layershell::{run_layer_multi, LayerBarOptions};

mod apps;
mod backdrop;
mod icons;
mod i18n;
mod launchpad;
mod launcher;
mod panel;
mod pins;
mod process;
mod theme;

use panel::{DockData, Panel, SURFACE_HEIGHT, panel_width_for, surface_width_for};

/// Monitor size assumed for the baked panel backdrop when no output geometry
/// is known yet. The wallpaper sample only needs the right proportions.
const BACKDROP_MONITOR: (i32, i32, i32, i32) = (0, 0, 1920, 1080);
/// Cache version of the baked panel backdrop.
const BACKDROP_VERSION: u32 = 11;

fn main() {
    i18n::init_i18n();

    // Resolve the appearance FIRST: icons are cached per appearance
    // (`tontoo-dock-<app>-<light|dark>-vN.png`), so generating them before
    // the mode is known would reuse the wrong variant after a switch.
    let appearance = theme::Appearance::resolve();
    println!("TontooOS Dock v{}", env!("CARGO_PKG_VERSION"));

    // Real programs via CoreWindows: first N pinned in the dock, everything in
    // the LaunchPad. Empty install -> demo fallback tiles.
    let loaded = apps::load();
    if std::env::var("DOCK_DEBUG").is_ok() {
        println!(
            "[dock] programs: {} installed ({} pinned){}",
            loaded.all.len(),
            loaded.dock.len(),
            if loaded.demo_mode { " [demo fallback]" } else { "" }
        );
    }
    let tile_count = loaded.dock.len() + 1;
    let dock_icons = icons::dock_icons(appearance, &loaded.dock);
    let running = apps::open_snapshot();
    let separator_after = if loaded.demo_mode { Some(4) } else { None };
    let panel_w = panel_width_for(tile_count);
    let backdrop = bake_backdrop(appearance, panel_w);

    let data = DockData {
        dock_apps: loaded.dock,
        all_apps: loaded.all,
        icons: dock_icons,
        running,
        separator_after,
    };

    // One bottom bar per output. The vertical-only anchor makes the
    // compositor center it and keeps the whole output free for windows, so
    // maximized windows still reach the bottom edge.
    let options = LayerBarOptions::bottom("dock", SURFACE_HEIGHT)
        .with_width(surface_width_for(tile_count));
    if let Err(err) = run_layer_multi(move |output| {
        let app = Panel::new(
            DockData {
                dock_apps: data.dock_apps.clone(),
                all_apps: data.all_apps.clone(),
                icons: data.icons.clone(),
                running: apps::OpenSnapshot {
                    daemon_ok: data.running.daemon_ok,
                    windows: data.running.windows.clone(),
                },
                separator_after: data.separator_after,
            },
            appearance,
            backdrop.clone(),
        );
        let _ = output;
        Some(vec![(Box::new(app) as Box<dyn TontooUI::renderer::window::App>, options.clone())])
    }) {
        eprintln!("[dock] failed to start: {err}");
        std::process::exit(1);
    }
}

/// Bake the blurred panel backdrop once per appearance.
///
/// Samples the wallpaper behind the panel, blurs it at quarter resolution and
/// grades it; a guaranteed gradient stands in when no wallpaper is found, so
/// the panel never reads as black.
fn bake_backdrop(appearance: theme::Appearance, panel_w: f32) -> Option<String> {
    let variant = appearance.cache_suffix();
    let out = std::env::temp_dir()
        .join(format!("tontoo-dock-backdrop-{variant}-v{BACKDROP_VERSION}.png"))
        .into_os_string()
        .into_string()
        .expect("temp path");
    let panel_w = panel_w.round() as i32;
    let panel_h = panel::PANEL_HEIGHT.round() as i32;
    let radius = panel::PANEL_RADIUS.round() as i32;
    let gap = panel::BOTTOM_GAP.round() as i32;
    match backdrop::find_wallpaper() {
        Some(wallpaper) => match backdrop::generate(
            &wallpaper,
            BACKDROP_MONITOR,
            panel_w,
            panel_h,
            radius,
            gap,
            appearance,
            &out,
        ) {
            Some(path) => Some(path),
            None => {
                eprintln!("[dock] backdrop generation failed for {wallpaper} -> fallback");
                backdrop::generate_fallback(panel_w, panel_h, radius, appearance, &out)
            }
        },
        None => {
            eprintln!("[dock] no wallpaper found -> fallback gradient");
            backdrop::generate_fallback(panel_w, panel_h, radius, appearance, &out)
        }
    }
}