//! Dock appearance: light/dark resolution and the macOS glass materials.
//!
//! The mode comes from the TontooUI [`ThemeWatcher`] (the settings daemon,
//! live with a crossfade) and can be pinned with `DOCK_APPEARANCE=light|dark`.
//! Icons are cached per mode, so the mode has to be resolved before the first
//! tile is generated -- see [`crate::icons`].

use crate::TontooUI::theme::{ThemeMode, ThemeWatcher};
use crate::TontooUI::Color;

/// Dark or light, as the dock needs it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    /// System appearance, with the `DOCK_APPEARANCE` override on top.
    pub fn resolve() -> Self {
        if let Ok(value) = std::env::var("DOCK_APPEARANCE") {
            match value.as_str() {
                "light" => return Self::Light,
                "dark" => return Self::Dark,
                _ => {}
            }
        }
        Self::from_mode(watcher_mode())
    }

    pub fn from_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self::Light,
            ThemeMode::Dark => Self::Dark,
        }
    }

    /// Cache key suffix: every icon is cached twice (`-light` / `-dark`) so
    /// switching appearance never reuses the wrong variant.
    pub fn cache_suffix(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// True for the dark material set.
    pub fn is_dark(self) -> bool {
        self == Self::Dark
    }

    /// macOS-accurate panel materials.
    ///
    /// Light: dark tint, less blur -- more of the background shows through
    /// (26% instead of the 30% of the first generation).
    /// Dark: `rgba(30,30,30,0.60)` plus a `rgba(255,255,255,0.10)` hairline.
    pub fn material(self) -> Material {
        match self {
            Self::Light => Material {
                panel_fill: rgba(32, 32, 34, 0.26),
                border_color: rgba(255, 255, 255, 0.12),
                dot_color: rgba(0, 0, 0, 0.85),
                separator: rgba(255, 255, 255, 0.08),
                bg_fallback_top: rgba(55, 55, 58, 0.30),
                bg_fallback_bottom: rgba(28, 28, 30, 0.24),
                placeholder_fill: rgba(255, 255, 255, 0.12),
                placeholder_border: rgba(255, 255, 255, 0.28),
                tooltip_bg: rgba(28, 28, 30, 0.92),
                tooltip_text: rgba(255, 255, 255, 1.0),
            },
            Self::Dark => Material {
                panel_fill: rgba(30, 30, 30, 0.60),
                border_color: rgba(255, 255, 255, 0.10),
                dot_color: rgba(255, 255, 255, 0.92),
                separator: rgba(255, 255, 255, 0.14),
                bg_fallback_top: rgba(42, 42, 44, 0.65),
                bg_fallback_bottom: rgba(28, 28, 30, 0.55),
                placeholder_fill: rgba(255, 255, 255, 0.12),
                placeholder_border: rgba(255, 255, 255, 0.28),
                tooltip_bg: rgba(28, 28, 30, 0.92),
                tooltip_text: rgba(255, 255, 255, 1.0),
            },
        }
    }

    /// LaunchPad card background (`#ececec` / `#1d1d1d` at 90%).
    pub fn launchpad_bg(self) -> Color {
        match self {
            Self::Light => rgba(236, 236, 236, 0.90),
            Self::Dark => rgba(29, 29, 29, 0.90),
        }
    }

    /// LaunchPad card border.
    pub fn launchpad_border(self) -> Color {
        match self {
            Self::Light => rgba(0, 0, 0, 0.08),
            Self::Dark => rgba(255, 255, 255, 0.10),
        }
    }

    /// LaunchPad tile label color.
    pub fn launchpad_fg(self) -> Color {
        match self {
            Self::Light => crate::TontooUI::Color::from_rgb8(0x1d, 0x1d, 0x1f),
            Self::Dark => crate::TontooUI::Color::from_rgb8(0xf5, 0xf5, 0xf7),
        }
    }
}

/// Panel colors of one appearance, as premultiplied peniko colors.
#[derive(Clone, Copy, Debug)]
pub struct Material {
    /// Panel body fill.
    pub panel_fill: Color,
    /// 1 px panel hairline.
    pub border_color: Color,
    /// Running indicator dot.
    pub dot_color: Color,
    /// Vertical separator between apps and system items.
    pub separator: Color,
    /// Fallback gradient top stop (used when no backdrop image exists).
    pub bg_fallback_top: Color,
    /// Fallback gradient bottom stop.
    pub bg_fallback_bottom: Color,
    /// Drag placeholder body.
    pub placeholder_fill: Color,
    /// Drag placeholder dashed border.
    pub placeholder_border: Color,
    /// Hover label pill background.
    pub tooltip_bg: Color,
    /// Hover label text.
    pub tooltip_text: Color,
}

pub fn rgba(r: u8, g: u8, b: u8, alpha: f64) -> Color {
    Color::from_rgb8(r, g, b).with_alpha(alpha as f32)
}

thread_local! {
    /// One watcher per thread so the daemon connection is opened once. The
    /// layer-shell loop is single threaded, so this is the process watcher.
    static WATCHER: std::cell::RefCell<ThemeWatcher> =
        std::cell::RefCell::new(ThemeWatcher::new());
}

/// Seconds since process start, the clock the theme crossfade runs on.
pub fn start_seconds() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

/// Current system mode, without polling the daemon first.
fn watcher_mode() -> ThemeMode {
    WATCHER.with(|watcher| watcher.borrow().theme().mode)
}

/// Poll the watcher and return the resolved appearance plus the palette text
/// color. Call once per frame.
pub fn poll() -> (Appearance, Color) {
    WATCHER.with(|cell| {
        let mut watcher = cell.borrow_mut();
        let time = start_seconds();
        // `poll` drains the pushed `customize_changed` events and starts a
        // fade; `palette` blends the crossfade and `theme` reports the target.
        watcher.poll(time);
        let palette = watcher.palette(time);
        let mode = watcher.theme().mode;
        (Appearance::from_mode(mode), palette.text)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_material_matches_the_dock_spec() {
        let light = Appearance::Light.material();
        assert_eq!(light.panel_fill, rgba(32, 32, 34, 0.26));
        assert_eq!(light.border_color, rgba(255, 255, 255, 0.12));
        assert_eq!(light.dot_color, rgba(0, 0, 0, 0.85));
        assert_eq!(light.separator, rgba(255, 255, 255, 0.08));
    }

    #[test]
    fn dark_material_matches_the_dock_spec() {
        let dark = Appearance::Dark.material();
        assert_eq!(dark.panel_fill, rgba(30, 30, 30, 0.60));
        assert_eq!(dark.border_color, rgba(255, 255, 255, 0.10));
        assert_eq!(dark.dot_color, rgba(255, 255, 255, 0.92));
        assert_eq!(dark.separator, rgba(255, 255, 255, 0.14));
    }

    #[test]
    fn cache_suffixes_differ_per_mode() {
        assert_eq!(Appearance::Light.cache_suffix(), "light");
        assert_eq!(Appearance::Dark.cache_suffix(), "dark");
        assert!(Appearance::Dark.is_dark());
        assert!(!Appearance::Light.is_dark());
    }

    #[test]
    fn launchpad_background_follows_the_system_spec() {
        assert_eq!(Appearance::Light.launchpad_bg(), rgba(236, 236, 236, 0.90));
        assert_eq!(Appearance::Dark.launchpad_bg(), rgba(29, 29, 29, 0.90));
    }
}