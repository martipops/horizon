//! The controller crafting screen's layout (Horizon's own; model in `legacy_craft.rs`): the
//! calls that draw it, in GUI pixels, and the places a click or the pad can land.
//!
//! Every surface is a vanilla sprite or texture, so a resource pack restyles it with the rest of
//! the GUI: the panel is cut from the crafting table's background, the tabs are the creative
//! screen's and the recipe book's, the recipe slots and the variant column the recipe book's, the
//! selection the game mode switcher's.
//!
//! Two layouts: the full one (category tabs over a panel, mode tabs at its left, the recipe row
//! with its variant column, the grid and the inventory below) and, for views too small for it
//! (a splitscreen view, a small window), a compact one without the mode tabs and the inventory.
use crate::{
    items::ItemStack,
    recipe_book_screen::{Call, Drawn, IDENTITY, ItemCall, Run},
};

/// Vanilla's container title colour (`0x404040`), opaque.
pub const TITLE: i32 = 0xFF40_4040_u32 as i32;
/// `ChatFormatting.DARK_GRAY`, for quiet notes.
pub const NOTE: i32 = 0xFF55_5555_u32 as i32;
/// `ChatFormatting.DARK_RED`, for what's missing.
pub const MISSING: i32 = 0xFFAA_0000_u32 as i32;
/// `ChatFormatting.DARK_GREEN`, for what's there.
pub const HAVE: i32 = 0xFF00_AA00_u32 as i32;
/// The tint over a grid cell whose ingredient the player lacks.
const MISSING_CELL: i32 = 0x96C8_3030_u32 as i32;
/// Over a slot the pointer or the pad is on (`AbstractContainerScreen`'s slot highlight).
const HOVER: i32 = 0x80FF_FFFF_u32 as i32;

/// The full layout's panel, without its tabs.
pub const PANEL_W: i32 = 296;
pub const PANEL_H: i32 = 194;
/// How far the creative tabs stand above the panel.
const TAB_RISE: i32 = 28;
/// The creative tabs' pitch.
const TAB_PITCH: i32 = 27;
/// How far the mode tabs stand out at the panel's left.
const MODE_OUT: i32 = 30;
/// Room kept under the panel for the hint row.
pub const HINT_ROOM: i32 = 14;
/// A recipe slot (`recipe_book/slot_craftable`) and the row's pitch.
const RECIPE: i32 = 25;
/// The compact layout's panel.
const COMPACT_W: i32 = 252;
const COMPACT_H: i32 = 132;

/// One slot of the recipe row or the variant column.
#[derive(Clone, Debug)]
pub struct RecipeSlot {
    pub stack: ItemStack,
    pub craftable: bool,
    /// A recipe the player just learned (`ClientRecipeBook.highlight`).
    pub fresh: bool,
}

/// One cell of the grid preview.
#[derive(Clone, Debug)]
pub struct Cell {
    pub stack: ItemStack,
    pub missing: bool,
}

/// What the screen shows this frame. Text comes translated.
#[derive(Clone, Debug, Default)]
pub struct View {
    /// The view's size in GUI pixels.
    pub width: i32,
    pub height: i32,
    /// The category tabs' icons, the selected one, and the first shown (they page).
    pub tabs: Vec<ItemStack>,
    pub tab: usize,
    pub first_tab: usize,
    /// "2/3" when the tabs page.
    pub tab_page: Option<String>,
    pub title: String,
    /// The search tab: what's typed, or none on another tab; the placeholder when empty.
    pub search: Option<(String, bool)>,
    pub search_hint: String,
    /// The modes at the panel's left (crafting, then the special recipes), the selected one.
    pub modes: Vec<ItemStack>,
    pub mode: usize,
    pub row: Vec<RecipeSlot>,
    pub selected: usize,
    /// The first recipe the row showed last frame: it scrolls only as the selection nears an end.
    pub row_from: usize,
    /// The selected collection's recipes when it has several, and the shown one.
    pub variants: Vec<RecipeSlot>,
    pub variant: usize,
    /// The selected recipe's name, and a line under its grid (colour).
    pub name: String,
    pub note: Option<(String, i32)>,
    /// When the row is empty.
    pub empty: String,
    /// The grid's side (2 or 3), its cells row by row, and the result.
    pub grid: usize,
    pub cells: Vec<Option<Cell>>,
    pub result: Option<RecipeSlot>,
    pub inventory_title: String,
    /// The player's 36 slots: 0-8 the hotbar, 9-35 the rest.
    pub inventory: Vec<Option<ItemStack>>,
    /// What the pointer is over, so it shows highlighted.
    pub hover: Option<Hit>,
    /// Where the pad's focus is, and its cell there (an inventory slot or a grid cell).
    pub focus: Area,
    pub cursor: usize,
}

/// A part of the screen the pad's focus is in: the right stick moves between them, the left stick
/// and the D-pad within one. Stations add their own (`Station(n)`: the station's slots).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Area {
    /// The recipe row and the variant column standing on it.
    #[default]
    Row,
    /// The recipe's grid preview.
    Grid,
    /// The player's 36 slots, a vanilla cursor over them.
    Inventory,
}

/// A place on the screen that does something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    TabPrev,
    TabNext,
    Mode(usize),
    Row(usize),
    Variant(usize),
    VariantUp,
    VariantDown,
    Result,
    Search,
    Inventory(usize),
    /// A cell of the grid preview, row by row.
    Cell(usize),
}

/// The drawn screen.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub calls: Vec<Drawn>,
    /// Where each `Hit` is: x, y, width, height.
    pub hits: Vec<([i32; 4], Hit)>,
    /// The panel with its tabs, so a click outside it can close the screen.
    pub bounds: [i32; 4],
    /// The baseline of the hint row under the panel.
    pub hint_y: i32,
    pub compact: bool,
    /// The first recipe the row shows.
    pub row_from: usize,
    /// Each area there is, and where.
    pub areas: Vec<(Area, [i32; 4])>,
}

impl Layout {
    /// Where a hit is.
    pub fn rect_of(&self, hit: Hit) -> Option<[i32; 4]> {
        self.hits.iter().find(|(_, h)| *h == hit).map(|(r, _)| *r)
    }

    /// The area a flick of the right stick goes to from `from` (a point in area `area`) in
    /// direction `dir` (x right, y down): the nearest one that way, its nearest point counting,
    /// with a sideways step weighing twice a forward one. None when nothing lies that way.
    pub fn area_toward(&self, area: Area, from: [i32; 2], dir: [i32; 2]) -> Option<Area> {
        self.areas
            .iter()
            .filter(|(a, _)| *a != area)
            .filter_map(|(a, [x, y, w, h])| {
                let near = [from[0].clamp(*x, x + w - 1), from[1].clamp(*y, y + h - 1)];
                let (dx, dy) = (near[0] - from[0], near[1] - from[1]);
                let along = dx * dir[0] + dy * dir[1];
                let across = (dx * dir[1] - dy * dir[0]).abs();
                (along > 0 && across <= along * 3).then_some((along + 2 * across, *a))
            })
            .min_by_key(|(score, _)| *score)
            .map(|(_, a)| a)
    }

    /// The cell of `area` nearest a point: an inventory slot or a grid cell.
    pub fn nearest_cell(&self, area: Area, at: [i32; 2]) -> Option<usize> {
        let distance = |[x, y, w, h]: [i32; 4]| {
            let (cx, cy) = (x + w / 2, y + h / 2);
            (cx - at[0]).pow(2) + (cy - at[1]).pow(2)
        };
        self.hits
            .iter()
            .filter_map(|(r, hit)| match (area, hit) {
                (Area::Inventory, Hit::Inventory(i)) | (Area::Grid, Hit::Cell(i)) => Some((*r, *i)),
                _ => None,
            })
            .min_by_key(|(r, _)| distance(*r))
            .map(|(_, i)| i)
    }
}

/// `text` cut to `room` pixels, with "..." where it was cut.
pub fn fit(text: &str, room: i32, width: &dyn Fn(&str) -> i32) -> String {
    if width(text) <= room {
        return text.to_owned();
    }
    let mut cut = text.to_owned();
    while cut.pop().is_some() {
        let shown = format!("{}...", cut.trim_end());
        if width(&shown) <= room {
            return shown;
        }
    }
    String::new()
}

/// The full layout fits.
pub fn full_fits(width: i32, height: i32) -> bool {
    width >= PANEL_W + 2 * MODE_OUT + 4 && height >= TAB_RISE + PANEL_H + HINT_ROOM
}

/// How many category tabs show at once.
pub fn tabs_shown(width: i32, height: i32) -> usize {
    let room = if full_fits(width, height) {
        PANEL_W
    } else {
        COMPACT_W - 32
    };
    (room / TAB_PITCH) as usize
}

/// How many recipes the row shows at once.
pub fn row_shown(width: i32, height: i32) -> usize {
    if full_fits(width, height) { 11 } else { 9 }
}

/// The first recipe the row shows, keeping the selection in sight with one slot to spare.
pub fn row_first(len: usize, selected: usize, shown: usize, first: usize) -> usize {
    if len <= shown {
        return 0;
    }
    let last_first = len - shown;
    let first = first.min(last_first);
    if selected < first + 1 {
        selected.saturating_sub(1)
    } else if selected + 2 > first + shown {
        (selected + 2 - shown).min(last_first)
    } else {
        first
    }
}

struct Out {
    calls: Vec<Drawn>,
    hits: Vec<([i32; 4], Hit)>,
}
impl Out {
    fn push(&mut self, call: Call) {
        self.calls.push(Drawn {
            call,
            pose: IDENTITY,
        });
    }
    fn sprite(&mut self, id: &str, x: i32, y: i32, w: i32, h: i32) {
        self.push(Call::Sprite {
            id: id.to_owned(),
            x,
            y,
            w,
            h,
        });
    }
    fn blit(&mut self, texture: &'static str, x: i32, y: i32, w: i32, h: i32, u: f32, v: f32) {
        self.push(Call::Blit {
            texture,
            x,
            y,
            w,
            h,
            u,
            v,
            tw: 256,
            th: 256,
        });
    }
    /// `region` (u, v, w, h) of `texture` repeated over `dest`, cut at its right and bottom.
    fn tile(&mut self, texture: &'static str, region: [i32; 4], dest: [i32; 4]) {
        let [u, v, rw, rh] = region;
        let [x, y, w, h] = dest;
        let mut ty = 0;
        while ty < h {
            let th = rh.min(h - ty);
            let mut tx = 0;
            while tx < w {
                let tw = rw.min(w - tx);
                self.blit(texture, x + tx, y + ty, tw, th, u as f32, v as f32);
                tx += tw;
            }
            ty += th;
        }
    }
    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, color: i32) {
        self.push(Call::Fill {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
            color,
        });
    }
    fn text(&mut self, text: &str, x: i32, y: i32, color: i32) {
        self.push(Call::Text {
            runs: vec![Run::plain(text)],
            x,
            y,
            color,
            shadow: false,
        });
    }
    fn item(&mut self, stack: &ItemStack, x: i32, y: i32, count: bool) {
        self.push(Call::Item {
            call: ItemCall::Item,
            stack: stack.clone(),
            x,
            y,
        });
        if count {
            self.push(Call::Item {
                call: ItemCall::Decorations,
                stack: stack.clone(),
                x,
                y,
            });
        }
    }
    fn hit(&mut self, rect: [i32; 4], hit: Hit) {
        self.hits.push((rect, hit));
    }
}

/// `width`: how wide a text is in the GUI font.
pub fn layout(view: &View, width: &dyn Fn(&str) -> i32) -> Layout {
    if full_fits(view.width, view.height) {
        full(view, width)
    } else {
        compact(view, width)
    }
}

/// The panel: a nine slice cut from the crafting table's background (`gui/container/crafting_table`,
/// 176x166), so a pack's container look carries over. Corners 4 pixels; the edges and the inside
/// tile blank stretches of it (above the grid, where vanilla draws only the title).
fn panel(out: &mut Out, x: i32, y: i32, w: i32, h: i32) {
    const T: &str = "textures/gui/container/crafting_table.png";
    const B: i32 = 4;
    const SW: i32 = 176;
    const SH: i32 = 166;
    let (ix, iy, iw, ih) = (x + B, y + B, w - 2 * B, h - 2 * B);
    out.blit(T, x, y, B, B, 0.0, 0.0);
    out.blit(T, x + w - B, y, B, B, (SW - B) as f32, 0.0);
    out.blit(T, x, y + h - B, B, B, 0.0, (SH - B) as f32);
    out.blit(
        T,
        x + w - B,
        y + h - B,
        B,
        B,
        (SW - B) as f32,
        (SH - B) as f32,
    );
    out.tile(T, [B, 0, 152, B], [ix, y, iw, B]);
    out.tile(T, [B, SH - B, 152, B], [ix, y + h - B, iw, B]);
    out.tile(T, [0, B, B, 60], [x, iy, B, ih]);
    out.tile(T, [SW - B, B, B, 60], [x + w - B, iy, B, ih]);
    out.tile(T, [B, B, 160, 10], [ix, iy, iw, ih]);
}

/// A sunken frame: the slot sprite's one-pixel bevel round an area of the panel's colour.
fn inset(out: &mut Out, x: i32, y: i32, w: i32, h: i32) {
    const DARK: i32 = 0xFF37_3737_u32 as i32;
    const LIGHT: i32 = 0xFFFF_FFFF_u32 as i32;
    out.fill(x, y, w - 1, 1, DARK);
    out.fill(x, y, 1, h - 1, DARK);
    out.fill(x + 1, y + h - 1, w - 1, 1, LIGHT);
    out.fill(x + w - 1, y + 1, 1, h - 1, LIGHT);
}

fn recipe_slot(out: &mut Out, slot: &RecipeSlot, x: i32, y: i32) {
    let sprite = if slot.craftable {
        "recipe_book/slot_craftable"
    } else {
        "recipe_book/slot_uncraftable"
    };
    out.sprite(sprite, x, y, RECIPE, RECIPE);
    out.item(&slot.stack, x + 4, y + 4, slot.stack.count > 1);
    if slot.fresh {
        // The recipe book's toast mark for a new recipe: a small star in the corner.
        out.fill(x + RECIPE - 6, y + 2, 3, 3, 0xFFFF_FF55_u32 as i32);
    }
}

fn selection(out: &mut Out, x: i32, y: i32) {
    out.sprite("gamemode_switcher/selection", x, y, 26, 26);
}

fn creative_tabs(out: &mut Out, view: &View, x: i32, y: i32, shown: usize, front: bool) {
    let end = (view.first_tab + shown).min(view.tabs.len());
    for (column, i) in (view.first_tab..end).enumerate() {
        let selected = i == view.tab;
        if selected != front {
            continue;
        }
        // Vanilla's sprites by column: 1 sits flush with the panel's left edge.
        let n = if column == 0 { 1 } else { 2 };
        let state = if selected { "selected" } else { "unselected" };
        let tx = x + column as i32 * TAB_PITCH;
        out.sprite(
            &format!("container/creative_inventory/tab_top_{state}_{n}"),
            tx,
            y - TAB_RISE,
            26,
            32,
        );
        out.item(
            &view.tabs[i],
            tx + 5,
            y - TAB_RISE + if selected { 8 } else { 9 },
            false,
        );
        out.hit([tx, y - TAB_RISE, 26, TAB_RISE], Hit::Tab(i));
    }
}

fn tab_pager(
    out: &mut Out,
    view: &View,
    x: i32,
    y: i32,
    shown: usize,
    width: &dyn Fn(&str) -> i32,
) {
    let Some(page) = &view.tab_page else {
        return;
    };
    let right = x + shown as i32 * TAB_PITCH + 2;
    out.sprite("widget/page_backward", x - 26, y - 20, 23, 13);
    out.hit([x - 26, y - 20, 23, 13], Hit::TabPrev);
    out.sprite("widget/page_forward", right, y - 20, 23, 13);
    out.hit([right, y - 20, 23, 13], Hit::TabNext);
    let _ = width;
    out.push(Call::Text {
        runs: vec![Run::plain(page.clone())],
        x: right + 26,
        y: y - 17,
        color: -1,
        shadow: true,
    });
}

fn mode_tabs(out: &mut Out, view: &View, x: i32, y: i32, front: bool) {
    for (i, icon) in view.modes.iter().enumerate() {
        let on = i == view.mode;
        if on != front {
            continue;
        }
        let tx = x - if on { MODE_OUT + 2 } else { MODE_OUT };
        let ty = y + 4 + i as i32 * 27;
        let sprite = if on {
            "recipe_book/tab_selected"
        } else {
            "recipe_book/tab"
        };
        out.sprite(sprite, tx, ty, 35, 27);
        out.item(icon, tx + if on { 8 } else { 10 }, ty + 5, false);
        out.hit([tx, ty, MODE_OUT, 27], Hit::Mode(i));
    }
}

fn title_or_search(out: &mut Out, view: &View, cx: i32, y: i32, width: &dyn Fn(&str) -> i32) {
    match &view.search {
        Some((query, focused)) => {
            const W: i32 = 120;
            let x = cx - W / 2;
            out.sprite(
                if *focused {
                    "widget/text_field_highlighted"
                } else {
                    "widget/text_field"
                },
                x,
                y - 3,
                W,
                14,
            );
            if query.is_empty() {
                out.text(&view.search_hint, x + 4, y, 0xFF80_8080_u32 as i32);
            } else {
                out.push(Call::Text {
                    runs: vec![Run::plain(query.clone())],
                    x: x + 4,
                    y,
                    color: 0xFFE0_E0E0_u32 as i32,
                    shadow: true,
                });
            }
            out.hit([x, y - 3, W, 14], Hit::Search);
        }
        None => {
            let title = fit(&view.title, PANEL_W - 80, width);
            out.text(&title, cx - width(&title) / 2, y, TITLE);
        }
    }
}

/// The row and the variant column standing on the selected slot.
fn row(out: &mut Out, view: &View, x: i32, y: i32, shown: usize, first: usize) {
    let end = (first + shown).min(view.row.len());
    let column = view.variants.len() > 1;
    for i in first..end {
        let sx = x + (i - first) as i32 * RECIPE;
        out.hit([sx, y, RECIPE, RECIPE], Hit::Row(i));
        if i == view.selected && column {
            continue;
        }
        recipe_slot(out, &view.row[i], sx, y);
        if view.hover == Some(Hit::Row(i)) && i != view.selected {
            out.fill(sx + 4, y + 4, 16, 16, HOVER);
        }
    }
    if view.selected < first || view.selected >= end {
        return;
    }
    let sx = x + (view.selected - first) as i32 * RECIPE;
    if !column {
        selection(out, sx, y);
        return;
    }
    // Vanilla's recipe-collection popup, upright: one recipe above and below the shown one.
    let top = y - RECIPE - 9;
    out.sprite(
        "recipe_book/overlay_recipe",
        sx - 3,
        top,
        31,
        RECIPE * 3 + 19,
    );
    let n = view.variants.len();
    let above = (view.variant + n - 1) % n;
    let below = (view.variant + 1) % n;
    out.sprite("transferable_list/move_up", sx - 4, top - 10, 32, 32);
    out.hit([sx, top, RECIPE, 8], Hit::VariantUp);
    for (slot, at, hit) in [
        (above, y - RECIPE, Hit::VariantUp),
        (below, y + RECIPE, Hit::VariantDown),
    ] {
        recipe_slot(out, &view.variants[slot], sx, at);
        out.hit([sx, at, RECIPE, RECIPE], hit);
    }
    out.sprite(
        "transferable_list/move_down",
        sx - 4,
        y + 2 * RECIPE - 12,
        32,
        32,
    );
    out.hit([sx, y + 2 * RECIPE + 1, RECIPE, 8], Hit::VariantDown);
    recipe_slot(out, &view.variants[view.variant], sx, y);
    selection(out, sx, y);
}

/// The grid preview, the arrow and the result, from the grid's top left.
fn grid(out: &mut Out, view: &View, x: i32, y: i32) {
    let side = view.grid.max(1) as i32;
    // A 2x2 grid sits in the middle of the 3x3 room.
    let (gx, gy) = if side == 2 { (x + 9, y + 9) } else { (x, y) };
    for i in 0..(side * side) as usize {
        let cx = gx + (i as i32 % side) * 18;
        let cy = gy + (i as i32 / side) * 18;
        out.sprite("container/slot", cx, cy, 18, 18);
        let focused = view.focus == Area::Grid && view.cursor == i;
        if focused {
            highlight_back(out, cx, cy);
        }
        if let Some(Some(cell)) = view.cells.get(i) {
            if cell.missing {
                out.fill(cx + 1, cy + 1, 16, 16, MISSING_CELL);
            }
            out.item(&cell.stack, cx + 1, cy + 1, false);
        }
        if focused {
            highlight_front(out, cx, cy);
        }
        out.hit([cx, cy, 18, 18], Hit::Cell(i));
    }
    // The crafting table's arrow (its background at 90,35).
    out.blit(
        "textures/gui/container/crafting_table.png",
        x + 57,
        y + 19,
        22,
        15,
        90.0,
        35.0,
    );
    if let Some(result) = &view.result {
        recipe_slot(out, result, x + 82, y + 14);
        out.hit([x + 82, y + 14, RECIPE, RECIPE], Hit::Result);
    }
}

/// Vanilla's slot highlight (`container/slot_highlight_back` behind the item, `_front` over it,
/// 24 px round an 18 px slot), as the pointer over a slot shows it.
fn highlight_back(out: &mut Out, x: i32, y: i32) {
    out.sprite("container/slot_highlight_back", x - 3, y - 3, 24, 24);
}
fn highlight_front(out: &mut Out, x: i32, y: i32) {
    out.sprite("container/slot_highlight_front", x - 3, y - 3, 24, 24);
}

fn inventory(out: &mut Out, view: &View, x: i32, y: i32, width: &dyn Fn(&str) -> i32) {
    let t = &view.inventory_title;
    out.text(t, x + 84 - width(t) / 2, y + 4, TITLE);
    for i in 0..36usize {
        // The rest of the inventory first, the hotbar under it, as vanilla's menus lay them out.
        let (row, col) = if i < 9 {
            (3, i)
        } else {
            ((i - 9) / 9, (i - 9) % 9)
        };
        let sx = x + 3 + col as i32 * 18;
        let sy = y + 14 + row as i32 * 18 + if row == 3 { 4 } else { 0 };
        out.sprite("container/slot", sx, sy, 18, 18);
        let focused = view.focus == Area::Inventory && view.cursor == i;
        if focused {
            highlight_back(out, sx, sy);
        }
        if let Some(Some(stack)) = view.inventory.get(i) {
            out.item(stack, sx + 1, sy + 1, true);
        }
        if focused {
            highlight_front(out, sx, sy);
        }
        out.hit([sx, sy, 18, 18], Hit::Inventory(i));
    }
}

fn full(view: &View, width: &dyn Fn(&str) -> i32) -> Layout {
    let mut out = Out {
        calls: Vec::new(),
        hits: Vec::new(),
    };
    let (w, h) = (PANEL_W, PANEL_H);
    // The mode tabs show only with a choice to make; without them the panel sits in the middle.
    let modes = view.modes.len() > 1;
    let x = (view.width - w) / 2 + if modes { MODE_OUT / 2 } else { 0 };
    let y = TAB_RISE.max((view.height - (TAB_RISE + h + HINT_ROOM)) / 2 + TAB_RISE);
    let shown = tabs_shown(view.width, view.height);
    creative_tabs(&mut out, view, x, y, shown, false);
    if modes {
        mode_tabs(&mut out, view, x, y, false);
    }
    panel(&mut out, x, y, w, h);
    creative_tabs(&mut out, view, x, y, shown, true);
    if modes {
        mode_tabs(&mut out, view, x, y, true);
    }
    tab_pager(&mut out, view, x, y, shown, width);
    title_or_search(&mut out, view, x + w / 2, y + 7, width);
    let row_y = y + 36;
    let row_x = x + 8;
    let shown_row = row_shown(view.width, view.height);
    let first = row_first(view.row.len(), view.selected, shown_row, view.row_from);
    if view.row.is_empty() {
        out.text(
            &view.empty,
            x + w / 2 - width(&view.empty) / 2,
            row_y + 9,
            NOTE,
        );
    } else {
        let pos = format!("{}/{}", view.selected + 1, view.row.len());
        out.text(&pos, x + w - 9 - width(&pos), row_y + 28, NOTE);
    }
    // The lower half: the recipe on the left, the inventory on the right.
    let low = y + 97;
    let low_h = h - 103;
    inset(&mut out, x + 6, low, 112, low_h);
    inset(&mut out, x + 122, low, 168, low_h);
    if !view.row.is_empty() {
        const ROOM: i32 = 104;
        let name = fit(&view.name, ROOM, width);
        out.text(&name, x + 6 + 56 - width(&name) / 2, low + 4, TITLE);
        grid(&mut out, view, x + 10, low + 16);
        if let Some((note, color)) = &view.note {
            let note = fit(note, ROOM, width);
            out.text(&note, x + 6 + 56 - width(&note) / 2, low + 76, *color);
        }
    }
    inventory(&mut out, view, x + 122, low, width);
    // Drawn last: the column stands over the title's corner and the lower half's frame.
    let mut areas = vec![(Area::Inventory, [x + 122, low, 168, low_h])];
    if !view.row.is_empty() {
        row(&mut out, view, row_x, row_y, shown_row, first);
        let visible = (view.row.len() - first).min(shown_row) as i32;
        areas.push((Area::Row, [row_x, row_y, visible * RECIPE, RECIPE]));
        areas.push((Area::Grid, [x + 6, low, 112, low_h]));
    }
    let out_at = if modes { MODE_OUT + 2 } else { 0 };
    Layout {
        calls: out.calls,
        hits: out.hits,
        bounds: [x - out_at, y - TAB_RISE, w + out_at, h + TAB_RISE],
        hint_y: y + h + 4,
        compact: false,
        row_from: first,
        areas,
    }
}

fn compact(view: &View, width: &dyn Fn(&str) -> i32) -> Layout {
    let mut out = Out {
        calls: Vec::new(),
        hits: Vec::new(),
    };
    let (w, h) = (COMPACT_W.min(view.width - 4), COMPACT_H);
    let x = (view.width - w) / 2;
    let y = TAB_RISE.max((view.height - (TAB_RISE + h + HINT_ROOM)) / 2 + TAB_RISE);
    let shown = tabs_shown(view.width, view.height);
    // The mode, as a slot left of the tabs, when there's a choice of them.
    if view.modes.len() > 1 {
        out.sprite("container/slot", x + 4, y - 22, 18, 18);
        if let Some(icon) = view.modes.get(view.mode) {
            out.item(icon, x + 5, y - 21, false);
        }
        out.hit([x + 4, y - 22, 18, 18], Hit::Mode(view.mode));
    }
    creative_tabs(&mut out, view, x + 32, y, shown, false);
    panel(&mut out, x, y, w, h);
    creative_tabs(&mut out, view, x + 32, y, shown, true);
    title_or_search(&mut out, view, x + w / 2, y + 6, width);
    let row_y = y + 36;
    let shown_row = row_shown(view.width, view.height);
    let row_x = x + 1 + (w - shown_row as i32 * RECIPE) / 2;
    let first = row_first(view.row.len(), view.selected, shown_row, view.row_from);
    if view.row.is_empty() {
        out.text(
            &view.empty,
            x + w / 2 - width(&view.empty) / 2,
            row_y + 9,
            NOTE,
        );
    } else {
        let column_end = row_x + (view.selected.saturating_sub(first) as i32 + 1) * RECIPE + 4;
        let tx = column_end.max(row_x + 3 * RECIPE + 4).min(x + w - 120);
        let pos = format!("{}/{}", view.selected + 1, view.row.len());
        let pos_x = x + w - 8 - width(&pos);
        out.text(
            &fit(&view.name, pos_x - tx - 6, width),
            tx,
            row_y + 30,
            TITLE,
        );
        out.text(&pos, pos_x, row_y + 30, NOTE);
        grid(&mut out, view, tx, row_y + 40);
        if let Some((note, color)) = &view.note {
            // Under the result, clear of the grid.
            let note = fit(note, x + w - 6 - (tx + 58), width);
            out.text(&note, tx + 58, row_y + 84, *color);
        }
        row(&mut out, view, row_x, row_y, shown_row, first);
    }
    // No inventory here: small views have no room for it (the vanilla screen, by L3, has one).
    let mut areas = Vec::new();
    if !view.row.is_empty() {
        let visible = (view.row.len() - first).min(shown_row) as i32;
        areas.push((Area::Row, [row_x, row_y, visible * RECIPE, RECIPE]));
        if let Some(r) = out.hits.iter().find(|(_, h)| *h == Hit::Cell(0)).map(|(r, _)| *r) {
            areas.push((Area::Grid, [r[0], r[1], 54, 54]));
        }
    }
    Layout {
        calls: out.calls,
        hits: out.hits,
        bounds: [x, y - TAB_RISE, w, h + TAB_RISE],
        hint_y: y + h + 3,
        compact: true,
        row_from: first,
        areas,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(id: &str) -> ItemStack {
        ItemStack::new(id, 1)
    }
    fn slot(id: &str) -> RecipeSlot {
        RecipeSlot {
            stack: stack(id),
            craftable: true,
            fresh: false,
        }
    }
    fn view(width: i32, height: i32) -> View {
        View {
            width,
            height,
            tabs: (0..10).map(|_| stack("minecraft:bricks")).collect(),
            modes: vec![stack("minecraft:crafting_table")],
            row: (0..30).map(|_| slot("minecraft:stick")).collect(),
            selected: 2,
            variants: vec![slot("minecraft:oak_planks"), slot("minecraft:birch_planks")],
            grid: 3,
            cells: vec![None; 9],
            result: Some(slot("minecraft:oak_planks")),
            inventory: vec![None; 36],
            ..View::default()
        }
    }
    fn width(s: &str) -> i32 {
        s.chars().count() as i32 * 6
    }
    fn inside(outer: [i32; 4], inner: [i32; 4]) -> bool {
        inner[0] >= outer[0]
            && inner[1] >= outer[1]
            && inner[0] + inner[2] <= outer[0] + outer[2]
            && inner[1] + inner[3] <= outer[1] + outer[3]
    }

    #[test]
    fn auto_scale_views_get_the_full_layout_and_small_ones_the_compact() {
        // 1280x720, 854x480 and 2560x1440 at automatic GUI scale are all 427x240.
        assert!(full_fits(427, 240));
        assert!(full_fits(480, 270));
        assert!(!full_fits(320, 180));
        assert!(!full_fits(427, 200));
    }

    #[test]
    fn every_place_stays_on_the_view() {
        for (w, h) in [(427, 240), (480, 270), (320, 240), (320, 180), (640, 360)] {
            let l = layout(&view(w, h), &width);
            for (rect, hit) in &l.hits {
                assert!(
                    inside([0, 0, w, h], *rect),
                    "{hit:?} {rect:?} off a {w}x{h} view"
                );
            }
            assert!(l.hint_y + 9 <= h, "hint row off a {w}x{h} view");
        }
    }

    #[test]
    fn row_slots_never_overlap_and_the_selection_is_shown() {
        for (w, h) in [(427, 240), (320, 180)] {
            for selected in [0, 1, 9, 10, 15, 28, 29] {
                let mut v = view(w, h);
                v.selected = selected;
                let l = layout(&v, &width);
                let rows: Vec<[i32; 4]> = l
                    .hits
                    .iter()
                    .filter(|(_, h)| matches!(h, Hit::Row(_)))
                    .map(|(r, _)| *r)
                    .collect();
                for (a, b) in rows.iter().zip(rows.iter().skip(1)) {
                    assert!(a[0] + a[2] <= b[0]);
                }
                assert!(
                    l.hits.iter().any(|(_, h)| *h == Hit::Row(selected)),
                    "selection {selected} scrolled out at {w}x{h}"
                );
            }
        }
    }

    #[test]
    fn the_row_scrolls_by_one_slot_at_its_ends() {
        assert_eq!(row_first(30, 0, 11, 0), 0);
        assert_eq!(row_first(30, 9, 11, 0), 0);
        assert_eq!(row_first(30, 10, 11, 0), 1);
        assert_eq!(row_first(30, 29, 11, 0), 19);
        assert_eq!(row_first(30, 5, 11, 19), 4);
        assert_eq!(row_first(5, 4, 11, 0), 0);
    }

    #[test]
    fn a_single_recipe_gets_no_variant_column() {
        let mut v = view(427, 240);
        v.variants.clear();
        let l = layout(&v, &width);
        assert!(
            !l.hits
                .iter()
                .any(|(_, h)| matches!(h, Hit::VariantUp | Hit::VariantDown))
        );
        let v = view(427, 240);
        let l = layout(&v, &width);
        assert!(l.hits.iter().any(|(_, h)| *h == Hit::VariantDown));
    }

    #[test]
    fn the_right_stick_goes_to_the_area_that_way_from_where_the_focus_is() {
        let l = layout(&view(427, 240), &width);
        let centre = |hit| {
            let [x, y, w, h] = l.rect_of(hit).unwrap();
            [x + w / 2, y + h / 2]
        };
        // Down from a recipe at the row's left: the grid under it; from its right: the inventory.
        assert_eq!(l.area_toward(Area::Row, centre(Hit::Row(1)), [0, 1]), Some(Area::Grid));
        assert_eq!(l.area_toward(Area::Row, centre(Hit::Row(10)), [0, 1]), Some(Area::Inventory));
        // Across the lower half, and back up to the row.
        assert_eq!(l.area_toward(Area::Grid, centre(Hit::Cell(4)), [1, 0]), Some(Area::Inventory));
        assert_eq!(l.area_toward(Area::Inventory, centre(Hit::Inventory(13)), [-1, 0]), Some(Area::Grid));
        assert_eq!(l.area_toward(Area::Inventory, centre(Hit::Inventory(13)), [0, -1]), Some(Area::Row));
        // Nothing below the hotbar, nothing left of the grid.
        assert_eq!(l.area_toward(Area::Inventory, centre(Hit::Inventory(4)), [0, 1]), None);
        assert_eq!(l.area_toward(Area::Grid, centre(Hit::Cell(3)), [-1, 0]), None);
    }

    #[test]
    fn entering_an_area_lands_on_its_nearest_cell() {
        let l = layout(&view(427, 240), &width);
        let [x, y, ..] = l.rect_of(Hit::Inventory(0)).unwrap();
        assert_eq!(l.nearest_cell(Area::Inventory, [x + 2, y + 30]), Some(0));
        let [x, y, w, _] = l.rect_of(Hit::Row(10)).unwrap();
        // Straight down from the row's right end: the inventory's top row, not the hotbar.
        let cell = l.nearest_cell(Area::Inventory, [x + w / 2, y + 60]).unwrap();
        assert!((9..18).contains(&cell), "{cell}");
    }

    #[test]
    fn the_inventory_isnt_an_area_of_the_compact_layout() {
        let l = layout(&view(320, 180), &width);
        assert!(l.areas.iter().all(|(a, _)| *a != Area::Inventory));
        assert!(l.areas.iter().any(|(a, _)| *a == Area::Grid));
    }

    #[test]
    fn a_single_mode_shows_no_mode_tabs() {
        let l = layout(&view(427, 240), &width);
        assert!(!l.hits.iter().any(|(_, h)| matches!(h, Hit::Mode(_))));
        let mut two = view(427, 240);
        two.modes.push(stack("minecraft:firework_rocket"));
        let l = layout(&two, &width);
        assert_eq!(l.hits.iter().filter(|(_, h)| matches!(h, Hit::Mode(_))).count(), 2);
    }

    #[test]
    fn fit_cuts_with_dots_only_when_needed() {
        assert_eq!(fit("Oak Planks", 60, &width), "Oak Planks");
        assert_eq!(fit("Bamboo Pressure Plate", 60, &width), "Bamboo...");
        assert!(width(&fit("Bamboo Pressure Plate", 60, &width)) <= 60);
        assert_eq!(fit("Anything", 10, &width), "");
    }

    #[test]
    fn ten_tabs_fit_the_full_panel() {
        assert!(tabs_shown(427, 240) >= 10);
    }
}
