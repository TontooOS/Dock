//! Real program data for the Dock and LaunchPad via CoreWindows.
//!
//! The dock pins the first [`DOCK_APP_COUNT`] entries of
//! `CoreWindows::list_programs()` (`~/Applications` + `/Applications`,
//! sorted by display name); every installed program is shown in the
//! LaunchPad. Running dots come from the open-window list
//! (`WindowsProvider::windows()`, the "list opened programs" API).
//!
//! When no `.app` bundles are installed (e.g. dev machines without
//! `/Applications`), a hardcoded demo set is used so the dock still
//! renders and stays interactive.

use std::path::PathBuf;

use crate::CoreIcon::{
    APP_FILL, CALENDAR, ENVELOPE_FILL, GEARSHAPE_FILL, MACWINDOW, MAGNIFYINGGLASS, MAP_FILL,
    MESSAGE_FILL, MUSIC_NOTE, NOTE_TEXT, PHOTO_FILL, SAFARI_FILL, SFSymbol, SLIDER_HORIZONTAL_3,
    TERMINAL_FILL,
};

/// How many installed programs are pinned in the dock.
/// The LaunchPad tile is always prepended, so the panel holds
/// `DOCK_APP_COUNT + 1` tiles.
pub const DOCK_APP_COUNT: usize = 5;

/// One app tile, either a real installed program or a demo fallback entry.
#[derive(Clone, Debug)]
pub struct AppItem {
    /// Localized display name (`Info.tontoo` name for real programs).
    pub display_name: String,
    /// Bundle id (`Info.tontoo` `bundle_id`; `demo.*` for fallback entries).
    pub bundle_id: String,
    /// Absolute bundle directory (`....app`). `None` for demo entries.
    pub bundle_path: Option<PathBuf>,
    /// Bundle icon file from CoreWindows, when the bundle ships one.
    pub icon_path: Option<PathBuf>,
    /// Fallback tile color (used when no bundle icon exists).
    pub color: &'static str,
    /// Fallback tile symbol (used when no bundle icon exists).
    pub symbol: SFSymbol,
    /// Legacy demo spawn command (demo entries only, e.g. `Some("vlc")`).
    pub demo_cmd: Option<&'static str>,
}

/// Demo programs used when no `.app` bundles are installed.
/// The LaunchPad tile is added separately by the dock, never from here.
pub const DEMO_PROGRAMS: &[(&str, &str, SFSymbol, Option<&str>)] = &[
    ("Sliders", "#0A84FF", SLIDER_HORIZONTAL_3, Some("vlc")),
    ("Finder", "#6C5CE7", MACWINDOW, None),
    ("Mail", "#D63031", ENVELOPE_FILL, None),
    ("Music", "#E17055", MUSIC_NOTE, None),
    ("Photos", "#00B894", PHOTO_FILL, None),
    ("Settings", "#636e72", GEARSHAPE_FILL, None),
];

/// Everything the dock and the LaunchPad need for one startup.
pub struct LoadedApps {
    /// The pinned dock programs (at most [`DOCK_APP_COUNT`]).
    pub dock: Vec<AppItem>,
    /// All installed programs (dock programs first, then the rest).
    pub all: Vec<AppItem>,
    /// `true` when no bundles were found and demo data is shown.
    pub demo_mode: bool,
}

/// Load installed programs via CoreWindows, falling back to demo data.
pub fn load() -> LoadedApps {
    let all: Vec<AppItem> = crate::CoreWindows::list_programs()
        .into_iter()
        .map(from_entry)
        .collect();
    if all.is_empty() {
        return LoadedApps {
            dock: demo_dock(),
            all: demo_launchpad(),
            demo_mode: true,
        };
    }
    let dock = all.iter().take(DOCK_APP_COUNT).cloned().collect();
    LoadedApps {
        dock,
        all,
        demo_mode: false,
    }
}

/// All installed programs (empty when none are installed).
/// Used by the LaunchPad icon prewarm so it warms the real tiles.
pub fn load_all() -> Vec<AppItem> {
    crate::CoreWindows::list_programs()
        .into_iter()
        .map(from_entry)
        .collect()
}

fn from_entry(entry: crate::CoreWindows::AppEntry) -> AppItem {
    let (color, symbol) = fallback_style(&entry.display_name);
    AppItem {
        display_name: entry.display_name.clone(),
        bundle_id: entry.bundle_id.clone(),
        bundle_path: Some(entry.bundle_path.clone()),
        icon_path: entry.icon.icon_path.clone(),
        color,
        symbol,
        demo_cmd: None,
    }
}

/// Demo dock programs (first [`DOCK_APP_COUNT`] of [`DEMO_PROGRAMS`]).
fn demo_dock() -> Vec<AppItem> {
    DEMO_PROGRAMS
        .iter()
        .take(DOCK_APP_COUNT)
        .map(|(name, color, symbol, cmd)| AppItem {
            display_name: name.to_string(),
            bundle_id: format!("demo.{}", name.to_lowercase()),
            bundle_path: None,
            icon_path: None,
            color,
            symbol: *symbol,
            demo_cmd: *cmd,
        })
        .collect()
}

/// Demo LaunchPad entries (mirrors the legacy hardcoded grid).
pub fn demo_launchpad() -> Vec<AppItem> {
    const NAMES: &[&str] = &[
        "Firefox",
        "Mail",
        "System Settings",
        "ICUE",
        "Preview",
        "AdBlock Pro",
        "App Store",
        "Automator",
        "BetterDisplay",
        "Books",
        "Calculator",
        "Calendar",
        "Chess",
        "Clock",
        "Contacts",
        "Dictionary",
        "Epic Games Launcher",
        "Epic Seven",
        "Eve",
        "FaceTime",
        "Music",
        "News",
        "Stocks",
        "Voice Memos",
        "Home",
        "Podcasts",
        "TV",
        "Wallet",
        "Shortcuts",
        "Find My",
    ];
    NAMES
        .iter()
        .map(|name| {
            let (color, symbol) = fallback_style(name);
            AppItem {
                display_name: name.to_string(),
                bundle_id: format!("demo.{}", name.to_lowercase().replace(' ', "-")),
                bundle_path: None,
                icon_path: None,
                color,
                symbol,
                demo_cmd: None,
            }
        })
        .collect()
}

/// Shared fallback color + symbol mapping so dock tiles, LaunchPad tiles
/// and the prewarm step render identical generated icons for the same name.
pub fn fallback_style(name: &str) -> (&'static str, SFSymbol) {
    match name {
        "Firefox" => ("#FF9500", SAFARI_FILL),
        "Mail" => ("#007AFF", ENVELOPE_FILL),
        "System Settings" => ("#636e72", GEARSHAPE_FILL),
        "App Store" => ("#0A84FF", APP_FILL),
        "Calculator" => ("#4CAF50", APP_FILL),
        "Calendar" => ("#FF3B30", CALENDAR),
        "Books" => ("#FF9500", NOTE_TEXT),
        "Contacts" => ("#5AC8FA", MESSAGE_FILL),
        "FaceTime" => ("#34C759", MESSAGE_FILL),
        "Music" => ("#AF52DE", MUSIC_NOTE),
        "Photos" => ("#00B894", PHOTO_FILL),
        _ => {
            const COLORS: &[&str] = &[
                "#FF3B30", "#007AFF", "#34C759", "#AF52DE", "#FF9500", "#5AC8FA", "#0A84FF",
                "#6C5CE7", "#D63031", "#00B894", "#E17055", "#636e72",
            ];
            const SYMBOLS: &[SFSymbol] = &[
                APP_FILL,
                ENVELOPE_FILL,
                GEARSHAPE_FILL,
                MACWINDOW,
                MESSAGE_FILL,
                MUSIC_NOTE,
                PHOTO_FILL,
                SAFARI_FILL,
                CALENDAR,
                TERMINAL_FILL,
                MAP_FILL,
                NOTE_TEXT,
                SLIDER_HORIZONTAL_3,
                MAGNIFYINGGLASS,
            ];
            let h = name.len() * 31 + *name.as_bytes().first().unwrap_or(&b'A') as usize;
            (COLORS[h % COLORS.len()], SYMBOLS[h % SYMBOLS.len()])
        }
    }
}

/// One open window relevant for dots and click toggling.
#[derive(Clone, Debug)]
pub struct OpenWindow {
  /// Daemon-side window id (for minimize/restore actions).
  pub id: u64,
  /// Whether the window is currently minimized to the dock.
  pub minimized: bool,
  /// Bundle id from the owning `.app` `Info.tontoo`, if any.
  pub bundle_id: Option<String>,
  /// Display name (localized bundle name, title or app id).
  pub app_name: Option<String>,
}

/// Snapshot of the currently open programs for dots and click toggling.
pub struct OpenSnapshot {
  /// `false` when the window daemon is unreachable; dots then fall back
  /// to the legacy demo heuristic (see [`is_running`]) and clicks fall
  /// back to launching.
  pub daemon_ok: bool,
  /// All open windows (visible and minimized).
  pub windows: Vec<OpenWindow>,
}

/// Currently open programs via CoreWindows (`WindowsProvider::windows`).
/// Never fails: an unreachable daemon yields an empty snapshot with
/// `daemon_ok == false`.
pub fn open_snapshot() -> OpenSnapshot {
  match crate::CoreWindows::WindowsProvider::from_env().windows() {
    Ok(windows) => {
      let windows = windows
        .into_iter()
        .map(|w| OpenWindow {
          id: w.id,
          minimized: w.minimized,
          bundle_id: w.bundle_id,
          app_name: w.app_name,
        })
        .collect();
      OpenSnapshot {
        daemon_ok: true,
        windows,
      }
    }
    Err(err) => {
      if std::env::var("DOCK_DEBUG").is_ok() {
        eprintln!("[dock] open windows unavailable: {err}");
      }
      OpenSnapshot {
        daemon_ok: false,
        windows: Vec::new(),
      }
    }
  }
}

fn window_matches(item: &AppItem, window: &OpenWindow) -> bool {
  window
    .bundle_id
    .as_deref()
    .map(|bundle| bundle.eq_ignore_ascii_case(&item.bundle_id))
    .unwrap_or(false)
    || window
      .app_name
      .as_deref()
      .map(|name| name.eq_ignore_ascii_case(&item.display_name))
      .unwrap_or(false)
}

/// Whether `item` currently runs and deserves a dot (visible or
/// minimized windows count).
///
/// With a reachable daemon this is exact (bundle id or app name matches an
/// open window). Without a daemon only demo entries show dots, using the
/// legacy heuristic (Finder/Mail always, Sliders while its process runs).
pub fn is_running(item: &AppItem, snapshot: &OpenSnapshot) -> bool {
  if snapshot.daemon_ok {
    return snapshot.windows.iter().any(|w| window_matches(item, w));
  }
  if item.bundle_path.is_some() {
    return false;
  }
  match item.display_name.as_str() {
    "Finder" | "Mail" => true,
    "Sliders" => crate::x11_place::is_app_running(),
    _ => false,
  }
}

/// Open windows of `item` as `(daemon id, minimized)` pairs.
/// Empty when the app is not running.
pub fn app_windows(item: &AppItem, snapshot: &OpenSnapshot) -> Vec<(u64, bool)> {
  snapshot
    .windows
    .iter()
    .filter(|w| window_matches(item, w))
    .map(|w| (w.id, w.minimized))
    .collect()
}
