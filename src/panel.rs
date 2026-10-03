//! The dock panel: one bottom-anchored TontooUI layer surface.
//!
//! The whole surface is `panel_width() + GHOST_SIZE` by
//! `PANEL_HEIGHT + GHOST_SIZE + BOTTOM_GAP`, exactly the geometry the GTK
//! dock used, so the compositor centers it at the bottom and the 8 px float
//! gap, the ghost headroom and the drag region all stay where they were.
//! [`App::input_region`] shrinks the clickable area to the glass panel and
//! grows it back to the whole surface while a tile is dragged.
//!
//! Layout and painting are TontooUI elements: a `ZStack` of the baked
//! backdrop `FileImage`, the `RoundedRectangle` panel body plus hairline and
//! an `HStack` row of tiles, each a `GestureArea<Animated<ZStack>>` holding a
//! `FileImage` (or a colored `RoundedRectangle` plus `BasicText` letter when
//! the generated artwork is missing). The three pieces whose position the
//! stacks cannot express -- the running dots below the icons, the drag ghost
//! with its drop placeholder and the hover label -- are TontooUI element
//! views placed directly each frame.

use std::cell::RefCell;
use std::rc::Rc;

use crate::TontooUI::elements::{
    Align, Animated, BasicText, Circle, FileImage, GestureArea, HStack, ImageFit, Keyframe,
    Padding, RoundedRectangle, Spacer, TextAlignment, Transform, View, ZStack,
};
use crate::TontooUI::renderer::layershell::{LayerBarOptions, OverlayRequest};
use crate::TontooUI::renderer::window::{App, Viewport};
use crate::TontooUI::renderer::{FontSystem, ImageLoader};
use crate::TontooUI::{Color, Scene};
use crate::apps::{self, AppItem};
use crate::i18n::trk;
use crate::icons;
use crate::launchpad::LaunchPadApp;
use crate::launcher;
use crate::pins;
use crate::theme::{self, Appearance, Material};
use crate::CoreWindows;

// ── Panel shape (macOS continuous squircle, 18-24 px) ───────────────────
/// Corner radius of the glass panel.
pub const PANEL_RADIUS: f32 = 20.0;
/// Hairline width of the panel border.
pub const BORDER_WIDTH: f32 = 1.0;

// ── Icon layout ────────────────────────────────────────────────────────
// 10% smaller dock overall, icons only 5% smaller, icons centered in panel
/// Tile size (56 * 0.95 = 53.2).
pub const ICON_SIZE: f32 = 53.0;
/// Gap between tiles (10 * 0.90).
pub const ICON_GAP: f32 = 9.0;
/// macOS bottom gap 8-12 px, 8 px like Sonoma.
pub const BOTTOM_GAP: f32 = 8.0;
/// Breathing room left and right inside the panel.
pub const ROW_PAD: f32 = 20.0;
/// Drag ghost size (53 * 1.5 = 79).
pub const GHOST_SIZE: f32 = 79.0;
/// Panel height is icon-driven: 90 px with the icon vertically centered.
pub const PANEL_HEIGHT: f32 = ((ICON_SIZE + 2.0 * 11.0) * 1.2).round();
/// Running indicator diameter.
pub const DOT_SIZE: f32 = 4.0;
/// Gap between the icon bottom and the dot.
pub const DOT_MARGIN: f32 = 4.0;
/// Corner radius of a generated tile (ICON_SIZE / 4).
const TILE_RADIUS: f32 = ICON_SIZE / 4.0;
/// Drag placeholder corner radius.
const PLACEHOLDER_RADIUS: f32 = 14.0;
/// Letter fallback metrics (white bold 20 px on the app color).
const LETTER_SIZE: f32 = 20.0;
const LETTER_WEIGHT: f32 = 800.0;
/// Hover label metrics.
const TOOLTIP_SIZE: f32 = 13.0;
const TOOLTIP_RADIUS: f32 = 6.0;
const TOOLTIP_PAD_X: f32 = 8.0;
const TOOLTIP_PAD_Y: f32 = 4.0;

/// Row child that is a drag placeholder rather than a tile.
const TILE_PLACEHOLDER: usize = usize::MAX;
/// Row child that is the app/system separator rather than a tile.
const TILE_SEPARATOR: usize = usize::MAX - 1;

/// The vertical separator between apps and system items, exactly the old CSS
/// geometry: 10 px of padding after the icon, a 1 px line, then 4 px of
/// margin before the next tile.
const SEPARATOR_PAD: f32 = 10.0;
const SEPARATOR_MARGIN: f32 = 4.0;
const SEPARATOR_WIDTH: f32 = 1.0;

/// Panel width in logical px for `tiles` entries:
/// `n * ICON_SIZE + (n - 1) * ICON_GAP + 2 * ROW_PAD`.
pub fn panel_width_for(tiles: usize) -> f32 {
    let n = tiles.max(1) as f32;
    n * ICON_SIZE + (n - 1.0) * ICON_GAP + 2.0 * ROW_PAD
}

/// Surface width: the panel plus the ghost headroom on both sides.
pub fn surface_width_for(tiles: usize) -> u32 {
    (panel_width_for(tiles) + GHOST_SIZE).round().max(1.0) as u32
}

/// Surface height: the panel plus the ghost headroom on top and the float gap
/// below.
pub const SURFACE_HEIGHT: u32 = (PANEL_HEIGHT + GHOST_SIZE + BOTTOM_GAP).round() as u32;

// ── Bounce physics ─────────────────────────────────────────────────────
/// Peak of the first hop in logical px.
const BOUNCE_PEAK: f32 = 52.0;
/// One full bounce sequence in seconds.
const BOUNCE_SECONDS: f32 = 0.9;

/// Vertical offset of a bouncing tile.
///
/// Three decaying parabolas inside one sequence: -52 px, then 35% of that,
/// then 9%, each starting and ending at rest. This is the curve the dock has
/// always used; [`bounce_keyframes`] samples it so TontooUI's animation engine
/// replays exactly this motion.
pub fn calculate_y_displacement(t: f32, duration: f32, max_h: f32) -> f32 {
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

/// Bounce curve as `Animated` keyframes, sampled every 30 ms. Linear
/// interpolation between two samples of a parabola stays within a fraction of
/// a pixel, so the replay is the original motion.
fn bounce_keyframes() -> Vec<Keyframe> {
    use crate::TontooUI::animation::Easing;
    const SAMPLES: usize = 30;
    (0..=SAMPLES)
        .map(|i| {
            let at = i as f32 / SAMPLES as f32;
            let y = calculate_y_displacement(at * BOUNCE_SECONDS * 1000.0, 900.0, BOUNCE_PEAK);
            Keyframe::new(at, Transform::offset(0.0, y), Easing::Linear)
        })
        .collect()
}

// ── Tile actions ───────────────────────────────────────────────────────
/// What a click on a tile does.
#[derive(Clone)]
enum TileAction {
    LaunchPad,
    /// Real installed program: click toggles minimize/restore, launches when
    /// not running.
    Real { item: AppItem },
    /// Demo fallback entry: legacy log/spawn behavior.
    Demo { name: String, cmd: Option<String> },
}

#[derive(Clone)]
struct TileDef {
    /// App name, used for the letter fallback.
    name: String,
    /// Localized label of the hover pill.
    label: String,
    item: Option<AppItem>,
    action: TileAction,
}

/// Live dock content, reloaded when pins change.
pub struct DockData {
    /// Pinned programs (without the LaunchPad tile).
    pub dock_apps: Vec<AppItem>,
    /// All installed programs, opened in the LaunchPad.
    pub all_apps: Vec<AppItem>,
    /// Tile icons aligned with the tiles: `[launchpad, app0, app1, ...]`.
    pub icons: Vec<Option<String>>,
    /// Open-program snapshot for the running dots.
    pub running: apps::OpenSnapshot,
    /// Separator tile index (`Some(4)` in demo mode, `None` with real data).
    pub separator_after: Option<usize>,
}

/// A tile in the row: the icon (or letter fallback) inside a `GestureArea`.
type TileView = GestureArea<Animated<ZStack>>;

/// Everything the gesture closures hand back to the app, pumped once per frame.
#[derive(Default)]
struct Shared {
    /// Tile whose tap fired.
    tapped: Option<usize>,
    /// Tile the pointer entered last.
    hovered: Option<usize>,
    /// Long press (or drag) started a drag on this tile.
    drag_from: Option<usize>,
    /// Pointer position in surface coordinates.
    pointer: (f32, f32),
    /// The LaunchPad tile was clicked.
    want_launchpad: bool,
}

/// A drag in flight.
struct DragState {
    /// Row slot the drag started at.
    src_pos: usize,
    /// Drop gap the ghost currently sits in.
    gap: usize,
}

/// The dock surface app.
pub struct Panel {
    data: DockData,
    appearance: Appearance,
    material: Material,
    /// Baked panel backdrop, when generation succeeded.
    backdrop: Option<String>,
    /// Panel tree: backdrop, panel body, hairline, padded tile row.
    tree: ZStack,
    /// Tile index per row child, so hit results survive a row rebuild.
    row_tiles: Vec<usize>,
    /// Running dots, one per row child (`usize::MAX` for the placeholder).
    dots: Vec<(usize, Circle)>,
    /// Drag ghost, present only while dragging.
    ghost: Option<FileImage>,
    /// Drop placeholder shown above the dock, present only while dragging.
    drop_slot: Option<RoundedRectangle>,
    /// Hover label above the dock.
    tooltip: Option<BasicText>,
    shared: Rc<RefCell<Shared>>,
    order: Vec<usize>,
    drag: Option<DragState>,
    tiles: Vec<TileDef>,
    last_dots: f64,
    last_pins: usize,
    last_prewarm: f64,
}

impl Panel {
    /// Build the dock for one output. `appearance` has to be resolved before
    /// this call: icons are cached per mode.
    pub fn new(
        data: DockData,
        appearance: Appearance,
        backdrop: Option<String>,
    ) -> Self {
        let tiles = build_tiles(&data);
        let material = appearance.material();
        let mut panel = Self {
            order: (0..tiles.len()).collect(),
            data,
            appearance,
            material,
            backdrop,
            tree: ZStack::new(),
            row_tiles: Vec::new(),
            dots: Vec::new(),
            ghost: None,
            drop_slot: None,
            tooltip: None,
            shared: Rc::new(RefCell::new(Shared::default())),
            drag: None,
            tiles,
            last_dots: 0.0,
            last_pins: pins::version(),
            // Start the LaunchPad cache prewarm two seconds after launch.
            last_prewarm: 2.0,
        };
        panel.rebuild();
        panel
    }

    /// Panel rect inside the surface: centered horizontally, `GHOST_SIZE`
    /// down from the top so the float gap sits below it.
    fn panel_rect(&self, surface_w: f32) -> (f32, f32, f32, f32) {
        let width = panel_width_for(self.tiles.len());
        (
            (surface_w - width) / 2.0,
            GHOST_SIZE,
            width,
            PANEL_HEIGHT,
        )
    }

    /// Width of one row child: a tile or the placeholder is `ICON_SIZE`, the
    /// separator carries its padding and margin.
    fn child_width(&self, index: usize) -> f32 {
        if index == TILE_SEPARATOR {
            SEPARATOR_PAD + SEPARATOR_WIDTH + SEPARATOR_MARGIN
        } else {
            ICON_SIZE
        }
    }

    /// Total row width, gaps included.
    fn row_width(&self) -> f32 {
        let n = self.row_tiles.len().max(1) as f32;
        let sum: f32 = self.row_tiles.iter().map(|i| self.child_width(*i)).sum();
        sum + ICON_GAP * (n - 1.0)
    }

    /// Left edge of the centered row inside the panel.
    fn row_x(&self, panel_x: f32, panel_w: f32) -> f32 {
        panel_x + (panel_w - self.row_width()) / 2.0
    }

    /// Left edge of the row child at `slot`.
    fn child_x(&self, row_x: f32, slot: usize) -> f32 {
        let mut x = row_x;
        for index in self.row_tiles.iter().take(slot) {
            x += self.child_width(*index) + ICON_GAP;
        }
        x
    }

    /// Row slot of the drag placeholder, if one is in the row.
    fn placeholder_slot(&self) -> Option<usize> {
        self.row_tiles
            .iter()
            .position(|index| *index == TILE_PLACEHOLDER)
    }

    /// Rebuild the row and the panel tree. The dragged tile leaves the row and
    /// a placeholder takes the drop gap, so the row always holds exactly
    /// `order.len()` children and never changes width.
    fn rebuild(&mut self) {
        let mut row = HStack::new().spacing(ICON_GAP).align(Align::Center);
        let mut row_tiles: Vec<usize> = Vec::with_capacity(self.order.len());
        let mut placed = 0usize;
        for slot in 0..self.order.len() {
            if self.drag.as_ref().map(|d| d.gap) == Some(placed) {
                row = row.child(self.placeholder());
                row_tiles.push(TILE_PLACEHOLDER);
            }
            placed += 1;
            if self.drag.as_ref().map(|d| d.src_pos) == Some(slot) {
                continue;
            }
            let index = self.order[slot];
            row = row.child(self.tile_view(index));
            row_tiles.push(index);
            // Demo mode splits apps from system items with a hairline.
            if self.data.separator_after == Some(index) {
                row = row.child(self.separator());
                row_tiles.push(TILE_SEPARATOR);
            }
        }
        if self.drag.as_ref().map(|d| d.gap) == Some(placed) {
            row = row.child(self.placeholder());
            row_tiles.push(TILE_PLACEHOLDER);
        }

        // Running dots follow the row: one per tile child, hidden for the
        // separator, the placeholder and tile 0 (the LaunchPad opener).
        let dots = row_tiles
            .iter()
            .enumerate()
            .map(|(slot, index)| {
                let running = *index < TILE_SEPARATOR
                    && slot > 0
                    && self
                        .tiles
                        .get(*index)
                        .and_then(|tile| tile.item.as_ref())
                        .map(|item| apps::is_running(item, &self.data.running))
                        .unwrap_or(false);
                (*index, Circle::new(DOT_SIZE).fill(self.material.dot_color).filled(running))
            })
            .collect();

        let (px, py, pw, ph) = self.panel_rect(surface_width_for(self.tiles.len()) as f32);
        let mut tree = ZStack::new().align(Align::Center);
        match self.backdrop.clone() {
            // The baked image already carries the tint, radius and shadow, so
            // the body only keeps a hairline more tint on top.
            Some(path) => {
                tree = tree.child(FileImage::new(path, pw, ph).fit(ImageFit::Cover).radius(PANEL_RADIUS));
                tree = tree.child(panel_body(&self.material, pw, ph, true));
            }
            // No backdrop: the fallback gradient keeps the panel from ever
            // reading as black.
            None => {
                tree = tree.child(panel_body(&self.material, pw, ph, false));
            }
        }
        tree = tree.child(panel_border(&self.material, pw, ph));
        tree = tree.child(Padding::all(row, ROW_PAD));
        let _ = (px, py);

        self.tree = tree;
        self.row_tiles = row_tiles;
        self.dots = dots;
    }

    /// The drop placeholder inside the row.
    fn placeholder(&self) -> RoundedRectangle {
        RoundedRectangle::new(ICON_SIZE, ICON_SIZE, PLACEHOLDER_RADIUS)
            .fill(self.material.placeholder_fill)
            .stroke(self.material.placeholder_border)
            .stroke_width(2.0)
            .filled(false)
    }

    /// The app/system separator hairline plus its padding and margin.
    fn separator(&self) -> HStack {
        HStack::new()
            .spacing(0.0)
            .align(Align::Center)
            .child(Spacer::new().min_size(SEPARATOR_PAD))
            .child(
                RoundedRectangle::new(SEPARATOR_WIDTH, PANEL_HEIGHT, 0.0)
                    .fill(self.material.separator)
                    .filled(true),
            )
            .child(Spacer::new().min_size(SEPARATOR_MARGIN))
    }

    /// One tile: the icon (or the letter fallback) wrapped in the bounce
    /// animation, inside a `GestureArea` carrying tap, hover, long press and
    /// drag.
    fn tile_view(&self, index: usize) -> TileView {
        let shared = Rc::clone(&self.shared);
        let tile = &self.tiles[index];
        let icon: ZStack = match self.data.icons.get(index).cloned().flatten() {
            Some(path) => ZStack::new().child(
                FileImage::new(path, ICON_SIZE, ICON_SIZE)
                    .fit(ImageFit::Fit)
                    .radius(TILE_RADIUS),
            ),
            // Generated artwork is missing: the first letter on the app color,
            // the same fallback the GTK dock painted.
            None => ZStack::new()
                .child(RoundedRectangle::new(ICON_SIZE, ICON_SIZE, TILE_RADIUS).fill(tile_color(tile)))
                .child(
                    BasicText::new(tile.letter())
                        .size(LETTER_SIZE)
                        .weight(LETTER_WEIGHT)
                        .alignment(TextAlignment::Center)
                        .foreground_color(Color::WHITE),
                ),
        };
        let anim = Animated::new(icon).keyframes(bounce_keyframes(), BOUNCE_SECONDS);
        let tap_shared = Rc::clone(&shared);
        let hover_shared = Rc::clone(&shared);
        let drag_shared = Rc::clone(&shared);
        // Pick-up is the long press: the `View` trait has no motion hook, so
        // `GestureArea::on_drag` can never fire inside a stack. The press and
        // hold reports the tile, and the app tracks the absolute pointer
        // itself through `App::mouse_move`.
        GestureArea::new(anim)
            .on_tap(move || tap_shared.borrow_mut().tapped = Some(index))
            .on_hover(move |inside| {
                hover_shared.borrow_mut().hovered = if inside { Some(index) } else { None };
            })
            .on_long_press(move || drag_shared.borrow_mut().drag_from = Some(index))
    }

    /// Reload pins, icons and the open-program list when the pin version moved.
    fn refresh_pins(&mut self) {
        if pins::version() == self.last_pins || self.drag.is_some() {
            // A drag in flight defers the rebuild; the version stays new.
            return;
        }
        self.last_pins = pins::version();
        let loaded = apps::load();
        let icons = icons::dock_icons(self.appearance, &loaded.dock);
        let running = apps::open_snapshot();
        let separator_after = if loaded.demo_mode { Some(4) } else { None };
        self.data.dock_apps = loaded.dock;
        self.data.all_apps = loaded.all;
        self.data.icons = icons;
        self.data.running = running;
        self.data.separator_after = separator_after;
        self.tiles = build_tiles(&self.data);
        self.order = (0..self.tiles.len()).collect();
        self.drag = None;
        self.rebuild();
    }

    /// Re-query the open-program list every two seconds and toggle the dots.
    fn refresh_dots(&mut self, now: f64) {
        if now - self.last_dots < 2.0 {
            return;
        }
        self.last_dots = now;
        self.data.running = apps::open_snapshot();
        self.rebuild();
    }

    /// Generate one missing LaunchPad icon cache file per call. Main-thread
    /// only, see [`crate::icons`].
    fn prewarm(&mut self, now: f64) {
        if now - self.last_prewarm < 0.05 {
            return;
        }
        self.last_prewarm = now;
        if !icons::prewarm_step() {
            // Cache is warm: stop asking by pushing the clock far ahead.
            self.last_prewarm = now + 1_000.0;
        }
    }

    /// Replay the bounce on the tile of `index`.
    fn bounce(&mut self, index: usize) {
        let Some(slot) = self.row_tiles.iter().position(|tile| *tile == index) else {
            return;
        };
        // Card padding -> row -> the tile at its row slot. `Padding` holds a
        // single child, so its accessor takes no index.
        let row = self
            .tree
            .child_mut::<Padding>(2)
            .and_then(|padding| padding.child_mut::<HStack>());
        if let Some(tile) = row.and_then(|row| row.child_mut::<TileView>(slot)) {
            tile.child_mut().restart();
        }
    }

    /// Handle a tap on a tile: LaunchPad, toggle windows or demo spawn.
    fn activate(&mut self, index: usize) {
        let Some(tile) = self.tiles.get(index).cloned() else {
            return;
        };
        match tile.action {
            TileAction::LaunchPad => self.shared.borrow_mut().want_launchpad = true,
            TileAction::Real { item } => {
                // Bounce, then toggle: visible windows minimize, minimized
                // ones restore, closed apps launch out-of-process.
                self.bounce(index);
                toggle_app_windows(&item);
            }
            TileAction::Demo { name, cmd } => {
                self.bounce(index);
                match cmd {
                    Some(exe) => match std::process::Command::new(&exe).spawn() {
                        Ok(_) => println!("[dock] launched {exe} ({})", tile.label),
                        Err(err) => eprintln!("[dock] spawn {exe} failed: {err}"),
                    },
                    None => println!("[dock] launch request: {name} ({})", tile.label),
                }
            }
        }
    }

    /// Start a drag for `index` when none is running yet.
    fn begin_drag(&mut self, index: usize) {
        if self.drag.is_some() {
            return;
        }
        let Some(src_pos) = self.order.iter().position(|i| *i == index) else {
            return;
        };
        self.ghost = self
            .data
            .icons
            .get(index)
            .cloned()
            .flatten()
            .map(|path| FileImage::new(path, GHOST_SIZE, GHOST_SIZE).radius(TILE_RADIUS));
        self.drop_slot = Some(
            RoundedRectangle::new(ICON_SIZE, ICON_SIZE, PLACEHOLDER_RADIUS)
                .fill(self.material.placeholder_fill)
                .stroke(self.material.placeholder_border)
                .stroke_width(2.0)
                .filled(false),
        );
        self.drag = Some(DragState {
            src_pos,
            gap: src_pos,
        });
        self.rebuild();
    }

    /// Move the drop gap to follow the pointer.
    fn update_drag(&mut self, row_x: f32) {
        let (pointer_x, _) = self.shared.borrow().pointer;
        let slot = ICON_SIZE + ICON_GAP;
        let tiles = self.order.len() as i32;
        let gap = (((pointer_x - row_x + slot / 2.0) / slot).floor() as i32).clamp(0, tiles) as usize;
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if drag.gap == gap {
            return;
        }
        drag.gap = gap;
        self.rebuild();
    }

    /// Finish a drag: reorder `order` and persist the new pin order.
    fn end_drag(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if drag.gap != drag.src_pos {
            let moved = self.order.remove(drag.src_pos);
            let insert_at = drag.gap.min(self.order.len());
            self.order.insert(insert_at, moved);
            self.persist_order();
        }
        self.ghost = None;
        self.drop_slot = None;
        self.rebuild();
    }

    /// Persist a drag reorder: the LaunchPad tile is fixed, so the pin order
    /// is the remaining tiles.
    fn persist_order(&mut self) {
        let mut pinned: Vec<AppItem> = Vec::new();
        for index in &self.order {
            if *index == 0 {
                continue;
            }
            if let Some(item) = self.tiles.get(*index).and_then(|tile| tile.item.clone()) {
                pinned.push(item);
            }
        }
        if pinned.is_empty() {
            return;
        }
        if pins::save(&pinned) {
            self.last_pins = pins::version();
        }
    }

    /// Pump the gesture closures: taps, hover, drag start and drag motion.
    fn pump_events(&mut self, row_x: f32) {
        let (tapped, hovered, drag_from) = {
            let mut shared = self.shared.borrow_mut();
            (
                shared.tapped.take(),
                shared.hovered,
                shared.drag_from.take(),
            )
        };
        if let Some(index) = tapped {
            self.activate(index);
        }
        if let Some(index) = drag_from {
            self.begin_drag(index);
        }
        self.update_drag(row_x);
        self.update_tooltip(hovered);
    }

    /// Show the hover label for `index`, or hide it.
    fn update_tooltip(&mut self, index: Option<usize>) {
        let label = index
            .filter(|_| self.drag.is_none())
            .and_then(|index| self.tiles.get(index))
            .map(|tile| tile.label.clone());
        let Some(label) = label else {
            self.tooltip = None;
            return;
        };
        match self.tooltip.as_mut() {
            Some(text) => text.set_text(label),
            None => {
                self.tooltip = Some(
                    BasicText::new(label)
                        .size(TOOLTIP_SIZE)
                        .alignment(TextAlignment::Center)
                        .foreground_color(self.material.tooltip_text),
                )
            }
        }
    }

    /// Pill behind the hover label.
    fn tooltip_pill(&self, width: f32, height: f32) -> RoundedRectangle {
        RoundedRectangle::new(width, height, TOOLTIP_RADIUS).fill(self.material.tooltip_bg)
    }
}

impl TileDef {
    /// First letter of the app name, for the fallback tile.
    fn letter(&self) -> String {
        self.name
            .chars()
            .next()
            .unwrap_or('A')
            .to_string()
    }
}

/// Glass panel body: the tinted fill, or the fallback gradient when no baked
/// backdrop is painted under it.
fn panel_body(material: &Material, width: f32, height: f32, has_backdrop: bool) -> RoundedRectangle {
    let shape = RoundedRectangle::new(width, height, PANEL_RADIUS);
    if has_backdrop {
        shape.fill(material.panel_fill)
    } else {
        shape.linear_gradient(
            vec![material.bg_fallback_top, material.bg_fallback_bottom],
            90.0,
        )
    }
}

/// 1 px hairline around the panel body.
fn panel_border(material: &Material, width: f32, height: f32) -> RoundedRectangle {
    RoundedRectangle::new(width, height, PANEL_RADIUS)
        .stroke(material.border_color)
        .stroke_width(BORDER_WIDTH)
        .filled(false)
}

/// App color of a tile, used by the letter fallback.
fn tile_color(tile: &TileDef) -> Color {
    tile.item
        .as_ref()
        .and_then(|item| crate::CoreIcon::Color::from_hex(item.color))
        .map(|c| {
            Color::from_rgb8(
                (c.r * 255.0) as u8,
                (c.g * 255.0) as u8,
                (c.b * 255.0) as u8,
            )
        })
        .unwrap_or(Color::from_rgb8(0x29, 0x79, 0xff))
}

/// Tiles for the current dock content: the LaunchPad opener plus one per
/// pinned program.
fn build_tiles(data: &DockData) -> Vec<TileDef> {
    let mut tiles: Vec<TileDef> = Vec::with_capacity(data.dock_apps.len() + 1);
    tiles.push(TileDef {
        name: "LaunchPad".to_string(),
        label: trk("dock.app.launchpad", "Launchpad"),
        item: None,
        action: TileAction::LaunchPad,
    });
    for item in &data.dock_apps {
        let action = match &item.bundle_path {
            Some(_) => TileAction::Real { item: item.clone() },
            None => TileAction::Demo {
                name: item.display_name.clone(),
                cmd: item.demo_cmd.map(str::to_owned),
            },
        };
        tiles.push(TileDef {
            name: item.display_name.clone(),
            label: trk(
                &format!("dock.app.{}", item.display_name.to_lowercase()),
                &item.display_name,
            ),
            item: Some(item.clone()),
            action,
        });
    }
    tiles
}

/// Minimize the visible windows of `item`, restore the minimized ones, or
/// launch it when it is closed. macOS dock behavior.
fn toggle_app_windows(item: &AppItem) {
    let snapshot = apps::open_snapshot();
    if !snapshot.daemon_ok {
        launch_item(item);
        return;
    }
    let windows = apps::app_windows(item, &snapshot);
    if windows.is_empty() {
        launch_item(item);
        return;
    }
    let provider = CoreWindows::WindowsProvider::from_env();
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

fn launch_item(item: &AppItem) {
    match item.bundle_path.as_deref() {
        Some(bundle_path) => launcher::launch_app(bundle_path, &item.display_name),
        None => println!("[dock] launch request: {}", item.display_name),
    }
}

impl App for Panel {
    fn draw(
        &mut self,
        scene: &mut Scene,
        fonts: &mut FontSystem,
        images: &mut ImageLoader<'_>,
        viewport: Viewport,
        time_secs: f64,
    ) {
        // Live theme: the appearance drives the icon cache variant, so a
        // switch re-resolves the materials and the tile artwork once.
        let (appearance, _text) = theme::poll();
        if appearance != self.appearance {
            self.appearance = appearance;
            self.material = appearance.material();
            self.data.icons = icons::dock_icons(appearance, &self.data.dock_apps);
            self.rebuild();
        }
        self.refresh_pins();
        self.refresh_dots(time_secs);
        self.prewarm(time_secs);

        let (px, py, pw, ph) = self.panel_rect(viewport.width);
        let row_x = self.row_x(px, pw);
        self.pump_events(row_x);

        // Panel tree into the panel rect.
        self.tree.place(fonts, px, py, pw, ph);
        self.tree.draw(scene, fonts, images);

        // Running dots, `DOT_MARGIN` under each icon so they never shift it.
        let dot_y = py + ph / 2.0 + ICON_SIZE / 2.0 + DOT_MARGIN - DOT_SIZE / 2.0;
        // Resolve every x before borrowing the dots mutably.
        let dot_centers: Vec<f32> = (0..self.dots.len())
            .map(|slot| self.child_x(row_x, slot) + ICON_SIZE / 2.0)
            .collect();
        for (slot, (index, dot)) in self.dots.iter_mut().enumerate() {
            if *index >= TILE_SEPARATOR {
                continue;
            }
            dot.place(
                fonts,
                dot_centers[slot] - DOT_SIZE / 2.0,
                dot_y,
                DOT_SIZE,
                DOT_SIZE,
            );
            dot.draw(scene, fonts, images);
        }

        // Drag ghost at the pointer plus the drop placeholder above the dock.
        let (pointer_x, pointer_y) = self.shared.borrow().pointer;
        if self.drag.is_some() {
            if let Some(ghost) = self.ghost.as_mut() {
                ghost.place(
                    fonts,
                    pointer_x - GHOST_SIZE / 2.0,
                    pointer_y - GHOST_SIZE / 2.0,
                    GHOST_SIZE,
                    GHOST_SIZE,
                );
                ghost.draw(scene, fonts, images);
            }
            let gap_x = match self.placeholder_slot() {
                Some(slot) => self.child_x(row_x, slot),
                None => row_x,
            };
            if let Some(placeholder) = self.drop_slot.as_mut() {
                placeholder.place(
                    fonts,
                    gap_x,
                    GHOST_SIZE - ICON_SIZE - 10.0,
                    ICON_SIZE,
                    ICON_SIZE,
                );
                placeholder.draw(scene, fonts, images);
            }
        }

        // Hover label above the panel, on its own pill like the system
        // tooltip the GTK dock used.
        let label_size = self.tooltip.as_mut().map(|label| label.measure(fonts));
        if let Some((w, h)) = label_size {
            let pill_w = w + 2.0 * TOOLTIP_PAD_X;
            let pill_h = h + 2.0 * TOOLTIP_PAD_Y;
            let cx = pointer_x.clamp(px + pill_w / 2.0, (px + pw - pill_w / 2.0).max(px + pill_w / 2.0));
            let top = (pointer_y - pill_h - 10.0).max(2.0);
            let mut pill_shape = self.tooltip_pill(pill_w, pill_h);
            pill_shape.place(fonts, cx - pill_w / 2.0, top, pill_w, pill_h);
            pill_shape.draw(scene, fonts, images);
            if let Some(label) = self.tooltip.as_mut() {
                label.place(fonts, cx - w / 2.0, top + (pill_h - h) / 2.0, w, h);
                label.draw(scene, fonts, images);
            }
        }
    }

    fn mouse_down(&mut self, x: f64, y: f64) {
        self.tree.mouse_down(x, y);
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        // A release while a drag is in flight drops the tile.
        if self.drag.is_some() {
            self.end_drag();
            return;
        }
        self.tree.mouse_up(x, y);
    }

    fn mouse_move(&mut self, x: f64, y: f64) {
        // The tree gets motion through `set_hover`: `View` has no
        // `mouse_move`, so hover is the only per-child pointer position.
        self.shared.borrow_mut().pointer = (x as f32, y as f32);
        self.tree.set_hover(x as f32, y as f32);
    }

    fn input_region(&self) -> Option<(f32, f32, f32, f32)> {
        let width = surface_width_for(self.tiles.len()) as f32;
        if self.drag.is_some() {
            // Dragging: the ghost flies above the panel, so the whole surface
            // has to be clickable again.
            return Some((0.0, 0.0, width, SURFACE_HEIGHT as f32));
        }
        let panel_w = panel_width_for(self.tiles.len());
        Some(((width - panel_w) / 2.0, GHOST_SIZE, panel_w, PANEL_HEIGHT))
    }

    fn poll_overlay(&mut self) -> Option<OverlayRequest> {
        if !self.shared.borrow_mut().want_launchpad {
            return None;
        }
        Some(OverlayRequest {
            app: Box::new(LaunchPadApp::new(
                self.data.all_apps.clone(),
                self.appearance,
            )),
            // Keyboard interactive: the search field has to be typeable, and the
            // compositor focuses the surface while it is mapped.
            options: LayerBarOptions::fullscreen("launchpad").with_keyboard(true),
        })
    }

    fn transparent_body(&self) -> bool {
        // The surface is fully transparent; only the panel paints.
        true
    }

    fn background(&self) -> Color {
        Color::TRANSPARENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_width_follows_the_tile_formula() {
        // One tile (the LaunchPad opener only): 53 + 2 * 20.
        assert_eq!(panel_width_for(1), 93.0);
        // Six tiles: 6 * 53 + 5 * 9 + 2 * 20.
        assert_eq!(panel_width_for(6), 403.0);
        assert_eq!(surface_width_for(6) as f32, 482.0);
    }

    #[test]
    fn surface_geometry_matches_the_dock_constants() {
        assert_eq!(PANEL_HEIGHT, 90.0);
        assert_eq!(SURFACE_HEIGHT, 177);
        assert_eq!(BOTTOM_GAP, 8.0);
        assert_eq!(GHOST_SIZE, 79.0);
        assert_eq!(ICON_SIZE, 53.0);
        assert_eq!(ICON_GAP, 9.0);
        assert_eq!(ROW_PAD, 20.0);
    }

    #[test]
    fn bounce_curve_has_three_decaying_peaks() {
        // First hop peaks at -52 px in the middle of its window.
        let peak = calculate_y_displacement(189.0, 900.0, BOUNCE_PEAK);
        assert!((peak + 52.0).abs() < 0.01, "first peak was {peak}");
        // Second and third hops are 35% and 9% of the first.
        let second = calculate_y_displacement(531.0, 900.0, BOUNCE_PEAK);
        let third = calculate_y_displacement(783.0, 900.0, BOUNCE_PEAK);
        assert!((second + 18.2).abs() < 0.05, "second peak was {second}");
        assert!((third + 4.68).abs() < 0.05, "third peak was {third}");
        // Back at rest at the end.
        assert_eq!(calculate_y_displacement(900.0, 900.0, BOUNCE_PEAK), 0.0);
        assert_eq!(calculate_y_displacement(1200.0, 900.0, BOUNCE_PEAK), 0.0);
    }

    #[test]
    fn bounce_keyframes_cover_the_curve() {
        let keys = bounce_keyframes();
        assert_eq!(keys.len(), 31);
        assert_eq!(keys.first().map(|k| k.at), Some(0.0));
        assert_eq!(keys.last().map(|k| k.at), Some(1.0));
        // Every sample stays inside the first hop amplitude and above rest.
        for key in &keys {
            let y = key.transform.offset.1;
            assert!(y <= 0.0, "bounce went below the panel: {y}");
            assert!(y >= -BOUNCE_PEAK - 0.01, "bounce overshot: {y}");
        }
    }

    #[test]
    fn input_region_leaves_eight_px_below_the_panel() {
        let width = surface_width_for(6) as f32;
        let panel_w = panel_width_for(6);
        let y = GHOST_SIZE;
        let h = PANEL_HEIGHT;
        assert_eq!(y + h, SURFACE_HEIGHT as f32 - BOTTOM_GAP);
        // Centered: the ghost headroom is split evenly on both sides.
        assert_eq!((width - panel_w) / 2.0, GHOST_SIZE / 2.0);
    }

    fn test_item(name: &str, bundle_id: &str) -> AppItem {
        let (color, symbol) = apps::fallback_style(name);
        AppItem {
            display_name: name.to_string(),
            bundle_id: bundle_id.to_string(),
            bundle_path: Some(std::path::PathBuf::from(format!("/Applications/{name}.app"))),
            icon_path: None,
            color,
            symbol,
            demo_cmd: None,
        }
    }

    #[test]
    fn tiles_start_with_the_launchpad_opener() {
        let data = DockData {
            dock_apps: vec![test_item("Terminal", "com.tontoo.terminal")],
            all_apps: Vec::new(),
            icons: Vec::new(),
            running: apps::OpenSnapshot {
                daemon_ok: false,
                windows: Vec::new(),
            },
            separator_after: None,
        };
        let tiles = build_tiles(&data);
        assert_eq!(tiles.len(), 2);
        assert!(matches!(tiles[0].action, TileAction::LaunchPad));
        assert_eq!(tiles[0].label, "Launchpad");
        assert_eq!(tiles[0].letter(), "L");
        assert!(matches!(tiles[1].action, TileAction::Real { .. }));
        assert_eq!(tiles[1].label, "Terminal");
        assert_eq!(tiles[1].letter(), "T");
    }

    #[test]
    fn demo_entries_keep_the_spawn_command() {
        let data = DockData {
            dock_apps: vec![AppItem {
                display_name: "Sliders".to_string(),
                bundle_id: "demo.sliders".to_string(),
                bundle_path: None,
                icon_path: None,
                color: "#0A84FF",
                symbol: crate::CoreIcon::SLIDER_HORIZONTAL_3,
                demo_cmd: Some("vlc"),
            }],
            all_apps: Vec::new(),
            icons: Vec::new(),
            running: apps::OpenSnapshot {
                daemon_ok: false,
                windows: Vec::new(),
            },
            separator_after: Some(4),
        };
        let tiles = build_tiles(&data);
        match &tiles[1].action {
            TileAction::Demo { name, cmd } => {
                assert_eq!(name, "Sliders");
                assert_eq!(cmd.as_deref(), Some("vlc"));
            }
            TileAction::LaunchPad => panic!("expected a demo action, got the launcher"),
            TileAction::Real { .. } => panic!("expected a demo action, got a real program"),
        }
    }
}