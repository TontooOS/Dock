# Dock

The dock is a macOS Sonoma / Sequoia-accurate glass bar of app tiles rendered
with TontooUIKit (`App`, `View`, `ViewContent`) and GTK4. It provides
running-app dots, bounce on launch, a vertical separator and HiDPI-correct X11
placement with click-through outside the panel. Hover magnification is
intentionally disabled (static icons).

## Design Tokens

| Group | Constant | Default | Meaning |
|---|---|---|---|
| Glass Panel | `PANEL_RADIUS` | `20` px | Continuous squircle, 18–24 px macOS range |
| Glass Panel | `PANEL_HEIGHT` | `90` px logical | `((53+2*11)*1.2)` 10% smaller, icon centered |
| Glass Panel | `panel_width()` | `n*53+(n-1)*9+2*20` px logical | Dynamic: LaunchPad tile + pinned programs (`TILE_COUNT`) |
| Glass Panel | `BORDER_WIDTH` | `1` px | Hairline border |
| Material Light | `panel_fill` | `rgba(255,255,255,0.30)` | Frosted white tint, 0.25–0.35 |
| Material Light | `border_color` | `rgba(255,255,255,0.40)` | 0.5–1 px hairline |
| Material Dark | `panel_fill` | `rgba(30,30,30,0.60)` | Dark tint, 0.55–0.65 |
| Material Dark | `border_color` | `rgba(255,255,255,0.10)` | Subtle neutral rim |
| Layout | `BOTTOM_GAP` | `8` px | Bottom-center float, 8–12 px |
| Icons | `ICON_SIZE` | `53` px | Tile size, 5% smaller (56→53) |
| Icons | `ICON_GAP` | `9` px | Gap, 10% smaller |
| Icons | `ROW_PAD` | `12` px | Horizontal breathing room, 10% smaller |
| Dots | `DOT_SIZE` | `4` px | Running indicator |
| Backdrop | `BLUR_RADIUS` | `30.0` | Gaussian blur |
| Backdrop | `BACKDROP_SCALE` | `0.25` | Downscale before blur |

Glass CSS (`src/main.rs:464`):

```css
.dock-panel {
  background: rgba(255,255,255,0.30); /* or rgba(30,30,30,0.60) */
  border-radius: 20px;
  border: 1px solid rgba(255,255,255,0.40);
  box-shadow: 0 16px 48px rgba(0,0,0,0.22), 0 4px 16px rgba(0,0,0,0.16), inset 0 1px 0 rgba(255,255,255,0.32);
  backdrop-filter: blur(30px) saturate(180%);
}
```

Dark mode (`src/main.rs:54` `Appearance::material`) uses `rgba(30,30,30,0.60)` and
`rgba(255,255,255,0.10)`, with a stronger drop shadow
`0 16px 48px rgba(0,0,0,0.45), 0 6px 20px rgba(0,0,0,0.38)`. Appearance is
resolved via `ColorScheme::detect_system()` with `DOCK_APPEARANCE=light|dark`
override (`src/main.rs:35`).

## Programs (CoreWindows)

Installed programs come from CoreWindows `list_programs()`
(`~/Applications` + `/Applications`, sorted by display name;
`src/apps.rs`). The dock shows the user's CoreData pins in pin order
(`src/pins.rs`, entity `DockPin` under `com.tontoo.dock`); first run
materializes the first `DOCK_APP_COUNT = 5` entries as pins. The
LaunchPad shows every installed program.
Tile icons prefer the bundle icon file (`AppEntry.icon.icon_path`,
used as-is); programs without a bundle icon get a generated CoreIcon
gradient tile keyed by bundle id (`tontoo-dock-<bundle>-<light|dark>-v3.png`).

When no `.app` bundles are installed (e.g. dev machines), a hardcoded
demo set is shown instead (`apps::DEMO_PROGRAMS` in the dock,
`apps::demo_launchpad()` in the LaunchPad) with the legacy log/spawn
click behavior. The separator (`dock-item-sep`) is only shown in demo
mode; with real programs there is no system area, so no separator is
rendered.

## Temp Icon Generation

Each tile PNG is generated at startup with the CoreIcon generator
(`IconCanvas`) into `/tmp/tontoo-dock-<app>-<light|dark>-v3.png`
(`src/main.rs:250`). Every icon is cached twice, once per appearance, so
switching light/dark never reuses the wrong variant (`cache_suffix()`):

- Background: top-to-bottom gradient of the app color (28 % lighter at top,
  10 % darker at bottom)
- `corner_radius(250)` on 1024 px canvas: iOS squircle
- `specular(0.15)` + `inner_depth(30, 0.30)`
- White SF Symbol centered at 600x600 px
- Launchpad uses raster `Resources/launchpad.png` through
  `build_launchpad_image()` (`src/main.rs`): light renders via
  `IconCanvas::dark_light_mode(Light)`, dark flattens the source over opaque
  white first and then swaps the background to the dark preset with the same
  glass finish. The flatten step is required because the source has a
  transparent border, which defeats CoreIcon's border-seeded flood fill (it
  would silently keep the white background). Cached as
  `/tmp/tontoo-dock-launchpad-<light|dark>-v6.png`

Appearance is resolved via `Appearance::resolve()` before icon generation
(`src/main.rs:1935`), honoring `DOCK_APPEARANCE=light|dark` with
`ColorScheme::detect_system()` fallback, so the correct cache variant is
generated on startup.

If generation fails the tile falls back to a letter tile (`src/main.rs:595`).

## Glass & Backdrop

The window itself is fully transparent (`src/main.rs:1784`):

```css
window { background-color: transparent; }
```

Only `.dock-panel` paints. The backdrop is baked per
`src/backdrop.rs:111` `generate()`:

1. Sample wallpaper behind the panel (Windows `TranscodedWallpaper` or
   `DOCK_WALLPAPER` override, `src/backdrop.rs:37`).
2. Downscale by `BACKDROP_SCALE` (0.25), `imageops::blur` with `BLUR_RADIUS`
   (30), upscale, then color-grade (`saturation 1.8/1.6`, `brightness +0.08`)
   and round corners (`src/backdrop.rs:88`).

If no wallpaper is found or transcoding fails a guaranteed fallback gradient
(`src/backdrop.rs:92` `generate_fallback()`) is used – a blurred two-stop
gradient matching the material (light: white→`#E4E4E8`, dark: `#2A2A2C`→`#161618`)
with the same grading and rounded corners, so the panel never appears black.
The panel CSS also has a fallback linear gradient when `backdrop` is `None`
(`src/main.rs:450`).

## Icon Behavior

Icons remain at a fixed `56` px size – hover magnification is disabled per
user request (previously a cosine falloff with `1.55` peak scale). Hover only
shows the native GTK tooltip (`tile.set_tooltip_text`) after the GTK4 default
delay – no custom hover label is rendered. No explicit timeout is set in
`main()`: the GTK3 `gtk-tooltip-timeout` / `gtk-tooltip-browse-timeout`
`GtkSettings` properties do not exist on GTK4 and setting them panics with
`property 'gtk-tooltip-timeout' of type 'GtkSettings' not found`.

### Running Indicators (src/main.rs, src/apps.rs)

Each tile is wrapped in a vertical `dock-item` box with a
4 px dot below the button. Dots reflect the CoreWindows open-window
list (`WindowsProvider::windows()`, the "list opened programs" API):
a tile shows its dot when its bundle id or app name matches an open
window. The list is re-queried every 2 s on the main loop and each dot
is toggled accordingly (`src/main.rs` refresh timeout).

Without a reachable window daemon only demo entries show dots, using
the legacy heuristic (`Finder`/`Mail` always, `Sliders` while its
process runs via `x11_place::is_app_running()`).

```rust
let dot = gtk::Box::new(Orientation::Horizontal, 0);
dot.add_css_class("dock-dot");
dot.set_margin_top(4);
```

`apps::is_running()` matches the tile against the snapshot; demo
fallback without a daemon uses the legacy heuristic. CSS:

```css
.dock-dot { background: rgba(0,0,0,0.85); /* light: black dot */ }
.dock-dot { background: rgba(255,255,255,0.92); /* dark: white dot */ }
```

Non-running dots get `dock-dot-hidden` (`opacity:0`) so layout stays stable.

### Bounce on Launch (src/main.rs:142)

Every click triggers `bounce_tile_dynamics()` (`src/main.rs:160`) – a
multi-bounce parabola with 3 decays:

```rust
fn calculate_y_displacement(t: f32, duration: f32, max_h: f32) -> f32
```

`-52 px` first peak, `-18 px` second, `-4.7 px` third, duration 900 ms via
a 16 ms `timeout_add_local` that writes `transform: translateY(y)` to a
dedicated `CssProvider` prio +30. The Sliders tile keeps its existing
client-count loop that re-bounces until the window appears
(`src/main.rs:1058`).

## Layout & Separation

- Panel is bottom-center with `BOTTOM_GAP = 8`, matching
  macOS 8–12 px float. Window `win_w = panel_width() + GHOST_SIZE` (79),
  `win_h = PANEL_HEIGHT + GHOST_SIZE + BOTTOM_GAP` is placed
  `mx + (mw - win_w*scale)/2` / `my + mh - win_h*scale` in physical pixels.
  Dock is 10% smaller (`449×90` vs `500×101`), icons
  only 5% smaller (`53` vs `56`). Icons are vertically centered: wrapper
  `valign: Center`
  so the icon center aligns to panel center (dot sits in bottom padding).
- Row holds `dock-item` wrappers with `ICON_GAP = 9`.
  A vertical separator is rendered as a right border on the item after
  `separator_after` (demo fallback mode only, `Some(4)`):

```css
.dock-item-sep { border-right: 1px solid rgba(0,0,0,0.10); padding-right:10px; margin-right:4px; }
```

Dark variant uses `rgba(255,255,255,0.14)`. Drag reordering skips the
separator – it is a styling of the wrapper, not an extra widget, so the
placeholder logic stays simple.

## Window Setup & Robustness

The UIKit root uses `App::no_window_bar()` (no traffic lights / drag area)
plus `App::no_window_frame()` (no UIKit margin, corner radius, border or
shadow) – the dock draws its own glass panel on a fully transparent window.

### Input Region & Click-Through (src/main.rs:1839)

The transparent GTK window covers `PANEL_WIDTH+GHOST_SIZE` ×
`PANEL_HEIGHT+GHOST_SIZE+BOTTOM_GAP` logical. Only the glass panel is
clickable (`SHAPE` input region, physical pixels):

```rust
let panel_x = (win_w - PANEL_WIDTH)/2;
let panel_y = win_h - PANEL_HEIGHT - BOTTOM_GAP;
x11_place::set_input_region(xid, panel_x*scale, panel_y*scale,
  (PANEL_WIDTH*scale) as u16, (PANEL_HEIGHT*scale) as u16);
surface.set_input_region(&Region::new(panel_x, panel_y, PANEL_WIDTH, PANEL_HEIGHT));
```

During drag the region is expanded to the full window (`src/main.rs:900`) and
restored on drop (`src/main.rs:1350`). All `move_window`/`set_position_hints`
calls use `win.scale_factor()` (`src/main.rs:1845`) and flush the X11
connection, fixing HiDPI placement on 4K scaled displays.

### HiDPI Scaling Fix (src/main.rs:1839, src/x11_place.rs:1417)

- `primary_monitor()` via RandR primary output (`src/main.rs:1451`)
- `px = mx + (mw - win_w*scale)/2`, `py = my + mh - win_h*scale`
- `set_position_hints`/`move_window` with physical coords
- `set_input_region` with `*scale` for X11, logical for GDK

Retried every 40 ms for 2 s after startup (`src/main.rs:1939`) so Weston
cannot override the position.

### Translation (SF Pro & lang/)

- Fonts: SF Pro Display loaded from `/usr/share/fonts/OTF` + `/TTF`
  (`BaseOS/fonts/SF-Pro/`). Global CSS forces
  `font-family: 'SF Pro Display', 'SF Pro Text'` (`src/main.rs:466`).
- Languages: `lang/en_us.json` and `lang/de_de.json` (
  `AGENTS.md`). Loaded via `load_lang()` (`src/main.rs:78`) based on
  `LANG`/`LC_ALL` (contains `de` → `de_de`). Tooltips and launch logs use
  `tr(map, "dock.app.<name>", fallback)` (`src/main.rs:108`).

## Drag & Drop

Long-press 500 ms → grow to `GHOST_SIZE` (1.5×) with pop to 1.65× →
remove wrapper from row, show dashed placeholder, expand input region to full
window. Motion recomputes `new_gap = (ghost_cx - row_x + slot/2)/slot` and
animates affected icons with a translate delta (`src/main.rs:1277`). Drop
via the overlay `GestureClick` restores panel-only input and suppresses the
follow-up click for 50 ms.

## Launching (src/launcher.rs)
Clicking a tile never runs app code inside the dock. `launcher::launch_app()`
hands the bundle path to a detached helper on a throwaway thread (the GTK
main loop is never blocked):

1. Preferred: the LaunchPad daemon (`/run/launchpad.sock`, `start_app` op,
   same framing as `launchpad_lib::LaunchpadClient::start_app`). The daemon
   supervises the app as its own temp process, so a crashing app can never
   take the dock down.
2. Fallback: `tapp` (FishRunner, always at `/usr/bin/tapp` on TontooOS,
   plain `tapp` from `PATH` elsewhere) is spawned detached with nulled
   stdio and its own process group on Linux. The dock never waits on it.

Dock tiles bounce first, then toggle: a fresh open-window snapshot
decides per click — visible windows of the app minimize
(`WindowsProvider::minimize_window`), fully minimized apps restore
(`WindowsProvider::restore_window`), closed apps launch out-of-process.
LaunchPad tiles launch and close the grid macOS-style. Demo entries
without a bundle only log (Sliders keeps its legacy `vlc` launch loop).

`launcher::open_in_finder()` reveals a bundle in the file manager
(`tapp <Finder.app> -- <dir>`, else `xdg-open <dir>`).

## Pins (src/pins.rs)

Dock membership is stored per user in CoreData (`com.tontoo.dock`,
entity `DockPin` with `bundle_id` + `position`, plus a `DockMeta`
marker so an explicitly emptied dock stays empty). New pins append far
right. A version counter lets the running dock rebuild its row live
(500 ms watch, deferred while dragging, panel re-placed afterwards);
unavailable CoreData falls back to unpinned defaults without saving.

## Launchpad

Launchpad (`src/launchpad.rs` `show_launchpad(items)`) opens centered
`900x600` with a `760x520` card showing every installed program (empty
input falls back to the demo grid). Background follows the system spec:
light `rgba(236,236,236,0.90)` (`#ececec`), dark `rgba(29,29,29,0.90)`
(`#1d1d1d`), plus `blur(24px) saturate(180%)`.

### Context Menu

Right-clicking a tile opens a popover menu: Open (launches and closes
the grid), a divider, Open In Finder (reveals the bundle), and Pin to
Dock — or Remove from Dock when already pinned (real programs only).
Pin changes save to CoreData and appear live in the dock (see Pins).

### Async Icon Loading

Grid icons plus the top Tontoo octopus icon were generated
synchronously with `IconCanvas::save` on the UI thread, blocking the window
for 15-20 s behind an empty transparent frame. The fix:

1. Build the skeleton first with letter placeholders
   (`placeholder_tile()`, shared `apps::fallback_style()` mapping).
2. Serve bundle icons (`AppEntry.icon.icon_path`) and cache hits
   (`/tmp/tontoo-launchpad-*-v2.png` demo, `/tmp/tontoo-launchpad-*-v3.png`
   bundle-id keyed) instantly.
3. `present()` the window immediately, then upgrade missing icons one per
   5 ms `timeout_add_local` tick without freezing input.
4. Load the Tontoo top icon async via `generate_tontoo_icon_cached()`
   (10 ms deferred).

No `set_visible(true)` before content exists, so no empty transparent frame
is shown.

### Cache Prewarm (src/launchpad.rs, src/main.rs)
`launchpad::prewarm_step()` generates one missing cache file per call without
GTK calls over the real program list (loaded once via `OnceLock`; demo grid
when nothing is installed). Bundle icons need no warming. `main()` schedules
it on the main loop (start after 2 s, then one icon per 50 ms), so the first
Launchpad open after `/tmp` was cleared or a cache version bump is already
warm. Cache hits skip generation entirely. Main thread only:
`CoreIcon::generator::ASSETS_DIR` is `static mut`, so a background thread
would data-race and abort GTK randomly.

## Building and Testing

```bash
wsl -d archlinux -- bash -lc 'cd /mnt/c/Users/arlo1/Documents/TontooProgramms/Dock && cargo build'
./target/debug/dock
DOCK_APPEARANCE=light ./target/debug/dock  # force light
DOCK_DEBUG=1 ./target/debug/dock           # verbose placement
```

Dependencies via `/Library/System/sdk` (`UIKit`, `CoreIcon`,
`UIKitDynamics`, `TontooUI`, `CoreWindows`) plus `serde_json` for `lang/` files.

## Cross References

- [MAIN.md](MAIN.md) -- project overview
- [RULE.md](RULE.md) -- wiki design system
