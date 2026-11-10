//! The controller crafting screen (Horizon's own, off by default; docs/controllers.md): a view of
//! the open crafting menu, the inventory's 2x2 grid or a crafting table's 3x3, laid out for a pad.
//! Recipes come by vanilla's creative tabs (`legacy_craft.rs`); LB and RB change the tab, the
//! D-pad or the left stick moves along the row and through a recipe group, A crafts (held, it
//! goes on), Y crafts all, X searches on the search tab and filters elsewhere, B closes.
//!
//! The menu under it stays the server's: each craft is the recipe book's placement and a quick
//! move of the result (`pad_craft.rs`), so it works on any server that takes the recipe book, and
//! someone else's server only with Quick Craft on Servers.
use super::super::pad_craft::Hold;
use super::super::{Game, Screen};
use horizon_client::{
    gamepad::{self, Button, MenuJob},
    hud::HudBatch,
    hud_kit::{
        hint::{self, HintRow, Look, Side},
        plate::Plate,
    },
    items::ItemStack,
    legacy_craft::{self, CraftTab, SEARCH},
    legacy_craft_screen::{self as screen, Cell, Hit, Layout, RecipeSlot, View},
    menu::place::Stacked,
    recipe_book::{Book, Collection},
};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// How long each of an ingredient's items shows before the next ("Any Planks" cycling): vanilla's
/// recipe book swaps its ghost items every 30 ticks.
const CYCLE: Duration = Duration::from_millis(1500);
/// How far the stick tilts before it steps, as menus' focus steps.
const STICK_STEP: f32 = 0.55;

/// The screen's state between frames.
#[derive(Default)]
pub(in crate::game) struct LegacyCraft {
    /// The crafting menu shows as this screen.
    pub(in crate::game) open: bool,
    /// The menu it was opened over: a new one (another crafting table) starts from its first tab.
    menu: Option<i32>,
    tab: usize,
    first_tab: usize,
    mode: usize,
    /// Per tab id: the selected collection and the recipe the player picked of it (none: the
    /// first that can be made, as vanilla's recipe buttons show).
    places: HashMap<String, (usize, Option<usize>)>,
    /// The recipes shown only when they can be made (X off the search tab).
    only_craftable: bool,
    query: String,
    typing: bool,
    /// The tabs as last built, and what they were built from.
    tabs: Vec<CraftTab>,
    tabs_key: Option<(u64, usize, usize, bool)>,
    /// The last frame's layout: where clicks land, and where the row had scrolled to.
    layout: Option<Layout>,
    /// A held direction and when it next steps.
    repeat: Option<(Button, Instant)>,
    stick: Option<Button>,
    /// The right stick, tilted left or right: pages along the row.
    page_stick: Option<Button>,
    page_repeat: Option<Instant>,
    opened_at: Option<Instant>,
    /// What the last frame showed: the row's length and the selected recipe's result.
    shown: (usize, Option<String>),
}

impl Game<'_> {
    /// The open screen is the crafting menu shown as the controller crafting screen.
    pub(in crate::game) fn legacy_showing(&self) -> bool {
        self.pad.legacy.open && self.legacy_fits_menu()
    }

    /// The open screen is a crafting menu this screen can show: the inventory's or a table's.
    fn legacy_fits_menu(&self) -> bool {
        self.screen == Screen::Inventory
            && self.title.is_none()
            && self.active_book() == Some(horizon_client::recipe_book::BookType::Crafting)
    }

    /// The vanilla screen of a crafting grid, with Console Crafting on: the left stick switches to
    /// this screen there.
    pub(in crate::game) fn legacy_switch_here(&self) -> bool {
        self.settings.controller.legacy_crafting
            && self.legacy_fits_menu()
            && !self.legacy_showing()
    }

    /// The pad's crafting button (`pad.crafting`): the inventory opens as this screen.
    pub(in crate::game) fn legacy_open_crafting(&mut self) -> horizon_client::Result<()> {
        if self.screen != Screen::Playing || self.spectator() {
            return Ok(());
        }
        if self.creative {
            // The creative screen has no grid; the inventory's own menu does, as on Legacy Console.
            self.screen = Screen::Inventory;
            self.capture(false);
        } else {
            self.key_pressed(
                winit::keyboard::KeyCode::KeyE,
                winit::keyboard::KeyCode::KeyE,
            )?;
        }
        if self.legacy_fits_menu() {
            self.legacy_start();
        }
        Ok(())
    }

    /// The inventory's switch, or a crafting table opened with the pad: show this screen.
    pub(in crate::game) fn legacy_start(&mut self) {
        let menu = self.remote_menu().0;
        let state = &mut self.pad.legacy;
        if state.menu != Some(menu) {
            // Every recipe: the search tab, before anything's typed.
            state.tab = 0;
            state.first_tab = 0;
            state.places.clear();
            state.query.clear();
        }
        state.open = true;
        state.menu = Some(menu);
        state.typing = false;
        state.repeat = None;
        state.opened_at = Some(Instant::now());
    }

    /// Every frame: the screen closes with its menu, and a crafting table the pad opened shows
    /// as this screen when the player turned it on.
    pub(in crate::game) fn legacy_watch(&mut self) {
        if !self.legacy_fits_menu() {
            let state = &mut self.pad.legacy;
            state.open = false;
            state.menu = None;
            state.typing = false;
            state.layout = None;
            return;
        }
        let menu = self.remote_menu().0;
        let table = self.craft_size == 3;
        if table
            && !self.pad.legacy.open
            && self.pad.legacy.menu != Some(menu)
            && self.pad.in_use
            && self.settings.controller.legacy_crafting
        {
            self.legacy_start();
        }
        self.pad.legacy.menu = Some(menu);
    }

    /// The pad's frame while the screen shows: the left stick steps like the D-pad, a held
    /// direction repeats, and the pointer hides. False when the on-screen keyboard has it.
    pub(in crate::game) fn legacy_frame(&mut self) -> bool {
        if !self.legacy_showing() || self.pad.menu.osk.is_some() {
            return false;
        }
        if !self.pad.in_use {
            return true;
        }
        let now = Instant::now();
        let deadzone = self.settings.controller.move_deadzone;
        let dir = gamepad::stick_direction_held(
            gamepad::radial(self.pad.state.left, deadzone),
            STICK_STEP,
            self.pad.legacy.stick,
        );
        if dir != self.pad.legacy.stick {
            self.pad.legacy.stick = dir;
            if let Some(button) = dir {
                self.legacy_step(button);
                self.legacy_hold(button, now);
            } else if self
                .pad
                .legacy
                .repeat
                .is_some_and(|(b, _)| !self.pad.edges.is_held(b))
            {
                self.pad.legacy.repeat = None;
            }
        }
        // The right stick pages: a row's worth at a time, held it repeats.
        let look = self.settings.controller.look_deadzone;
        let page = gamepad::stick_direction_held(
            gamepad::radial(self.pad.state.right, look),
            STICK_STEP,
            self.pad.legacy.page_stick,
        )
        .filter(|b| matches!(b, Button::DPadLeft | Button::DPadRight));
        if page != self.pad.legacy.page_stick {
            self.pad.legacy.page_stick = page;
            if let Some(button) = page {
                self.legacy_page(button == Button::DPadRight);
                self.pad.legacy.page_repeat = Some(now + self.legacy_delay());
            }
        } else if let (Some(button), Some(at)) = (page, self.pad.legacy.page_repeat)
            && now >= at
        {
            self.legacy_page(button == Button::DPadRight);
            let every = Duration::from_secs_f32(self.settings.controller.repeat_interval);
            self.pad.legacy.page_repeat = Some(now + every);
        }
        if let Some((button, at)) = self.pad.legacy.repeat {
            let held = self.pad.edges.is_held(button) || self.pad.legacy.stick == Some(button);
            if !held {
                self.pad.legacy.repeat = None;
            } else if now >= at {
                self.legacy_step(button);
                let every = Duration::from_secs_f32(self.settings.controller.repeat_interval);
                self.pad.legacy.repeat = Some((button, now + every));
            }
        }
        true
    }

    fn legacy_delay(&self) -> Duration {
        Duration::from_secs_f32(self.settings.controller.repeat_delay)
    }

    fn legacy_hold(&mut self, button: Button, now: Instant) {
        self.pad.legacy.repeat = Some((button, now + self.legacy_delay()));
    }

    /// A row's worth of recipes along: to the end and no further, then round to the other end.
    fn legacy_page(&mut self, next: bool) {
        let Some(len) = self.legacy_current().map(|(row, ..)| row.len()) else {
            return;
        };
        if len == 0 {
            return;
        }
        let shown = self.pad.legacy.layout.as_ref().map_or(11, |l| {
            l.hits
                .iter()
                .filter(|(_, h)| matches!(h, Hit::Row(_)))
                .count()
                .max(1)
        });
        let id = self.legacy_tab_id();
        let place = self.pad.legacy.places.entry(id).or_default();
        let last = len - 1;
        place.0 = match (next, place.0) {
            (true, at) if at == last => 0,
            (true, at) => (at + shown).min(last),
            (false, 0) => last,
            (false, at) => at.saturating_sub(shown),
        };
        place.1 = None;
        self.direct_craft_stop();
    }

    /// The inventory's drawing: this screen when it shows, else the open station's
    /// (`draw_station`); false when neither drew.
    pub(in crate::game) fn draw_legacy_or_station(&mut self, batch: &mut HudBatch) -> bool {
        if self.legacy_showing() {
            self.draw_legacy_craft(batch);
            return true;
        }
        self.draw_station(batch)
    }

    /// The inventory's wheel: along this screen's row when it shows, else the station's.
    pub(in crate::game) fn legacy_or_station_scroll(&mut self, d: f32) {
        if !self.legacy_scroll(d) {
            self.station_scroll(d);
        }
    }

    /// The mouse wheel over the screen: along the row, one recipe a notch.
    pub(in crate::game) fn legacy_scroll(&mut self, d: f32) -> bool {
        if !self.legacy_showing() {
            return false;
        }
        if d != 0.0 {
            self.legacy_step(if d > 0.0 {
                Button::DPadLeft
            } else {
                Button::DPadRight
            });
        }
        true
    }

    /// A pad press while the screen shows: whether it was the screen's.
    pub(in crate::game) fn legacy_button(&mut self, button: Button) -> bool {
        if !self.legacy_showing() {
            return false;
        }
        let now = Instant::now();
        match button {
            Button::DPadLeft | Button::DPadRight | Button::DPadUp | Button::DPadDown => {
                self.legacy_step(button);
                self.legacy_hold(button, now);
            }
            Button::LeftBumper => self.legacy_turn_tab(false),
            Button::RightBumper => self.legacy_turn_tab(true),
            Button::LeftTrigger => self.legacy_turn_mode(false),
            Button::RightTrigger => self.legacy_turn_mode(true),
            Button::South => self.legacy_craft(false),
            Button::North => self.legacy_craft(true),
            // X: only what can be made now, or everything again.
            Button::West => {
                let state = &mut self.pad.legacy;
                state.only_craftable = !state.only_craftable;
                state.places.clear();
            }
            // The left stick: the vanilla screen of the same menu (and back from it, `pad_menu`).
            Button::LeftStick => {
                self.pad.legacy.open = false;
                self.pad.legacy.typing = false;
            }
            // View: straight to the search, typing (it throws in an inventory; here nothing's under
            // a pointer to throw).
            Button::Select => {
                self.pad.legacy.tab = 0;
                self.legacy_type_start();
            }
            // B, Start and the rest: the inventory's own (B and Start close it).
            _ => return false,
        }
        true
    }

    fn legacy_step(&mut self, button: Button) {
        let Some((len, variants, shown)) = self
            .legacy_current()
            .map(|(row, sel, var)| (row.len(), row.get(sel).map_or(0, |c| c.entries.len()), var))
        else {
            return;
        };
        let id = self.legacy_tab_id();
        let place = self.pad.legacy.places.entry(id).or_default();
        match button {
            Button::DPadLeft if len > 0 => *place = ((place.0 + len - 1) % len, None),
            Button::DPadRight if len > 0 => *place = ((place.0 + 1) % len, None),
            Button::DPadUp if variants > 1 => place.1 = Some((shown + variants - 1) % variants),
            Button::DPadDown if variants > 1 => place.1 = Some((shown + 1) % variants),
            _ => return,
        }
        self.direct_craft_stop();
    }

    fn legacy_turn_tab(&mut self, next: bool) {
        let n = self.pad.legacy.tabs.len();
        if n == 0 {
            return;
        }
        let state = &mut self.pad.legacy;
        state.tab = if next {
            (state.tab + 1) % n
        } else {
            (state.tab + n - 1) % n
        };
        state.typing = false;
    }

    fn legacy_turn_mode(&mut self, next: bool) {
        let n = self.legacy_modes().len();
        let state = &mut self.pad.legacy;
        state.mode = if next {
            (state.mode + 1) % n
        } else {
            (state.mode + n - 1) % n
        };
    }

    /// The modes at the panel's left: crafting, for now.
    fn legacy_modes(&self) -> Vec<ItemStack> {
        vec![ItemStack::new("minecraft:crafting_table", 1)]
    }

    fn legacy_tab_id(&self) -> String {
        let state = &self.pad.legacy;
        state
            .tabs
            .get(state.tab)
            .map_or_else(String::new, |t| t.id.clone())
    }

    /// The tabs for the open grid, rebuilt when the known recipes or the grid change.
    fn legacy_refresh_tabs(&mut self) {
        let version = self.crafting_book_refresh();
        let grouped = self.settings.controller.legacy_tiers;
        let key = (
            version,
            self.craft_size,
            self.creative_screen.tabs.len(),
            grouped,
        );
        if self.pad.legacy.tabs_key == Some(key) {
            return;
        }
        let Some((book, client)) = self.crafting_book() else {
            return;
        };
        let tiers = if grouped {
            legacy_craft::tiers(&self.items().item_tag_order)
        } else {
            HashMap::new()
        };
        let tabs = legacy_craft::tabs(
            book,
            client,
            &self.creative_screen.tabs,
            self.craft_size,
            &tiers,
        );
        let state = &mut self.pad.legacy;
        let was = state.tabs.get(state.tab).map(|t| t.id.clone());
        state.tab = was
            .and_then(|id| tabs.iter().position(|t| t.id == id))
            .unwrap_or(0);
        state.tabs = tabs;
        state.tabs_key = Some(key);
    }

    /// The row the selected tab shows (searched or filtered), the selected collection and its
    /// shown recipe, kept in range.
    fn legacy_current(&mut self) -> Option<(Vec<Collection>, usize, usize)> {
        self.legacy_refresh_tabs();
        let stock = self.crafting_stock();
        self.legacy_row(&stock)
    }

    /// `legacy_current` with the book already brought up to date.
    fn legacy_row(&self, stock: &Stacked) -> Option<(Vec<Collection>, usize, usize)> {
        let names = |s: &ItemStack| horizon_client::stations::hover_name(&self.pack.names, s);
        let (book, _) = self.crafting_book()?;
        let state = &self.pad.legacy;
        let tab = state.tabs.get(state.tab)?;
        let mut row = if tab.id == SEARCH {
            legacy_craft::search(book, &tab.groups, &state.query, &names)
        } else {
            tab.groups.clone()
        };
        if state.only_craftable {
            row.retain(|c| {
                c.entries
                    .iter()
                    .any(|&e| book.can_craft(&book.entries[e], stock))
            });
        }
        let (sel, var) = state.places.get(&tab.id).copied().unwrap_or_default();
        let sel = sel.min(row.len().saturating_sub(1));
        let entries = row.get(sel).map_or(&[][..], |c| &c.entries[..]);
        let var = match var {
            Some(var) => var.min(entries.len().saturating_sub(1)),
            None => entries
                .iter()
                .position(|&e| book.can_craft(&book.entries[e], stock))
                .unwrap_or(0),
        };
        Some((row, sel, var))
    }

    /// The selected recipe's index in the book.
    fn legacy_selected_entry(&mut self) -> Option<usize> {
        let (row, sel, var) = self.legacy_current()?;
        row.get(sel)?.entries.get(var).copied()
    }

    /// A: one craft (held, it goes on); Y: every craft the ingredients allow.
    fn legacy_craft(&mut self, all: bool) {
        let Some(entry) = self.legacy_selected_entry() else {
            return;
        };
        let started = if all {
            self.direct_craft_all(entry)
        } else {
            self.direct_craft_start_held(entry, Hold::A)
        };
        if !started {
            // Nothing to make it with, or not here: the press is felt, not lost.
            self.pad_rumble(gamepad::Rumble::RadialNothing);
        }
    }

    /// The search box takes the on-screen keyboard (X on the search tab, or a click on it).
    fn legacy_type_start(&mut self) {
        self.pad.legacy.typing = true;
        // A real keyboard types into it as it is; the pad gets the on-screen one.
        if self.pad.in_use {
            self.pad_open_keyboard();
        }
    }

    /// Whether the search box is being typed in.
    pub(in crate::game) fn legacy_typing(&self) -> bool {
        self.legacy_showing() && self.pad.legacy.typing
    }

    /// What the on-screen keyboard or a real key types into the search box.
    pub(in crate::game) fn legacy_type(
        &mut self,
        text: Option<&str>,
        backspace: bool,
        enter: bool,
    ) {
        let state = &mut self.pad.legacy;
        if let Some(text) = text {
            const MOST: usize = 50;
            for c in text.chars() {
                if state.query.chars().count() < MOST && !c.is_control() {
                    state.query.push(c);
                }
            }
        }
        if backspace {
            state.query.pop();
        }
        if enter {
            state.typing = false;
        }
        state.places.remove(SEARCH);
    }

    /// The keyboard went away: typing ends.
    pub(in crate::game) fn legacy_stop_typing(&mut self) {
        self.pad.legacy.typing = false;
    }

    /// A click while the screen shows: always the screen's (the inventory under it isn't drawn).
    pub(in crate::game) fn legacy_click(&mut self, right: bool) -> bool {
        if !self.legacy_showing() {
            return false;
        }
        let at = (self.cursor.0 as i32, self.cursor.1 as i32);
        let hit = self.pad.legacy.layout.as_ref().and_then(|l| hit_at(l, at));
        let Some(hit) = hit else {
            return true;
        };
        let id = self.legacy_tab_id();
        match hit {
            Hit::Tab(i) => {
                self.pad.legacy.tab = i;
                self.pad.legacy.typing = false;
            }
            Hit::TabPrev => self.legacy_turn_tab(false),
            Hit::TabNext => self.legacy_turn_tab(true),
            Hit::Mode(i) => self.pad.legacy.mode = i,
            Hit::Row(i) => {
                let place = self.pad.legacy.places.entry(id).or_default();
                if place.0 == i && !right {
                    self.legacy_craft(false);
                } else {
                    *place = (i, None);
                }
            }
            Hit::Variant(i) => self.pad.legacy.places.entry(id).or_default().1 = Some(i),
            Hit::VariantUp => self.legacy_step(Button::DPadUp),
            Hit::VariantDown => self.legacy_step(Button::DPadDown),
            Hit::Result => self.legacy_craft(right),
            Hit::Search => self.legacy_type_start(),
            Hit::Inventory(_) => {}
        }
        true
    }

    /// Draws the screen; called in place of the inventory's drawing.
    pub(in crate::game) fn draw_legacy_craft(&mut self, batch: &mut HudBatch) {
        self.render_transparent_background("inventory", batch);
        let view = self.legacy_view(batch.width as i32, batch.height as i32);
        self.pad.legacy.shown = (
            view.row.len(),
            view.result.as_ref().map(|r| r.stack.item.to_string()),
        );
        let atlas_width = |s: &str| self.gui.atlas.font.width(s).round() as i32;
        let mut layout = screen::layout(&view, &atlas_width);
        if !self.pad.in_use {
            let at = (self.cursor.0 as i32, self.cursor.1 as i32);
            if let Some(hit) = hit_at(&layout, at)
                && matches!(hit, Hit::Row(_))
            {
                let hovered = View {
                    hover: Some(hit),
                    ..view
                };
                layout = screen::layout(&hovered, &atlas_width);
            }
        }
        self.paint_book(batch, &layout.calls);
        // The recipes in sight are seen: they lose their new marks (vanilla's `recipeShown` for
        // each button a page shows).
        let stock = self.crafting_stock();
        for display in self.legacy_in_sight(&layout, &stock) {
            self.recipe_seen(display);
        }
        if self.pad.in_use {
            self.legacy_hints(batch, layout.hint_y as f32);
        }
        self.pad.legacy.layout = Some(layout);
    }

    /// The screen as `horizon-ctl pad status` reports it: none while it isn't showing.
    pub(in crate::game) fn legacy_status(&self) -> serde_json::Value {
        if !self.legacy_showing() {
            return serde_json::Value::Null;
        }
        let state = &self.pad.legacy;
        serde_json::json!({
            "tab": state.tabs.get(state.tab).map(|t| t.id.clone()),
            "tabs": state.tabs.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            "row": state.shown.0,
            "selected": state.shown.1,
            "query": state.query,
            "typing": state.typing,
            "only_craftable": state.only_craftable,
            "compact": state.layout.as_ref().is_some_and(|l| l.compact),
        })
    }

    /// The display ids of every recipe in the row's slots on screen.
    fn legacy_in_sight(&self, layout: &Layout, stock: &Stacked) -> Vec<i32> {
        let Some((row, _, _)) = self.legacy_row(stock) else {
            return Vec::new();
        };
        let Some((book, client)) = self.crafting_book() else {
            return Vec::new();
        };
        layout
            .hits
            .iter()
            .filter_map(|(_, hit)| match hit {
                Hit::Row(i) => row.get(*i),
                _ => None,
            })
            .flat_map(|c| c.entries.iter().map(|&e| book.entries[e].display_id))
            .filter(|d| client.highlight.contains(d))
            .collect()
    }

    /// The hint row under the panel: each button's glyph from what it does, so a rebinding, the
    /// pad's family and a Nintendo pad's letters all reach it.
    fn legacy_hints(&mut self, batch: &mut HudBatch, y: f32) {
        let tr = |key: &str| horizon_client::lang::get(key);
        let mut rows = vec![
            HintRow::new(
                [self.menu_glyph(MenuJob::Confirm)],
                tr("horizon.legacy_craft.craft"),
            ),
            HintRow::new(
                [self.menu_glyph(MenuJob::Extra)],
                tr("horizon.legacy_craft.craft_all"),
            ),
            HintRow::new(
                [self.menu_glyph(MenuJob::Alternate)],
                tr(if self.pad.legacy.only_craftable {
                    "gui.recipebook.toggleRecipes.craftable"
                } else {
                    "gui.recipebook.toggleRecipes.all"
                }),
            ),
            HintRow::new(
                [
                    self.menu_glyph(MenuJob::PagePrev),
                    self.menu_glyph(MenuJob::PageNext),
                ],
                tr("horizon.legacy_craft.category"),
            ),
            HintRow::new(
                [self.menu_glyph(MenuJob::View)],
                tr("horizon.legacy_craft.search"),
            ),
            HintRow::new(
                [self.menu_glyph(MenuJob::LeftStick)],
                tr("horizon.legacy_craft.classic"),
            ),
            HintRow::new([self.menu_glyph(MenuJob::Back)], tr("gui.back")),
        ];
        let atlas = &self.gui.atlas;
        let look = Look {
            side: Side::Leading,
            ..hint::GUIDE
        };
        const BETWEEN: f32 = 8.0;
        const EDGE: f32 = 6.0;
        // The rows that fit, from the right; the least needed (the filter) goes first.
        let room = batch.width - 2.0 * EDGE;
        let total = |rows: &[HintRow]| {
            rows.iter()
                .map(|r| hint::plated_width(atlas, r, &look))
                .sum::<f32>()
                + BETWEEN * rows.len().saturating_sub(1) as f32
        };
        // What goes first when it doesn't fit: the filter, Classic, Craft All, then Search.
        for drop in [2, 4, 1, 2] {
            if total(&rows) <= room || rows.len() <= drop {
                break;
            }
            rows.remove(drop);
        }
        // On the player's text background (Accessibility), as the gameplay guide's rows are.
        let alpha = (self.settings.text_background(0.4) >> 24) as u8;
        const ROW_H: f32 = 11.0;
        // Centred under the panel, as the screen is.
        let mut x = ((batch.width - total(&rows)) / 2.0).max(EDGE).round();
        for row in &rows {
            let w = hint::plated_width(atlas, row, &look);
            let plate = Plate::Backing { alpha };
            hint::draw_plated(batch, atlas, row, &look, plate, [x, y - 1.0, w, ROW_H]);
            x += w + BETWEEN;
        }
    }

    /// What the screen shows this frame.
    fn legacy_view(&mut self, width: i32, height: i32) -> View {
        let tr = |key: &str| horizon_client::lang::get(key);
        self.legacy_refresh_tabs();
        let stock = self.crafting_stock();
        let current = self.legacy_row(&stock);
        let grid = self.craft_size;
        let inventory: Vec<Option<ItemStack>> = self.inventory[..36].to_vec();
        let shown = screen::tabs_shown(width, height);
        let opened = self.pad.legacy.opened_at.unwrap_or_else(Instant::now);
        let cycle = (opened.elapsed().as_millis() / CYCLE.as_millis()) as usize;
        let mut view = View {
            width,
            height,
            row_from: self.pad.legacy.layout.as_ref().map_or(0, |l| l.row_from),
            modes: self.legacy_modes(),
            mode: self.pad.legacy.mode,
            grid,
            inventory,
            inventory_title: self.translate("container.inventory", "Inventory"),
            empty: tr("horizon.legacy_craft.empty"),
            search_hint: tr("gui.recipebook.search_hint"),
            ..View::default()
        };
        let state = &self.pad.legacy;
        view.tabs = state
            .tabs
            .iter()
            .map(|t| ItemStack::new(&t.icon, 1))
            .collect();
        view.tab = state.tab;
        let first = state.first_tab.min(view.tabs.len().saturating_sub(shown));
        let first = if state.tab < first {
            state.tab
        } else if state.tab >= first + shown {
            state.tab + 1 - shown
        } else {
            first
        };
        view.first_tab = first;
        if view.tabs.len() > shown {
            let pages = view.tabs.len().div_ceil(shown);
            view.tab_page = Some(format!("{}/{}", state.tab / shown + 1, pages));
        }
        let tab = state.tabs.get(state.tab).cloned();
        let query = state.query.clone();
        let typing = state.typing;
        self.pad.legacy.first_tab = first;
        if let Some(tab) = &tab {
            if tab.id == SEARCH {
                view.search = Some((query, typing));
                view.title = self.translate("itemGroup.search", "Search Items");
            } else {
                view.title = tab.title.clone();
            }
        }
        let Some((row, sel, var)) = current else {
            return view;
        };
        let names = |s: &ItemStack| horizon_client::stations::hover_name(&self.pack.names, s);
        let Some((book, client)) = self.crafting_book() else {
            return view;
        };
        let slot = |e: usize| {
            let entry = &book.entries[e];
            RecipeSlot {
                stack: entry
                    .results()
                    .first()
                    .cloned()
                    .unwrap_or_else(|| entry.result.clone()),
                craftable: book.can_craft(entry, &stock),
                fresh: client.highlight.contains(&entry.display_id),
            }
        };
        view.row = row
            .iter()
            .map(|c| {
                // The collection shows a recipe that can be made, if any, as vanilla's buttons do.
                let e = c
                    .entries
                    .iter()
                    .copied()
                    .find(|&e| book.can_craft(&book.entries[e], &stock))
                    .unwrap_or(c.entries[0]);
                let mut s = slot(e);
                s.fresh = c
                    .entries
                    .iter()
                    .any(|&e| client.highlight.contains(&book.entries[e].display_id));
                s
            })
            .collect();
        view.selected = sel;
        let Some(collection) = row.get(sel) else {
            return view;
        };
        if collection.entries.len() > 1 {
            view.variants = collection.entries.iter().map(|&e| slot(e)).collect();
            view.variant = var;
        }
        let entry_index = collection.entries[var.min(collection.entries.len() - 1)];
        let entry = &book.entries[entry_index];
        let result = slot(entry_index);
        view.row[sel] = result.clone();
        let craftable = result.craftable;
        view.name = names(&result.stack);
        view.result = Some(result);
        let (cells, missing) = grid_cells(book, entry_index, grid, &stock, cycle);
        view.cells = cells;
        view.note = Some(if craftable {
            let ingredients = entry.requirements();
            let can = stock.clone().try_pick_all(&ingredients, i32::MAX, None);
            (
                horizon_client::lang::tr("horizon.legacy_craft.can_make", &[&can]),
                screen::HAVE,
            )
        } else if let Some((item, count)) = missing.first() {
            let what = format!("{} x{count}", names(&ItemStack::new(item, 1)));
            (
                horizon_client::lang::tr("horizon.legacy_craft.missing", &[&what]),
                screen::MISSING,
            )
        } else {
            (tr("horizon.legacy_craft.missing_some"), screen::MISSING)
        });
        view
    }
}

/// The hit under a point; later ones are drawn over earlier ones.
fn hit_at(layout: &Layout, (x, y): (i32, i32)) -> Option<Hit> {
    layout
        .hits
        .iter()
        .rev()
        .find(|([hx, hy, w, h], _)| x >= *hx && y >= *hy && x < hx + w && y < hy + h)
        .map(|(_, hit)| *hit)
}

/// The grid preview of a recipe: each cell's item (an ingredient with several items shows one
/// the player has, cycling through them; else cycles through all of them), whether the player
/// lacks it, and what's missing in all (item, count).
fn grid_cells(
    book: &Book,
    entry: usize,
    grid: usize,
    stock: &Stacked,
    cycle: usize,
) -> (Vec<Option<Cell>>, Vec<(String, i32)>) {
    let entry = &book.entries[entry];
    let ingredients = entry.requirements();
    let mut cells: Vec<Option<Cell>> = vec![None; grid * grid];
    // What each ingredient takes from what's left, in order, so two cells of one item count twice.
    let mut left: HashMap<String, i32> = HashMap::new();
    let mut missing: Vec<(String, i32)> = Vec::new();
    for (ingredient, cell) in entry.grid_cells(grid, grid) {
        let Some(i) = ingredient else {
            continue;
        };
        let Some(ing) = ingredients.get(i) else {
            continue;
        };
        let items = book.items_of(ing);
        let have: Vec<&String> = items
            .iter()
            .filter(|item| {
                *left
                    .entry((*item).clone())
                    .or_insert_with(|| stock.count(item))
                    > 0
            })
            .collect();
        let (item, lacking) = match have.get(cycle % have.len().max(1)) {
            Some(item) => ((*item).clone(), false),
            None => match items.get(cycle % items.len().max(1)) {
                Some(item) => (item.clone(), true),
                None => continue,
            },
        };
        if lacking {
            match missing.iter_mut().find(|(m, _)| *m == items[0]) {
                Some((_, n)) => *n += 1,
                None => missing.push((items[0].clone(), 1)),
            }
        } else if let Some(n) = left.get_mut(&item) {
            *n -= 1;
        }
        if let Some(slot) = cells.get_mut(cell) {
            *slot = Some(Cell {
                stack: ItemStack::new(&item, 1),
                missing: lacking,
            });
        }
    }
    (cells, missing)
}
