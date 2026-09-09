//! Bridge Window – füllt Lücke zwischen LaunchPad und Dock während Drag
//! Transparentes Vollbild-Fenster mit Ghost, Dock-Scale Animation.

use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::x11_place;

/// Dock wächst von 1.0 → 1.35 während Ghost über Dock
const DOCK_HOVER_SCALE: f32 = 1.35;
const DOCK_SCALE_CSS: &str = "dock-bridge-hover";

/// Erzeugt ein transparentes Fullscreen Bridge Window für den Drag.
/// Gibt (window, overlay, ghost_box) zurück. Window ist noch nicht presented.
pub fn create_bridge_window() -> Option<(gtk::Window, gtk::Overlay, gtk::Box)> {
    let win = gtk::Window::new();
    win.set_title(Some("TontooBridge"));
    win.set_decorated(false);
    win.set_resizable(false);
    // transparent window – kein Hintergrund, nur Ghost
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "window { background-color: transparent; } \
         overlay { background-color: transparent; } \
         .bridge-ghost { background: transparent; border-radius: 16px; box-shadow: 0 12px 32px rgba(0,0,0,0.45); } \
         .bridge-ghost image { transition: all 90ms ease-out; }",
    );
    win.add_css_class("bridge-window");
    win.style_context()
        .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 120);

    // monitor Größe für Fullscreen
    let scale = win.scale_factor().max(1);
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
    let win_w_log = mw / scale;
    let win_h_log = mh / scale;
    win.set_default_size(win_w_log, win_h_log);
    win.set_size_request(win_w_log, win_h_log);

    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    // transparent background box
    let bg = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bg.set_hexpand(true);
    bg.set_vexpand(true);
    overlay.set_child(Some(&bg));

    let ghost = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    ghost.set_size_request(96, 96);
    ghost.set_halign(gtk::Align::Start);
    ghost.set_valign(gtk::Align::Start);
    ghost.set_visible(false);
    ghost.add_css_class("bridge-ghost");
    overlay.add_overlay(&ghost);

    win.set_child(Some(&overlay));

    // Position auf Primary Monitor (X11)
    if let Some(surface) = win.surface() {
        if let Some(xid) = x11_place::xid_of(&surface) {
            x11_place::set_position_hints(xid, mx, my);
        }
    }
    // Nach present nochmal bewegen (wie Dock/LaunchPad Retry)
    let win_c = win.clone();
    let mx_c = mx;
    let my_c = my;
    glib::timeout_add_local_once(std::time::Duration::from_millis(10), move || {
        if let Some(surface) = win_c.surface() {
            if let Some(xid) = x11_place::xid_of(&surface) {
                x11_place::move_window(xid, mx_c, my_c);
            }
        }
    });

    Some((win, overlay, ghost))
}

/// Dock-Skalierung während Bridge-Drag. Sucht Dock Panel und appliziert Scale.
pub fn set_dock_hover(hover: bool) {
    for widget in gtk::Window::list_toplevels() {
        if let Ok(win) = widget.clone().downcast::<gtk::Window>() {
            // Dock erkennen via Titel oder Panel-Klasse
            let widget_ref: &gtk::Widget = win.upcast_ref();
            let is_dock = win.title().as_deref() == Some("Dock")
                || find_dock_panel(widget_ref).is_some();
            if !is_dock {
                continue;
            }
            if let Some(panel) = find_dock_panel(widget_ref) {
                if hover {
                    let css = format!(
                        ".{} {{ transform: scale({}); transition: transform 220ms cubic-bezier(0.2,0,0,1); transform-origin: bottom center; }}",
                        DOCK_SCALE_CSS, DOCK_HOVER_SCALE
                    );
                    let p = gtk::CssProvider::new();
                    p.load_from_string(&css);
                    panel.add_css_class(DOCK_SCALE_CSS);
                    panel
                        .style_context()
                        .add_provider(&p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 50);
                    // Provider merken? Für simplicity beim Unhover einfach Klasse entfernen
                    // (Provider bleibt, aber Klasse entfernt → kein Effekt)
                } else {
                    panel.remove_css_class(DOCK_SCALE_CSS);
                }
            }
        }
    }
}

fn find_dock_panel(root: &gtk::Widget) -> Option<gtk::Box> {
    if let Ok(b) = root.clone().downcast::<gtk::Box>() {
        if b.has_css_class("dock-panel") {
            return Some(b);
        }
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_dock_panel(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

pub fn set_launchpad_hover(hover: bool) {
    for widget in gtk::Window::list_toplevels() {
        if let Ok(win) = widget.clone().downcast::<gtk::Window>() {
            if win.title().as_deref() != Some("LaunchPad") {
                continue;
            }
            // LaunchPad bg_box finden via Klasse launchpad-bg
            fn find_bg(root: &gtk::Widget) -> Option<gtk::Box> {
                if let Ok(b) = root.clone().downcast::<gtk::Box>() {
                    if b.has_css_class("launchpad-bg") {
                        return Some(b);
                    }
                }
                let mut child = root.first_child();
                while let Some(c) = child {
                    if let Some(found) = find_bg(&c) {
                        return Some(found);
                    }
                    child = c.next_sibling();
                }
                None
            }
            if let Some(bg) = find_bg(win.upcast_ref()) {
                if hover {
                    bg.add_css_class("launchpad-bridge-hover");
                    let p = gtk::CssProvider::new();
                    p.load_from_string(".launchpad-bridge-hover { transform: scale(1.04); transition: transform 220ms cubic-bezier(0.2,0,0,1); transform-origin: center center; }");
                    bg.style_context().add_provider(&p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 50);
                } else {
                    bg.remove_css_class("launchpad-bridge-hover");
                }
            }
        }
    }
}

/// Prüft ob Punkt (global logical) über Dock-Panel liegt (mit Toleranz)
pub fn is_over_dock(logical_x: i32, logical_y: i32, scale: i32) -> bool {
    // Dock Panel Rect wie in place_dock: bottom-center – synced with main.rs constants
    // Panel width is dynamic (LaunchPad tile + pinned programs, see crate::panel_width).
    let panel_width: i32 = crate::panel_width();
    const PANEL_HEIGHT: i32 = ((53 + 2 * 11) as f32 * 1.2) as i32;
    const BOTTOM_GAP: i32 = 8;
    // Monitor logisch
    let win = match gtk::Window::list_toplevels()
        .into_iter()
        .find_map(|w| w.downcast::<gtk::Window>().ok())
    {
        Some(w) => w,
        None => return false,
    };
    let (mx, my, mw, mh) = x11_place::primary_monitor()
        .map(|(a, b, c, d)| (a / scale, b / scale, c / scale, d / scale))
        .unwrap_or((0, 0, 1920, 1080));
    let dock_x = mx + (mw - panel_width) / 2;
    let dock_y = my + mh - PANEL_HEIGHT - BOTTOM_GAP;
    // Toleranz 40px größer für leichtes Hover
    let pad = 40;
    logical_x >= dock_x - pad
        && logical_x <= dock_x + panel_width + pad
        && logical_y >= dock_y - pad
        && logical_y <= dock_y + PANEL_HEIGHT + pad
}
