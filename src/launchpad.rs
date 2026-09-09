//! LaunchPad window - centered, ESC/click outside to close, search + AppStore icon + categories + grid, Light/Dark via UIKit

use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use crate::UIKit::app::ColorScheme;

use crate::CoreIcon::generator::*;
use crate::CoreIcon::octopus::OctopusVariant;
use crate::CoreIcon::{Color as IconColor, Gradient, GradientDirection, GradientStop, SFSymbol};

use crate::apps::{fallback_style, AppItem};

const ICON_CORNER_RADIUS: f32 = 250.0;
/// Cache version of the legacy demo tiles (`tontoo-launchpad-<name>-v2.png`).
const ICON_GEN_VERSION: u32 = 2;
/// Cache version of tiles generated for real programs
/// (`tontoo-launchpad-<bundle-id>-v3.png`).
const REAL_ICON_GEN_VERSION: u32 = 3;

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

/// NOTE: must NOT write `ASSETS_DIR` (`static mut` in CoreIcon).
/// The single write happens in `main.rs::generate_temp_icons()` before the
/// prewarm thread is spawned (happens-before). Any extra write from the
/// Launchpad async loader while the prewarm thread reads it is a data race
/// (UB) and crashes GTK callbacks randomly. So this is intentionally a no-op.
fn ensure_assets_dir() {}

fn icon_cache_path(name: &str) -> Option<String> {
    std::env::temp_dir()
        .join(format!(
            "tontoo-launchpad-{}-v{}.png",
            name.to_lowercase().replace(' ', "-"),
            ICON_GEN_VERSION
        ))
        .into_os_string()
        .into_string()
        .ok()
}

/// Cache path for tiles generated for real programs (keyed by bundle id so
/// renames never collide and demo tiles are never reused).
fn real_icon_cache_path(bundle_id: &str) -> Option<String> {
    let safe: String = bundle_id
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    std::env::temp_dir()
        .join(format!(
            "tontoo-launchpad-{}-v{}.png",
            safe, REAL_ICON_GEN_VERSION
        ))
        .into_os_string()
        .into_string()
        .ok()
}

/// The list the prewarm step warms: all installed programs, or the demo
/// grid when nothing is installed. Loaded once, then served index by index.
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

/// Single-threaded prewarm step for the Dock main loop (NOT a background
/// thread: `CoreIcon::generator::ASSETS_DIR` is `static mut`, so any second
/// thread touching `IconCanvas` is a data race and aborts GTK randomly).
/// Call repeatedly from a low-priority idle/timeout source; returns `false`
/// when the cache is fully warm. Skips files that already exist.
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
        let _ = generate_tontoo_icon_cached(true);
    } else if i == 1 {
        let _ = generate_tontoo_icon_cached(false);
    } else if let Some(item) = list.get(i - 2) {
        // Bundle icons need no warming (loaded straight from the bundle).
        let has_bundle_icon = item
            .icon_path
            .as_deref()
            .map(|p| p.is_file())
            .unwrap_or(false);
        if !has_bundle_icon {
            if item.bundle_path.is_some() {
                let _ = generate_real_icon(item);
            } else {
                let (col, sym) = fallback_style(&item.display_name);
                let _ = generate_launchpad_icon(&item.display_name, col, sym);
            }
        }
    }
    true
}

/// Legacy bulk prewarm: main-thread only. Kept for tests/tools, NOT for
/// `std::thread::spawn`.
#[allow(dead_code)]
pub fn prewarm_cache() {
    while prewarm_step() {}
}

fn generate_launchpad_icon(name: &str, hex: &str, symbol: SFSymbol) -> Option<String> {
    let path = icon_cache_path(name)?;
    if std::path::Path::new(&path).is_file() {
        return Some(path);
    }
    render_tile(&path, name, hex, symbol)
}

/// Generated tile for a real program without a bundle icon.
/// Keyed by bundle id (never collides with demo tiles).
fn generate_real_icon(item: &AppItem) -> Option<String> {
    let path = real_icon_cache_path(&item.bundle_id)?;
    if std::path::Path::new(&path).is_file() {
        return Some(path);
    }
    render_tile(&path, &item.display_name, item.color, item.symbol)
}
/// Render one generated tile PNG to `path`.
fn render_tile(path: &str, name: &str, hex: &str, symbol: SFSymbol) -> Option<String> {
    let base = IconColor::from_hex(hex)?;
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
        Ok(()) => Some(path.to_owned()),
        Err(err) => {
            eprintln!("[launchpad] icon generation failed for {name}: {err}");
            None
        }
    }
}

fn generate_tontoo_icon_cached(is_light: bool) -> Option<String> {
    let variant = if is_light { OctopusVariant::Black } else { OctopusVariant::White };
    let out = std::env::temp_dir()
        .join(format!("tontoo-launchpad-tontoo-{}-36-v2.png", if is_light { "black" } else { "white" }))
        .to_string_lossy().to_string();
    if std::path::Path::new(&out).exists() {
        return Some(out);
    }
    let load_octopus_abs = |var: OctopusVariant| -> Option<image::RgbaImage> {
        let candidates = [
            format!("{}/../../TontooLibs/CoreIcon/assets/TontooOS/{}", env!("CARGO_MANIFEST_DIR"), var.file_name()),
            format!("/mnt/c/Users/arlo1/Documents/TontooLibs/CoreIcon/assets/TontooOS/{}", var.file_name()),
            format!("C:/Users/arlo1/Documents/TontooLibs/CoreIcon/assets/TontooOS/{}", var.file_name()),
            format!("/Library/System/CoreIcon/assets/TontooOS/{}", var.file_name()),
            var.path(),
        ];
        for p in candidates {
            if let Ok(img) = image::open(&p) {
                return Some(img.to_rgba8());
            }
        }
        crate::CoreIcon::octopus::OctopusIcon::new(var).load().ok()
    };
    if let Some(img) = load_octopus_abs(variant) {
        let thumb = image::imageops::resize(&img, 64, 64, image::imageops::FilterType::Lanczos3);
        if thumb.save(&out).is_ok() {
            return Some(out);
        }
    }
    let os_candidates = [
        format!("{}/../../TontooLibs/CoreIcon/assets/TontooOS/OSVersionAssets/26.1.0/TontooOS_Icon.png", env!("CARGO_MANIFEST_DIR")),
        "/mnt/c/Users/arlo1/Documents/TontooLibs/CoreIcon/assets/TontooOS/OSVersionAssets/26.1.0/TontooOS_Icon.png".to_string(),
        "C:/Users/arlo1/Documents/TontooLibs/CoreIcon/assets/TontooOS/OSVersionAssets/26.1.0/TontooOS_Icon.png".to_string(),
    ];
    for p in os_candidates {
        if let Ok(img) = image::open(&p) {
            let thumb = image::imageops::resize(&img.to_rgba8(), 64, 64, image::imageops::FilterType::Lanczos3);
            if thumb.save(&out).is_ok() {
                return Some(out);
            }
        }
    }
    if let Ok(img) = crate::CoreIcon::os_version::use_osversionicons("26.1.0", "TontooOS_Icon.png") {
        let thumb = image::imageops::resize(&img, 64, 64, image::imageops::FilterType::Lanczos3);
        if thumb.save(&out).is_ok() {
            return Some(out);
        }
    }
    let p = variant.path();
    if std::path::Path::new(&p).exists() {
        return Some(p);
    }
    eprintln!("[launchpad] tontoo icon not found for variant {:?}", variant);
    None
}

fn placeholder_tile(col: &str, name: &str) -> gtk::Box {
    let icon_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    icon_box.set_size_request(64, 64);
    icon_box.set_halign(gtk::Align::Center);
    icon_box.set_valign(gtk::Align::Center);
    let inner_css = format!(
        "box {{ background: {}; border-radius: 16px; }} label {{ color: white; font-weight: 800; font-size: 20px; }}",
        col
    );
    let p = gtk::CssProvider::new();
    p.load_from_string(&inner_css);
    icon_box.style_context().add_provider(
        &p,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let lbl = gtk::Label::new(Some(&name.chars().next().unwrap_or('A').to_string()));
    lbl.set_halign(gtk::Align::Center);
    lbl.set_valign(gtk::Align::Center);
    lbl.set_hexpand(true);
    lbl.set_vexpand(true);
    icon_box.append(&lbl);
    icon_box
}

/// Open the LaunchPad grid for `items` (all installed programs).
/// An empty list shows the demo grid instead.
pub fn show_launchpad(items: Vec<AppItem>) {
    let entries: Vec<AppItem> = if items.is_empty() {
        crate::apps::demo_launchpad()
    } else {
        items
    };
    let win = gtk::Window::new();
    win.set_title(Some("LaunchPad"));
    win.set_default_size(900, 600);
    win.set_resizable(false);
    win.set_decorated(false);
    win.set_modal(true);

    let scheme = ColorScheme::detect_system();
    let is_light = scheme == ColorScheme::Light;
    // Spec: Light = #ececec, Dark = #1d1d1d, jeweils 90% opak + Blur
    let bg_rgba = if is_light {
        "rgba(236,236,236,0.90)"
    } else {
        "rgba(29,29,29,0.90)"
    };
    let fg = if is_light { "#1d1d1f" } else { "#f5f5f7" };
    let border = if is_light {
        "rgba(0,0,0,0.08)"
    } else {
        "rgba(255,255,255,0.10)"
    };

    let css = format!(
        "window.launchpad-window {{ background-color: transparent; opacity: 1; }}
         window.launchpad-window overlay {{ background-color: transparent; opacity: 1; }}
         .launchpad-bg {{ background-color: {bg_rgba}; opacity: 1; border-radius: 16px; border: 1px solid {border}; box-shadow: 0 20px 60px rgba(0,0,0,0.35); backdrop-filter: blur(24px) saturate(180%); }}
         .app-tile, button.app-tile, flowboxchild, flowboxchild > * {{ background: transparent; background-color: transparent; border: none; box-shadow: none; outline: none; }}
         .app-tile:hover, .app-tile:active, .app-tile:focus, button.app-tile:hover, button.app-tile:active, button.app-tile:focus, flowboxchild:selected, flowboxchild:active, flowbox {{ background: transparent; background-color: transparent; border: none; box-shadow: none; outline: none; }}
         .app-tile label {{ font-family: 'SF Pro Display'; font-size: 11px; color: {fg}; }}
         flowbox, scrolledwindow, viewport {{ background: transparent; background-color: transparent; border: none; }}
         separator {{ background: {border}; min-height: 1px; }}",
         fg = fg
     );
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    // WICHTIG: kein Display-Provider! Sonst leaked CSS auf Dock-Fenster (macht es weiß).
    // Nur Window/Widget Provider damit Dock unbeeinflusst bleibt.
    win.add_css_class("launchpad-window");
    win.set_opacity(1.0);
    win.style_context()
        .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 110);
    // NOTE: no set_visible(true) here – window is presented once, after the
    // skeleton (placeholders) is built, so no empty transparent frame shows.

    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    overlay.add_css_class("launchpad-window");
    overlay
        .style_context()
        .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 110);

    // bg_box centered 760x520 inside larger window (900x600) -> transparent margin for click-outside
    let bg_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bg_box.add_css_class("launchpad-bg");
    bg_box.set_halign(gtk::Align::Center);
    bg_box.set_valign(gtk::Align::Center);
    bg_box.set_size_request(760, 520);
    bg_box.set_visible(true);
    bg_box.set_opacity(1.0);
    // ensure bg_box provider wins over global transparent
    bg_box
        .style_context()
        .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 110);
    overlay.set_child(Some(&bg_box));

    // content
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_hexpand(true);
    content.set_vexpand(true);
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);
    content.set_margin_end(16);
    bg_box.append(&content);

    // Drag-State: verhindert Schließen während Bridge-Drag (sonst focus-out → close)
    let is_dragging = Rc::new(RefCell::new(false));
    // Flow data early so TextInput on_change kann filtern (TontooUI highlight)
    let flow_children: Rc<RefCell<Vec<(String, gtk::FlowBoxChild)>>> =
        Rc::new(RefCell::new(Vec::new()));

    // top bar: AppStore icon links + TontooUI Default TextInput rechts
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    top.set_halign(gtk::Align::Fill);
    top.set_margin_bottom(12);

    // Tontoo icon top-left: instant fallback, real icon loads async (no UI block).
    let tontoo_btn = gtk::Button::new();
    tontoo_btn.set_size_request(36, 36);
    tontoo_btn.add_css_class("app-tile");
    tontoo_btn.set_has_frame(false);
    {
        let fallback = gtk::Image::from_icon_name("application-x-executable");
        fallback.set_pixel_size(24);
        tontoo_btn.set_child(Some(&fallback));
    }
    top.append(&tontoo_btn);
    {
        let btn_c = tontoo_btn.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(10), move || {
            if let Some(p) = generate_tontoo_icon_cached(is_light) {
                let img = gtk::Image::from_file(&p);
                img.set_pixel_size(24);
                img.add_css_class("tontoo-top-icon");
                btn_c.set_child(Some(&img));
            }
        });
    }

    // TontooUI TextInput – Default mit Highlighting / Focus-Ring / SF Pro
    // Nutzt TontooUI direkt (volle crate via sdk feature) für to_gtk()
    let search_widget = {
        use crate::UIKit::widget::Widget;
        crate::TontooUI::TextInput::new("Applications")
            .frame(320.0, 32.0)
            .to_gtk()
    };
    search_widget.set_hexpand(true);
    search_widget.set_halign(gtk::Align::Fill);
    // Entry für Filter-Signal extrahieren (Widget ist Entry)
    let search_entry = search_widget
        .clone()
        .downcast::<gtk::Entry>()
        .unwrap_or_else(|w| {
            // Fallback: suche Entry im Subtree
            fn find_entry(widget: &gtk::Widget) -> Option<gtk::Entry> {
                if let Ok(e) = widget.clone().downcast::<gtk::Entry>() {
                    return Some(e);
                }
                let mut child = widget.first_child();
                while let Some(c) = child {
                    if let Some(e) = find_entry(&c) {
                        return Some(e);
                    }
                    child = c.next_sibling();
                }
                None
            }
            find_entry(&w).expect("TextInput widget must contain gtk::Entry")
        });
    top.append(&search_widget);

    content.append(&top);

    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    content.append(&sep);

    // grid
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    let flow = gtk::FlowBox::new();
    flow.set_hexpand(true);
    flow.set_vexpand(true);
    flow.set_homogeneous(true);
    flow.set_max_children_per_line(6);
    flow.set_min_children_per_line(6);
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_row_spacing(16);
    flow.set_column_spacing(16);
    flow.set_margin_top(8);

    ensure_assets_dir();
    // Pending icons for async upgrade: (button, item). Placeholders are shown
    // instantly so the window is never an empty transparent frame.
    let pending: Rc<RefCell<Vec<(gtk::Button, AppItem)>>> = Rc::new(RefCell::new(Vec::new()));
    for item in entries.iter() {
        let name = item.display_name.as_str();
        let child = gtk::FlowBoxChild::new();
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
        vbox.set_halign(gtk::Align::Center);
        let btn = gtk::Button::new();
        btn.add_css_class("app-tile");
        btn.set_has_frame(false);
        btn.set_can_focus(false);
        btn.set_focus_on_click(false);
        btn.set_size_request(72, 72);
        // komplett transparent – kein Hintergrund bei Hover/Active/Focus
        let tp = gtk::CssProvider::new();
        tp.load_from_string("button.app-tile, button.app-tile:hover, button.app-tile:active, button.app-tile:focus { background: transparent; background-color: transparent; border: none; box-shadow: none; outline: none; }");
        btn.style_context().add_provider(&tp, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 40);
        // Fast path, in order:
        // 1. bundle icon file shipped by the program (used as-is),
        // 2. generated tile cache hit (legacy demo key or bundle-id key),
        // 3. letter placeholder + async generation in the idle loader below.
        let bundle_icon = item
            .icon_path
            .as_deref()
            .filter(|p| p.is_file())
            .and_then(|p| p.to_str().map(str::to_owned));
        let mut use_placeholder = true;
        if let Some(icon) = bundle_icon {
            let img = gtk::Image::from_file(&icon);
            img.set_pixel_size(64);
            btn.set_child(Some(&img));
            use_placeholder = false;
        } else {
            let cached = if item.bundle_path.is_some() {
                real_icon_cache_path(&item.bundle_id)
            } else {
                icon_cache_path(name)
            };
            if let Some(cached) = cached {
                if std::path::Path::new(&cached).is_file() {
                    let img = gtk::Image::from_file(&cached);
                    img.set_pixel_size(64);
                    btn.set_child(Some(&img));
                    use_placeholder = false;
                }
            }
        }
        if use_placeholder {
            btn.set_child(Some(&placeholder_tile(item.color, name)));
            pending.borrow_mut().push((btn.clone(), item.clone()));
        }
        let txt = gtk::Label::new(Some(name));
        txt.set_halign(gtk::Align::Center);
        txt.set_ellipsize(gtk::pango::EllipsizeMode::End);
        txt.set_max_width_chars(10);
        vbox.append(&btn);
        vbox.append(&txt);
        child.set_child(Some(&vbox));
        flow.append(&child);
        flow_children.borrow_mut().push((name.to_string(), child));
        // Click launches the program out-of-process (LaunchPad daemon temp
        // process, fallback detached tapp) and closes the grid macOS-style.
        // Demo entries without a bundle only log. Drag to dock is disabled
        // per user request – only the dock itself is reorderable.
        {
            let item_c = item.clone();
            let b = btn.clone();
            let win_c = win.clone();
            b.connect_clicked(move |_| {
                if let Some(bundle_path) = item_c.bundle_path.as_deref() {
                    crate::launcher::launch_app(bundle_path, &item_c.display_name);
                    win_c.close();
                } else {
                    println!("[launchpad] clicked {} (no drag)", item_c.display_name);
                }
            });
        }
    }

    scrolled.set_child(Some(&flow));
    content.append(&scrolled);

    // search filtering – TontooUI TextInput Entry
    {
        let flow_children_c = flow_children.clone();
        let flow_c = flow.clone();
        search_entry.connect_changed(move |e| {
            let q = e.text().to_string().to_lowercase();
            for (name, child) in flow_children_c.borrow().iter() {
                let visible = q.is_empty() || name.to_lowercase().contains(&q);
                child.set_visible(visible);
            }
            // if no query, show all
            if q.is_empty() {
                for (_, child) in flow_children_c.borrow().iter() {
                    child.set_visible(true);
                }
            }
            flow_c.queue_draw();
        });
    }

    win.set_child(Some(&overlay));

    // Async icon upgrade: one icon per 5ms tick so the window stays responsive.
    // The skeleton with placeholders is already built, present() below shows it
    // instantly – no more 15-20s empty transparent frame.
    {
        let pending_c = pending.clone();
        let win_c = win.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(5), move || {
            let item = pending_c.borrow_mut().pop();
            let Some((btn, item)) = item else {
                return glib::ControlFlow::Break;
            };
            let _ = &win_c;
            let generated = if item.bundle_path.is_some() {
                generate_real_icon(&item)
            } else {
                let (col, sym) = fallback_style(&item.display_name);
                generate_launchpad_icon(&item.display_name, col, sym)
            };
            match generated {
                Some(path) => {
                    let img = gtk::Image::from_file(&path);
                    img.set_pixel_size(64);
                    btn.set_child(Some(&img));
                }
                None => {
                    // Keep placeholder on failure (already set).
                }
            }
            if pending_c.borrow().is_empty() {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    // permanently selected: close when unselected / something else selected (only after first active)
    // is_dragging verhindert Schließen während Bridge-Drag
    let has_been_active = Rc::new(RefCell::new(false));
    let has_been_active_c = has_been_active.clone();
    let has_been_active_is = has_been_active.clone();
    let is_dragging_active = is_dragging.clone();
    win.connect_is_active_notify(move |w| {
        if *is_dragging_active.borrow() {
            return;
        }
        if w.is_active() {
            *has_been_active_c.borrow_mut() = true;
        } else if *has_been_active_is.borrow() {
            w.close();
        }
    });
    // also close on focus out (Wayland) - only after first active
    let focus = gtk::EventControllerFocus::new();
    let win_focus = win.clone();
    let has_been_active_focus = has_been_active.clone();
    let is_dragging_focus = is_dragging.clone();
    focus.connect_leave(move |_| {
        if *is_dragging_focus.borrow() {
            return;
        }
        // delay to allow click handling
        let w = win_focus.clone();
        let has_active = has_been_active_focus.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(100), move || {
            if *has_active.borrow() && !w.is_active() {
                w.close();
            }
        });
    });
    win.add_controller(focus);

    // center on screen (primary monitor) — robust: set WM hints before map
    // and keep re-asserting position via X11 move until mapped (like dock place_dock)
    // Pre-set hints if surface already exists (usually not yet)
    if let Some(surface) = win.surface() {
        if let Some(xid) = crate::x11_place::xid_of(&surface) {
            let scale = win.scale_factor().max(1);
            // try X11 RandR first, fallback to GDK monitor geometry (logical -> physical)
            let (mx, my, mw, mh) = crate::x11_place::primary_monitor().unwrap_or_else(|| {
                let display = gtk::prelude::WidgetExt::display(&win);
                display
                    .monitors()
                    .item(0)
                    .and_then(|o| o.downcast::<gtk::gdk::Monitor>().ok())
                    .map(|m| {
                        let g = m.geometry();
                        (g.x() * scale, g.y() * scale, g.width() * scale, g.height() * scale)
                    })
                    .unwrap_or((0, 0, 1920 * scale, 1080 * scale))
            });
            let win_w_phys = 900 * scale;
            let win_h_phys = 600 * scale;
            let px = mx + (mw - win_w_phys) / 2;
            let py = my + (mh - win_h_phys) / 2;
            crate::x11_place::set_position_hints(xid, px, py);
        }
    }

    win.present();
    win.grab_focus();

    // Ensure center on primary monitor via X11 move after map — retry loop
    // so WM (Weston XWayland) cannot override the spot.
    {
        let win_c = win.clone();
        let mut ticks = 0u32;
        glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
            ticks += 1;
            let Some(surface) = win_c.surface() else {
                return if ticks < 50 {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                };
            };
            let Some(xid) = crate::x11_place::xid_of(&surface) else {
                return if ticks < 50 {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                };
            };
            let scale = win_c.scale_factor().max(1);
            let (mx, my, mw, mh) = crate::x11_place::primary_monitor().unwrap_or_else(|| {
                let display = gtk::prelude::WidgetExt::display(&win_c);
                display
                    .monitors()
                    .item(0)
                    .and_then(|o| o.downcast::<gtk::gdk::Monitor>().ok())
                    .map(|m| {
                        let g = m.geometry();
                        (g.x() * scale, g.y() * scale, g.width() * scale, g.height() * scale)
                    })
                    .unwrap_or((0, 0, 1920 * scale, 1080 * scale))
            });
            let win_w_phys = 900 * scale;
            let win_h_phys = 600 * scale;
            let px = mx + (mw - win_w_phys) / 2;
            let py = my + (mh - win_h_phys) / 2;
            crate::x11_place::set_position_hints(xid, px, py);
            crate::x11_place::move_window(xid, px, py);
            if ticks < 50 {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }

    // ESC to close
    let key = gtk::EventControllerKey::new();
    let win_c = win.clone();
    key.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gtk::gdk::Key::Escape {
            win_c.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    win.add_controller(key);

    // click outside to close: click on overlay background outside bg_box
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let win_c2 = win.clone();
    let bg_box_c = bg_box.clone();
    let overlay_c = overlay.clone();
    click.connect_pressed(move |_, _, x, y| {
        // x,y are overlay-local; translate bg_box origin to overlay coords
        if let Some((bx, by)) = bg_box_c.translate_coordinates(&overlay_c, 0.0, 0.0) {
            let w = bg_box_c.allocated_width() as f64;
            let h = bg_box_c.allocated_height() as f64;
            if x < bx || x > bx + w || y < by || y > by + h {
                win_c2.close();
            }
        } else {
            // fallback: if translate fails, check against centered 760x520 rect
            let ox = overlay_c.allocated_width() as f64;
            let oy = overlay_c.allocated_height() as f64;
            let bw = bg_box_c.allocated_width() as f64;
            let bh = bg_box_c.allocated_height() as f64;
            let bx = (ox - bw) / 2.0;
            let by = (oy - bh) / 2.0;
            if x < bx || x > bx + bw || y < by || y > by + bh {
                win_c2.close();
            }
        }
    });
    overlay.add_controller(click);
}
