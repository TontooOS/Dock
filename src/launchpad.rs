//! LaunchPad: the app grid on its own fullscreen layer surface.
//!
//! The dock opens it through [`crate::panel::Panel::poll_overlay`]; it closes
//! again by returning `WindowCommand::Close`, which drops just this surface
//! and leaves the dock running. Layout mirrors the old GTK window exactly: a
//! `760x520` card centered in a `900x600` surface, so the transparent margin
//! around it is the click-outside area.
//!
//! Elements: `Background` for the card, `SearchField` for the filter,
//! `ScrollView` over a `VStack` of six-column `HStack` rows of
//! `GestureArea` tiles, and a `ContextMenu` for the per-tile actions.

use std::cell::RefCell;
use std::rc::Rc;

use crate::TontooUI::elements::{
    Align, Background, BasicText, ContextKind, ContextMenu, FileImage, Frame, GestureArea,
    HStack, ImageFit, MenuItem, NestedMenu, Padding, RoundedRectangle, ScrollView, SearchField,
    Spacer, TextAlignment, VStack, View, ZStack,
};
use crate::TontooUI::renderer::window::{App, Key, Viewport, WindowCommand};
use crate::TontooUI::renderer::{FontSystem, ImageLoader};
use crate::TontooUI::theme::{GlassAmount, ThemeMode};
use crate::TontooUI::{Color, Scene};
use crate::apps::AppItem;
use crate::icons;
use crate::launcher;
use crate::pins;
use crate::theme::Appearance;

// ── Geometry ───────────────────────────────────────────────────────────
/// Surface size: the transparent margin around the card is the click-outside
/// area, exactly like the old 900x600 window.
pub const SURFACE_WIDTH: u32 = 900;
pub const SURFACE_HEIGHT: u32 = 600;
/// Card size inside the surface.
pub const CARD_WIDTH: f32 = 760.0;
pub const CARD_HEIGHT: f32 = 520.0;
/// Card corner radius.
const CARD_RADIUS: f32 = 16.0;
/// Card inner padding.
const CARD_PAD: f32 = 16.0;
/// Tile box, icon size and label size.
const TILE_BOX: f32 = 72.0;
const TILE_ICON: f32 = 64.0;
const TILE_LABEL: f32 = 11.0;
const TILE_RADIUS: f32 = 16.0;
/// Grid gap and tiles per line.
const GRID_GAP: f32 = 16.0;
const GRID_COLUMNS: usize = 6;
/// Tontoo icon in the header.
const HEADER_ICON: f32 = 36.0;
/// Search field box.
const SEARCH_WIDTH: f32 = 320.0;
const SEARCH_HEIGHT: f32 = 32.0;
/// Separator under the header row.
const DIVIDER_H: f32 = 1.0;

/// Solid search field body: the macOS open-panel look. The glass capsule
/// would sample a window backdrop, which a layer surface does not have.
const SEARCH_FILL_LIGHT: Color = Color::from_rgb8(0xed, 0xed, 0xf0);
const SEARCH_FILL_DARK: Color = Color::from_rgb8(0x1c, 0x1c, 0x1e);

/// A grid tile: the icon (or its letter fallback) plus the name.
type TileView = GestureArea<VStack>;

/// Tile rectangles of the last frame, used to resolve a right-click.
type TileRects = Vec<(f32, f32, f32, f32)>;

/// What the context menu asked for, handed from the menu callback to the app.
#[derive(Clone)]
struct MenuAction {
    item: AppItem,
    /// Row label that was picked.
    label: String,
}

/// The LaunchPad surface app.
pub struct LaunchPadApp {
    entries: Vec<AppItem>,
    appearance: Appearance,
    /// Current filter, owned by the `SearchField` and mirrored here.
    query: Rc<RefCell<String>>,
    /// Entry whose tile was tapped, drained once per frame.
    tap: Rc<RefCell<Option<usize>>>,
    /// Card tree: background, header, divider, scrolling grid.
    tree: ZStack,
    /// The grid, rebuilt whenever the filter changes.
    grid: ScrollView,
    /// Per-tile context menu, rebuilt for the right-clicked tile.
    menu: ContextMenu,
    /// Action the menu queued, drained once per frame.
    action: Rc<RefCell<Option<MenuAction>>>,
    /// Tile the menu belongs to.
    menu_item: Option<AppItem>,
    /// Set when the surface should close.
    close: bool,
    /// Indices whose icon still has to be generated, one per frame.
    pending: Vec<usize>,
    /// Tile rects of the last frame.
    rects: TileRects,
}

impl LaunchPadApp {
    /// Build the grid for `entries` (every installed program; an empty list
    /// falls back to the demo grid).
    pub fn new(entries: Vec<AppItem>, appearance: Appearance) -> Self {
        let entries = if entries.is_empty() {
            crate::apps::demo_launchpad()
        } else {
            entries
        };
        let query = Rc::new(RefCell::new(String::new()));
        let tap = Rc::new(RefCell::new(None));
        let action = Rc::new(RefCell::new(None));
        let grid = ScrollView::new(VStack::new().spacing(GRID_GAP));
        let mut app = Self {
            pending: (0..entries.len()).collect(),
            entries,
            appearance,
            query,
            tap,
            tree: ZStack::new(),
            grid,
            menu: ContextMenu::new(
                (0.0, 0.0, 1.0, 1.0),
                ContextKind::Nested(NestedMenu::new("", Vec::new())),
            ),
            action,
            menu_item: None,
            close: false,
            rects: Vec::new(),
        };
        app.rebuild();
        app
    }

    fn theme_mode(&self) -> ThemeMode {
        match self.appearance {
            Appearance::Light => ThemeMode::Light,
            Appearance::Dark => ThemeMode::Dark,
        }
    }

    fn search_fill(&self) -> Color {
        match self.appearance {
            Appearance::Light => SEARCH_FILL_LIGHT,
            Appearance::Dark => SEARCH_FILL_DARK,
        }
    }

    fn accent(&self) -> Color {
        crate::TontooUI::Color::from_rgb8(0x00, 0x7a, 0xff)
    }

    /// Whether `entry` survives the current filter.
    fn matches(&self, index: usize) -> bool {
        let query = self.query.borrow().to_lowercase();
        if query.is_empty() {
            return true;
        }
        self.entries
            .get(index)
            .map(|entry| entry.display_name.to_lowercase().contains(&query))
            .unwrap_or(false)
    }

    /// Rebuild the grid rows for the current filter.
    fn rebuild(&mut self) {
        let fg = self.appearance.launchpad_fg();
        let visible: Vec<usize> = (0..self.entries.len()).filter(|i| self.matches(*i)).collect();
        let mut rows = VStack::new().spacing(GRID_GAP).align(Align::Center);
        for chunk in visible.chunks(GRID_COLUMNS) {
            let mut row = HStack::new().spacing(GRID_GAP).align(Align::Leading);
            for index in chunk {
                row = row.child(self.tile_view(*index, fg));
            }
            rows = rows.child(row);
        }
        self.grid = ScrollView::new(rows);
        self.build_tree();
    }

    /// Card tree: header (Tontoo icon plus search), divider and the grid.
    fn build_tree(&mut self) {
        let query = Rc::clone(&self.query);
        let mut top = HStack::new().spacing(12.0).align(Align::Center);
        // Tontoo octopus mark on the left.
        let header_icon = match icons::tontoo_icon(!self.appearance.is_dark()) {
            Some(path) => FileImage::new(path, HEADER_ICON, HEADER_ICON),
            None => FileImage::new("", HEADER_ICON, HEADER_ICON),
        };
        top = top.child(header_icon);
        let field = SearchField::new("Applications")
            .fill(self.search_fill())
            .on_change(move |text| {
                *query.borrow_mut() = text.to_string();
            });
        top = top.child(Spacer::new());
        top = top.child(Frame::new(field, SEARCH_WIDTH, SEARCH_HEIGHT));

        let mut content = VStack::new().spacing(12.0).align(Align::Leading);
        content = content.child(top);
        content = content.child(RoundedRectangle::new(CARD_WIDTH - 2.0 * CARD_PAD, DIVIDER_H, 0.0)
            .fill(self.appearance.launchpad_border())
            .filled(true));
        content = content.child(Padding::all(self.grid_take(), CARD_PAD));

        let card = Background::new(content, self.appearance.launchpad_bg()).radius(CARD_RADIUS);
        self.tree = ZStack::new().child(card);
    }

    /// Move the grid out so the card can own it.
    fn grid_take(&mut self) -> ScrollView {
        std::mem::replace(
            &mut self.grid,
            ScrollView::new(VStack::new().spacing(GRID_GAP)),
        )
    }

    /// One grid tile: icon or letter fallback plus the app name.
    fn tile_view(&self, index: usize, fg: Color) -> TileView {
        let Some(entry) = self.entries.get(index) else {
            return GestureArea::new(VStack::new());
        };
        let icon: ZStack = match icons::grid_icon(entry) {
            Some(path) => ZStack::new().child(
                FileImage::new(path, TILE_ICON, TILE_ICON)
                    .fit(ImageFit::Fit)
                    .radius(TILE_RADIUS),
            ),
            // Generated artwork is still missing: the letter on the app color
            // stands in until the cache file lands.
            None => ZStack::new()
                .child(
                    RoundedRectangle::new(TILE_ICON, TILE_ICON, TILE_RADIUS)
                        .fill(app_color(entry)),
                )
                .child(
                    BasicText::new(letter_of(&entry.display_name))
                        .size(20.0)
                        .weight(800.0)
                        .alignment(TextAlignment::Center)
                        .foreground_color(Color::WHITE),
                ),
        };
        let label = BasicText::new(entry.display_name.clone())
            .size(TILE_LABEL)
            .alignment(TextAlignment::Center)
            .width(TILE_BOX)
            .foreground_color(fg);
        let content = VStack::new()
            .spacing(6.0)
            .align(Align::Center)
            .child(Frame::new(icon, TILE_BOX, TILE_BOX))
            .child(label);
        let tap = Rc::clone(&self.tap);
        GestureArea::new(content).on_tap(move || *tap.borrow_mut() = Some(index))
    }

    /// Build the context menu for `item` and open it at the pointer.
    fn open_menu(&mut self, item: AppItem, x: f64, y: f64) {
        let action = Rc::clone(&self.action);
        let pinned = pins::is_pinned(&item.bundle_id);
        let pin_label = if pinned {
            "Remove from Dock".to_string()
        } else {
            "Pin to Dock".to_string()
        };
        let items = vec![
            MenuItem::action("Open"),
            MenuItem::divider(),
            MenuItem::action("Open In Finder"),
            // Only real programs can be pinned; a demo entry has no bundle.
            MenuItem::action(pin_label.clone()),
        ];
        let rows: Vec<String> = items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect();
        let queued = Rc::clone(&action);
        let menu_entry = item.clone();
        let menu = NestedMenu::new("", items).on_action(move |row| {
            let label = row
                .first()
                .and_then(|index| rows.get(*index).cloned())
                .unwrap_or_default();
            *queued.borrow_mut() = Some(MenuAction {
                item: menu_entry.clone(),
                label,
            });
        });
        // The area is the tile box; the panel anchors at the pointer.
        let (fx, fy) = (x as f32, y as f32);
        let area = self
            .rects
            .iter()
            .find(|(rx, ry, rw, rh)| {
                fx >= *rx && fx <= rx + rw && fy >= *ry && fy <= ry + rh
            })
            .copied()
            .unwrap_or((fx - 1.0, fy - 1.0, 2.0, 2.0));
        self.menu = ContextMenu::new(area, ContextKind::Nested(menu));
        self.menu_item = Some(item);
        self.menu.context_click(x, y);
    }

    /// Carry the per-frame theme into every element that follows it.
    fn theme_elements(&mut self) {
        let mode = self.theme_mode();
        let accent = self.accent();
        if let Some(card) = self.tree.child_mut::<Background>(0) {
            if let Some(content) = card.child_mut::<VStack>() {
                if let Some(top) = content.child_mut::<HStack>(0) {
                    if let Some(field) = top.child_mut::<crate::TontooUI::elements::Frame>(2) {
                        if let Some(search) = field.child_mut::<SearchField>() {
                            search.set_theme(mode, accent, GlassAmount::Glass);
                        }
                    }
                }
            }
        }
        self.grid.set_theme(accent, mode == ThemeMode::Dark);
        self.menu.set_theme(accent, mode == ThemeMode::Dark);
        self.menu.set_glass(mode, GlassAmount::Glass);
    }

    /// Tile rectangles of the placed grid, so a right-click resolves to a tile.
    fn collect_rects(&mut self) -> TileRects {
        let mut rects = TileRects::new();
        // The scroll view wraps a single child: the row stack.
        let Some(rows) = self.grid.child_mut::<VStack>() else {
            return rects;
        };
        for row in 0..rows.len() {
            let Some(row_stack) = rows.child_mut::<HStack>(row) else {
                continue;
            };
            for column in 0..row_stack.len() {
                if let Some(tile) = row_stack.child_mut::<TileView>(column) {
                    rects.push(tile.rect());
                }
            }
        }
        rects
    }

    /// Tile index under a point, or `None` outside the grid.
    fn tile_at(&self, x: f64, y: f64) -> Option<usize> {
        let (fx, fy) = (x as f32, y as f32);
        let position = self
            .rects
            .iter()
            .position(|(rx, ry, rw, rh)| fx >= *rx && fx <= rx + rw && fy >= *ry && fy <= ry + rh)?;
        // Rects are collected row by row, so the position maps back through the
        // same filter the rows were built from.
        let mut seen = 0usize;
        for index in 0..self.entries.len() {
            if !self.matches(index) {
                continue;
            }
            if seen == position {
                return Some(index);
            }
            seen += 1;
        }
        None
    }

    /// Run the queued menu action.
    fn run_action(&mut self) {
        let Some(action) = self.action.borrow_mut().take() else {
            return;
        };
        match action.label.as_str() {
            "Open" => {
                if let Some(bundle) = action.item.bundle_path.as_deref() {
                    launcher::launch_app(bundle, &action.item.display_name);
                } else {
                    println!("[launchpad] open {}", action.item.display_name);
                }
                self.close = true;
            }
            "Open In Finder" => {
                if let Some(bundle) = action.item.bundle_path.as_deref() {
                    launcher::open_in_finder(bundle);
                } else {
                    println!(
                        "[launchpad] reveal {} (demo entry, no bundle)",
                        action.item.display_name
                    );
                }
            }
            _ => {
                // Pin to Dock / Remove from Dock: saved per user to CoreData,
                // the running dock rebuilds its row live.
                pins::toggle(&action.item);
            }
        }
    }

    /// Close when the pointer went down outside the card.
    fn click_outside(&mut self, x: f64, y: f64) {
        if self.menu.is_open() {
            return;
        }
        let (cx, cy, cw, ch) = self.card_rect();
        if x < cx as f64 || x > (cx + cw) as f64 || y < cy as f64 || y > (cy + ch) as f64 {
            self.close = true;
        }
    }

    fn card_rect(&self) -> (f32, f32, f32, f32) {
        (
            (SURFACE_WIDTH as f32 - CARD_WIDTH) / 2.0,
            (SURFACE_HEIGHT as f32 - CARD_HEIGHT) / 2.0,
            CARD_WIDTH,
            CARD_HEIGHT,
        )
    }

    /// Generate one missing icon per frame so the grid never blocks.
    fn generate_pending(&mut self) {
        while let Some(index) = self.pending.pop() {
            let Some(entry) = self.entries.get(index) else {
                continue;
            };
            if icons::grid_icon(entry).is_some() {
                // Served from the bundle or the cache already.
                continue;
            }
            let _ = icons::generate_grid_icon(entry);
            // One generation per frame: stop here and continue next frame.
            break;
        }
    }
}

fn letter_of(name: &str) -> String {
    name.chars().next().unwrap_or('A').to_string()
}

fn app_color(entry: &AppItem) -> Color {
    crate::CoreIcon::Color::from_hex(entry.color)
        .map(|c| {
            Color::from_rgb8(
                (c.r * 255.0) as u8,
                (c.g * 255.0) as u8,
                (c.b * 255.0) as u8,
            )
        })
        .unwrap_or(Color::from_rgb8(0x29, 0x79, 0xff))
}

impl App for LaunchPadApp {
    fn draw(
        &mut self,
        scene: &mut Scene,
        fonts: &mut FontSystem,
        images: &mut ImageLoader<'_>,
        viewport: Viewport,
        _time_secs: f64,
    ) {
        self.generate_pending();
        self.rects = self.collect_rects();
        self.theme_elements();

        let (cx, cy, cw, ch) = self.card_rect();
        self.tree.place(fonts, cx, cy, cw, ch);
        self.tree.draw(scene, fonts, images);

        // The menu floats above everything else.
        self.menu
            .set_viewport(viewport.x, viewport.y, viewport.width, viewport.height);
        self.menu.draw(scene, fonts, images);

        self.run_action();
    }

    fn mouse_down(&mut self, x: f64, y: f64) {
        self.click_outside(x, y);
        if self.close {
            return;
        }
        if self.menu.is_open() {
            self.menu.mouse_down(x, y);
            return;
        }
        self.tree.mouse_down(x, y);
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        if self.menu.is_open() {
            self.menu.mouse_up(x, y);
            return;
        }
        self.tree.mouse_up(x, y);
    }

    fn mouse_move(&mut self, x: f64, y: f64) {
        // The tree gets motion through `set_hover`: `View` has no
        // `mouse_move`, so hover is the only per-child pointer position.
        if self.menu.is_open() {
            self.menu.mouse_move(x, y);
            return;
        }
        self.tree.set_hover(x as f32, y as f32);
    }

    fn mouse_wheel(&mut self, dx: f64, dy: f64) {
        if self.menu.is_open() {
            self.menu.mouse_wheel(dx, dy);
            return;
        }
        self.tree.mouse_wheel(dx, dy);
    }

    fn context_click(&mut self, x: f64, y: f64) {
        if let Some(index) = self.tile_at(x, y) {
            if let Some(entry) = self.entries.get(index).cloned() {
                self.open_menu(entry, x, y);
                return;
            }
        }
        // A right-click outside a tile still opens a click-outside close.
        self.click_outside(x, y);
    }

    fn key(&mut self, key: Key) {
        if key == Key::Escape {
            // An open menu handles its own dismissal first.
            if self.menu.is_open() {
                self.menu.close();
                return;
            }
            self.close = true;
        }
    }

    fn text(&mut self, text: &str) {
        // Typing anywhere filters the grid, like the macOS LaunchPad.
        if self.menu.is_open() {
            return;
        }
        let mut query = self.query.borrow_mut();
        query.push_str(text);
        drop(query);
        self.rebuild();
    }

    fn poll_window_command(&mut self) -> Option<WindowCommand> {
        if self.close {
            self.close = false;
            return Some(WindowCommand::Close);
        }
        None
    }

    fn wants_backdrop(&self) -> bool {
        // The menu glass samples the window backdrop while it is open; on a
        // layer surface there is none, so the pass is never requested.
        false
    }

    fn transparent_body(&self) -> bool {
        // Only the card paints; the margin stays see-through for click-outside.
        true
    }

    fn background(&self) -> Color {
        Color::TRANSPARENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, bundle_id: &str, real: bool) -> AppItem {
        let (color, symbol) = crate::apps::fallback_style(name);
        AppItem {
            display_name: name.to_string(),
            bundle_id: bundle_id.to_string(),
            bundle_path: real.then(|| std::path::PathBuf::from(format!("/Applications/{name}.app"))),
            icon_path: None,
            color,
            symbol,
            demo_cmd: None,
        }
    }

    #[test]
    fn card_is_centered_inside_the_surface() {
        let app = LaunchPadApp::new(
            vec![entry("Terminal", "com.tontoo.terminal", true)],
            Appearance::Dark,
        );
        let (x, y, w, h) = app.card_rect();
        assert_eq!((x, y, w, h), (70.0, 40.0, 760.0, 520.0));
        // 70 px of transparent margin left and right for click-outside.
        assert_eq!(x * 2.0, SURFACE_WIDTH as f32 - w);
        assert_eq!(y * 2.0, SURFACE_HEIGHT as f32 - h);
    }

    #[test]
    fn an_empty_list_falls_back_to_the_demo_grid() {
        let app = LaunchPadApp::new(Vec::new(), Appearance::Light);
        assert!(!app.entries.is_empty());
        assert!(app.entries.len() > 20);
        assert!(app.entries.iter().all(|entry| entry.bundle_path.is_none()));
    }

    #[test]
    fn filter_matches_case_insensitively() {
        let app = LaunchPadApp::new(
            vec![
                entry("Terminal", "com.tontoo.terminal", true),
                entry("Weather", "com.tontoo.weather", true),
                entry("Files", "com.tontoo.files", true),
            ],
            Appearance::Dark,
        );
        assert_eq!(app.entries.len(), 3);
        assert!(app.matches(0));
        *app.query.borrow_mut() = "TER".to_string();
        assert!(app.matches(0));
        assert!(!app.matches(1));
        *app.query.borrow_mut() = "e".to_string();
        // Every name contains an "e".
        assert!(app.matches(0) && app.matches(1) && app.matches(2));
        *app.query.borrow_mut() = "zzz".to_string();
        assert!(!app.matches(0));
    }

    #[test]
    fn letters_fall_back_to_the_first_character() {
        assert_eq!(letter_of("Terminal"), "T");
        assert_eq!(letter_of(""), "A");
    }
}