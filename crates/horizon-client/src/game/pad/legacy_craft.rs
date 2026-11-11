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
    legacy_craft_screen::{self as screen, Area, Cell, Hit, Layout, RecipeSlot, View},
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
/// Seconds a direction is held along the row before it runs faster (twice that: faster again).
const ACCELERATE_AFTER: f32 = 1.0;

/// The screen's state between frames.
#[derive(Default)]
pub(in crate::game) struct LegacyCraft {
    /// The crafting menu shows as this screen.
    pub(in crate::game) open: bool,
    /// The menu it was opened over: a new one (another crafting table) starts from its first tab.
    menu: Option<i32>,
    tab: usize,
    first_tab: usize,
    pub(in crate::game) mode: usize,
    /// Per tab id: the selected collection and the recipe the player picked of it (none: the
    /// first that can be made, as vanilla's recipe buttons show).
    places: HashMap<String, (usize, Option<usize>)>,
    /// The recipes shown only when they can be made (X off the search tab).
    only_craftable: bool,
    query: String,
    pub(in crate::game) typing: bool,
    /// The tabs as last built, and what they were built from.
    tabs: Vec<CraftTab>,
    tabs_key: Option<(u64, usize, usize, bool)>,
    /// The last frame's layout: where clicks land, and where the row had scrolled to.
    layout: Option<Layout>,
    /// A held direction and when it next steps.
    repeat: Option<(Button, Instant)>,
    stick: Option<Button>,
    /// The right stick's last flick, which moves the focus between areas once per flick.
    area_stick: Option<Button>,
    /// Where the pad's focus is, and its cell there (an inventory slot, a grid cell).
    pub(in crate::game) focus: Area,
    pub(in crate::game) cursor: usize,
    /// The builders' templates, staging and running craft.
    pub(in crate::game) build: super::legacy_build::Build,
    /// The stonecutter's materials, cuts and running cut.
    pub(in crate::game) cutter: super::legacy_cutter::Cutter,
    /// The smithing table's tabs and staging.
    pub(in crate::game) smith: super::legacy_smith::Smith,
    /// The anvil's selection, addition and name.
    pub(in crate::game) anvil: super::legacy_anvil::Anvil,
    /// The brewing stand's selected ingredient.
    pub(in crate::game) brew: super::legacy_brew::Brew,
    /// The loom's pattern and staging.
    pub(in crate::game) loom: super::legacy_loom::Loom,
    /// A held direction's start: past a second of it, the row runs faster.
    held_since: Option<Instant>,
    /// A vanilla click the screen passes on to the inventory under it.
    passthrough: bool,
    /// The row's selection as last drawn: where a flick out of the row starts.
    shown_selected: usize,
    /// The slot the carried stack was taken from by the cursor: it goes back there on leaving
    /// the inventory.
    taken_from: Option<usize>,
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
            && (self.active_book().is_some() || self.legacy_station())
    }

    /// The vanilla screen of a crafting grid, with Console Crafting on: the left stick switches to
    /// this screen there.
    pub(in crate::game) fn legacy_switch_here(&self) -> bool {
        self.settings.controller.legacy_crafting
            && self.legacy_fits_menu()
            && !self.legacy_showing()
    }

    /// The station the open menu is, as its screen choice is kept (`PadSettings::legacy_views`).
    pub(in crate::game) fn legacy_kind(&self) -> &'static str {
        use horizon_client::recipe_book::BookType;
        if self.legacy_cutter() {
            "stonecutter"
        } else if self.legacy_smith() {
            "smithing_table"
        } else if self.legacy_anvil() {
            "anvil"
        } else if self.legacy_brewing() {
            "brewing_stand"
        } else if self.legacy_loom() {
            "loom"
        } else {
            match self.legacy_furnace() {
                Some(BookType::Furnace) => "furnace",
                Some(BookType::BlastFurnace) => "blast_furnace",
                Some(BookType::Smoker) => "smoker",
                _ if self.craft_size == 3 => "crafting_table",
                _ => "inventory",
            }
        }
    }

    /// The player switched this station's screen (L3, the switch button): it shows, and the
    /// station opens on it from now on, kept in the controller settings file.
    pub(in crate::game) fn legacy_choose(&mut self, console: bool) {
        let kind = self.legacy_kind().to_owned();
        self.settings.controller.legacy_views.insert(kind, console);
        if let Some(dir) = Self::options_path().parent() {
            self.settings.controller.save(dir);
        }
        if console {
            self.legacy_start();
        } else {
            self.pad.legacy.open = false;
            self.pad.legacy.typing = false;
        }
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
        if state.menu != Some(menu) {
            // Crafting or the station: the builders are a crafting grid's.
            state.mode = 0;
        }
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
        // A crafting table's or a furnace's own menu: the screen it opens on, for a pad player.
        // Each station opens on the screen the player last switched it to (the inventory, opened
        // with Y, on vanilla's until switched); X opens crafting on this one regardless.
        let kind = self.legacy_kind();
        if self.settings.controller.legacy_view(kind)
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
        // The right stick flicks the focus to the next area that way, once per flick.
        let look = self.settings.controller.look_deadzone;
        let flick = gamepad::stick_direction_held(
            gamepad::radial(self.pad.state.right, look),
            STICK_STEP,
            self.pad.legacy.area_stick,
        );
        if flick != self.pad.legacy.area_stick {
            self.pad.legacy.area_stick = flick;
            if let Some(button) = flick {
                self.legacy_flick(button);
            }
        }
        if let Some((button, at)) = self.pad.legacy.repeat {
            let held = self.pad.edges.is_held(button) || self.pad.legacy.stick == Some(button);
            if !held {
                self.pad.legacy.repeat = None;
            } else if now >= at {
                self.legacy_step(button);
                let every = Duration::from_secs_f32(self.settings.controller.repeat_interval);
                // Along a long row, a held direction speeds up: twice as fast past a second, four
                // times past two (the all-recipes row runs to hundreds).
                let held = self
                    .pad
                    .legacy
                    .held_since
                    .map_or(Duration::ZERO, |t| now - t);
                let along_row = self.pad.legacy.focus == Area::Row
                    && matches!(button, Button::DPadLeft | Button::DPadRight);
                let speed = match held.as_secs_f32() {
                    t if along_row && t >= ACCELERATE_AFTER * 2.0 => 4,
                    t if along_row && t >= ACCELERATE_AFTER => 2,
                    _ => 1,
                };
                self.pad.legacy.repeat = Some((button, now + every / speed));
            }
        }
        true
    }

    fn legacy_delay(&self) -> Duration {
        Duration::from_secs_f32(self.settings.controller.repeat_delay)
    }

    fn legacy_hold(&mut self, button: Button, now: Instant) {
        self.pad.legacy.repeat = Some((button, now + self.legacy_delay()));
        self.pad.legacy.held_since = Some(now);
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
        // The inventory's cursor does what vanilla's pointer does over a slot, and so does the
        // furnace's over its own three.
        let furnace = self.legacy_furnace().is_some();
        let cutter = self.legacy_cutter();
        let cutter_slots = [
            super::super::stations::INPUT,
            super::super::stations::RESULT,
        ];
        let slot_area = match self.pad.legacy.focus {
            Area::Inventory => Some(self.pad.legacy.cursor),
            Area::Grid if furnace => super::legacy_furnace::SLOTS
                .get(self.pad.legacy.cursor)
                .copied(),
            Area::Grid if cutter => cutter_slots.get(self.pad.legacy.cursor).copied(),
            Area::Grid if self.legacy_brewing() => super::legacy_brew::SLOTS
                .get(self.pad.legacy.cursor)
                .copied(),
            _ => None,
        };
        if let Some(slot) = slot_area {
            match button {
                Button::South => return self.legacy_slot_click(slot, false, false),
                Button::West => return self.legacy_slot_click(slot, true, false),
                Button::North => return self.legacy_slot_click(slot, false, true),
                Button::Select => return self.legacy_slot_throw(slot),
                _ => {}
            }
        }
        // A builder's own presses: crafting its staging, stepping a grid cell's item.
        if self.legacy_build_button(button) {
            return true;
        }
        // A smithing table's: A and X step a cell in the grid; A or Y smiths.
        if self.legacy_smith() && self.legacy_smith_button(button) {
            return true;
        }
        // A brewing stand's row: A adds the ingredient, Y fills the bottles, X fuels it.
        if self.legacy_brewing() && self.legacy_brew_button(button) {
            return true;
        }
        // A loom's: A and X step a cell in the grid; A or Y weaves.
        if self.legacy_loom() && self.legacy_loom_button(button) {
            return true;
        }
        // An anvil's: the addition in the grid, the name, A or Y works it.
        if self.legacy_anvil() && self.legacy_anvil_button(button) {
            return true;
        }
        // A stonecutter's: A cuts one, Y all; X and View have nothing to do there.
        if cutter && self.pad.legacy.focus == Area::Row {
            match button {
                Button::South => {
                    self.legacy_cut(false);
                    return true;
                }
                Button::North => {
                    self.legacy_cut(true);
                    return true;
                }
                Button::West | Button::Select => return true,
                _ => {}
            }
        }
        match button {
            Button::DPadLeft | Button::DPadRight | Button::DPadUp | Button::DPadDown => {
                self.legacy_step(button);
                self.legacy_hold(button, now);
            }
            // B out of the grid or the inventory goes back to the recipes; from them it closes.
            Button::East if self.pad.legacy.focus != Area::Row => {
                if self.pad.legacy.focus != Area::Inventory || self.legacy_put_back() {
                    self.pad.legacy.focus = Area::Row;
                } else {
                    self.pad_rumble(gamepad::Rumble::RadialNothing);
                }
            }
            Button::LeftBumper => self.legacy_turn_tab(false),
            Button::RightBumper => self.legacy_turn_tab(true),
            Button::LeftTrigger => self.legacy_turn_mode(false),
            Button::RightTrigger => self.legacy_turn_mode(true),
            Button::South => self.legacy_craft(false),
            Button::North => self.legacy_craft(true),
            // X at a furnace: fuel in.
            Button::West if furnace => {
                let ingredient = self.legacy_selected_entry().and_then(|e| {
                    let (book, _) = self.crafting_book()?;
                    let first = book
                        .entries
                        .get(e)?
                        .requirements()
                        .first()?
                        .order
                        .first()?
                        .clone();
                    Some(first)
                });
                self.legacy_add_fuel(ingredient.as_deref());
            }
            // X: only what can be made now, or everything again.
            Button::West => {
                let state = &mut self.pad.legacy;
                state.only_craftable = !state.only_craftable;
                state.places.clear();
            }
            // The left stick: the vanilla screen of the same menu (and back from it, `pad_menu`).
            Button::LeftStick => self.legacy_choose(false),
            // View: straight to the search, typing (it throws in an inventory; here nothing's under
            // a pointer to throw).
            Button::Select => {
                self.pad.legacy.mode = 0;
                self.pad.legacy.focus = Area::Row;
                self.pad.legacy.tab = 0;
                self.legacy_type_start();
            }
            // B, Start and the rest: the inventory's own (B and Start close it).
            _ => return false,
        }
        true
    }

    fn legacy_step(&mut self, button: Button) {
        match self.pad.legacy.focus {
            Area::Row => self.legacy_row_step(button),
            Area::Grid if self.legacy_brewing() => {
                self.pad.legacy.cursor =
                    super::legacy_brew::brew_step(self.pad.legacy.cursor, button);
            }
            Area::Grid if self.legacy_cutter() => {
                self.pad.legacy.cursor = match button {
                    Button::DPadLeft => 0,
                    Button::DPadRight => 1,
                    _ => self.pad.legacy.cursor.min(1),
                };
            }
            Area::Grid if self.legacy_furnace().is_some() => {
                self.pad.legacy.cursor =
                    super::legacy_furnace::furnace_step(self.pad.legacy.cursor, button);
            }
            Area::Grid => {
                let side = self.craft_size.max(1);
                self.pad.legacy.cursor = grid_step(self.pad.legacy.cursor, side, button);
            }
            Area::Inventory => {
                self.pad.legacy.cursor = inventory_step(self.pad.legacy.cursor, button);
            }
        }
    }

    /// A right-stick flick: the focus moves to the nearest area that way from where it is, and
    /// onto the cell of it nearest the cell it left.
    fn legacy_flick(&mut self, button: Button) {
        let Some(layout) = self.pad.legacy.layout.as_ref() else {
            return;
        };
        let dir = match button {
            Button::DPadLeft => [-1, 0],
            Button::DPadRight => [1, 0],
            Button::DPadUp => [0, -1],
            _ => [0, 1],
        };
        let state = &self.pad.legacy;
        let at = match state.focus {
            Area::Row => layout.rect_of(Hit::Row(state.shown_selected)),
            Area::Grid => layout.rect_of(Hit::Cell(state.cursor)),
            Area::Inventory => layout.rect_of(Hit::Inventory(state.cursor)),
        }
        .or_else(|| {
            layout
                .areas
                .iter()
                .find(|(a, _)| *a == state.focus)
                .map(|(_, r)| *r)
        })
        .unwrap_or(layout.bounds);
        let from = [at[0] + at[2] / 2, at[1] + at[3] / 2];
        let Some(to) = layout.area_toward(state.focus, from, dir) else {
            // Nothing that way: a soft bump, the focus stays.
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        };
        let cursor = layout.nearest_cell(to, from).unwrap_or(0);
        if self.pad.legacy.focus == Area::Inventory && !self.legacy_put_back() {
            // Nowhere to put it down: the stack stays in hand and the focus with it.
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        self.pad.legacy.focus = to;
        self.pad.legacy.cursor = cursor;
        self.pad.legacy.repeat = None;
        self.direct_craft_stop();
    }

    /// Vanilla's click on an inventory slot, as the pointer over it would make it: A's take or
    /// put, X's half or one, Y's quick move. Always the screen's press.
    pub(in crate::game) fn legacy_slot_click(
        &mut self,
        slot: usize,
        right: bool,
        shift: bool,
    ) -> bool {
        let Ok((ox, oy)) = self.gui_origin() else {
            return true;
        };
        let (sx, sy) = self.slot_position(slot);
        let held = self.carried.is_some();
        self.pad.legacy.passthrough = true;
        self.click_at(ox + sx + 8.0, oy + sy + 8.0, right, shift);
        self.pad.legacy.passthrough = false;
        match (held, self.carried.is_some()) {
            (false, true) => self.pad.legacy.taken_from = Some(slot),
            (_, false) => self.pad.legacy.taken_from = None,
            _ => {}
        }
        true
    }

    /// Leaving the inventory with a stack in hand: it goes back down, so it never travels with
    /// the focus. Where it came from if that slot can take it, else the slot under the cursor,
    /// else any empty one. False when it couldn't all go down: the focus stays.
    fn legacy_put_back(&mut self) -> bool {
        let Some(carried) = self.carried.clone() else {
            return true;
        };
        let takes = |s: &Option<ItemStack>| {
            s.as_ref()
                .is_none_or(|s| s.same_item_same_components(&carried))
        };
        let state = &self.pad.legacy;
        let mut order: Vec<usize> = state.taken_from.into_iter().collect();
        order.push(state.cursor);
        // Any empty slot: the inventory's rows first, the hotbar last, as a quick move fills.
        order.extend((9..36).chain(0..9).filter(|&i| self.inventory[i].is_none()));
        for slot in order {
            if slot < 36 && takes(&self.inventory[slot]) {
                self.legacy_slot_click(slot, false, false);
                if self.carried.is_none() {
                    return true;
                }
            }
        }
        false
    }

    /// View on an inventory slot: vanilla's Q over it, one item thrown.
    fn legacy_slot_throw(&mut self, slot: usize) -> bool {
        let Ok((ox, oy)) = self.gui_origin() else {
            return true;
        };
        let (sx, sy) = self.slot_position(slot);
        let saved = self.cursor;
        self.cursor = (ox + sx + 8.0, oy + sy + 8.0);
        let key = winit::keyboard::KeyCode::KeyQ;
        if let Err(e) = self.key_pressed(key, key) {
            log::warn!("Horizon: crafting screen throw: {e}");
        }
        self.cursor = saved;
        true
    }

    fn legacy_row_step(&mut self, button: Button) {
        if self.legacy_cutter() {
            return self.legacy_cutter_step(button);
        }
        if self.legacy_smith() {
            return self.legacy_smith_step(button);
        }
        if self.legacy_anvil() {
            return self.legacy_anvil_step(button);
        }
        if self.legacy_brewing() {
            return self.legacy_brew_step(button);
        }
        if self.legacy_loom() {
            return self.legacy_loom_step(button);
        }
        if self.legacy_builder().is_some() {
            return self.legacy_build_step(button);
        }
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
        if self.legacy_cutter() {
            return self.legacy_cutter_turn(next);
        }
        if self.legacy_smith() {
            return self.legacy_smith_turn();
        }
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
        // A builder's craft ends where it is (its grid comes back as it goes on), and the focus
        // starts again on the row.
        if self.legacy_build_running() {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        let state = &mut self.pad.legacy;
        state.mode = if next {
            (state.mode + 1) % n
        } else {
            (state.mode + n - 1) % n
        };
        state.focus = Area::Row;
        state.cursor = 0;
        state.typing = false;
    }

    /// The modes at the panel's left: crafting, then the builders (`legacy_build.rs`).
    fn legacy_modes(&self) -> Vec<ItemStack> {
        if self.legacy_furnace().is_some() {
            return vec![self.legacy_furnace_icon()];
        }
        if self.legacy_cutter() {
            return vec![ItemStack::new("minecraft:stonecutter", 1)];
        }
        if self.legacy_smith() {
            return vec![ItemStack::new("minecraft:smithing_table", 1)];
        }
        if self.legacy_anvil() {
            return vec![ItemStack::new("minecraft:anvil", 1)];
        }
        if self.legacy_brewing() {
            return vec![ItemStack::new("minecraft:brewing_stand", 1)];
        }
        if self.legacy_loom() {
            return vec![ItemStack::new("minecraft:loom", 1)];
        }
        self.legacy_mode_icons()
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
        let furnace = self.legacy_furnace();
        // The book's type in the key: a furnace's tabs aren't a table's.
        let size = self.craft_size + furnace.map_or(0, |b| 10 * b.index());
        let key = (version, size, self.creative_screen.tabs.len(), grouped);
        if self.pad.legacy.tabs_key == Some(key) {
            return;
        }
        if let Some(book_type) = furnace {
            let tabs = self.legacy_furnace_tabs(book_type);
            let state = &mut self.pad.legacy;
            state.tab = state.tab.min(tabs.len().saturating_sub(1));
            state.tabs = tabs;
            state.tabs_key = Some(key);
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
        if let Some(book_type) = self.legacy_furnace() {
            return self.legacy_smelt(book_type, entry, all);
        }
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
        // At an anvil the box is its name.
        if self.legacy_anvil() {
            self.legacy_anvil_text(text, backspace);
            if enter {
                self.pad.legacy.typing = false;
            }
            return;
        }
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
        if !self.legacy_showing() || self.pad.legacy.passthrough {
            return false;
        }
        let at = (self.cursor.0 as i32, self.cursor.1 as i32);
        let hit = self.pad.legacy.layout.as_ref().and_then(|l| hit_at(l, at));
        let Some(hit) = hit else {
            // A click on an area's place with nothing there: the focus goes to it (the compact
            // layout's inventory page, its recipes again).
            let area = self.pad.legacy.layout.as_ref().and_then(|l| {
                l.areas.iter().find(|(_, [x, y, w, h])| {
                    at.0 >= *x && at.1 >= *y && at.0 < x + w && at.1 < y + h
                })
            });
            if let Some((area, _)) = area.copied() {
                self.pad.legacy.focus = area;
                self.pad.legacy.cursor = 0;
            }
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
                self.pad.legacy.focus = Area::Row;
                let place = self.pad.legacy.places.entry(id).or_default();
                if place.0 == i && !right {
                    self.legacy_craft(false);
                } else {
                    *place = (i, None);
                }
            }
            Hit::Variant(i) => self.pad.legacy.places.entry(id).or_default().1 = Some(i),
            Hit::VariantUp => self.legacy_row_step(Button::DPadUp),
            Hit::VariantDown => self.legacy_row_step(Button::DPadDown),
            Hit::Result => self.legacy_craft(right),
            Hit::Search => self.legacy_type_start(),
            Hit::Inventory(i) => {
                let shift = self.has_shift_down();
                self.pad.legacy.focus = Area::Inventory;
                self.pad.legacy.cursor = i;
                self.legacy_slot_click(i, right, shift);
            }
            Hit::Cell(i) => {
                self.pad.legacy.focus = Area::Grid;
                self.pad.legacy.cursor = i;
            }
        }
        true
    }

    /// Draws the screen; called in place of the inventory's drawing.
    pub(in crate::game) fn draw_legacy_craft(&mut self, batch: &mut HudBatch) {
        self.render_transparent_background("inventory", batch);
        self.legacy_build_poll();
        self.legacy_cutter_poll();
        let mut view = self.legacy_view(batch.width as i32, batch.height as i32);
        // What's selected: the preview's result, or the row's (a furnace has no preview slot).
        let selected = view.result.as_ref().or(view.row.get(view.selected));
        self.pad.legacy.shown = (view.row.len(), selected.map(|r| r.stack.item.to_string()));
        self.pad.legacy.shown_selected = view.selected;
        let atlas_width = |s: &str| self.gui.atlas.font.width(s).round() as i32;
        // The pad's focus shows only while the pad plays; the mouse gets the hover instead.
        let pad = self.pad.in_use;
        // The focus's area shows (the compact layout pages to it); its cursor only for the pad.
        view.focus = self.pad.legacy.focus;
        view.cursor = if pad {
            self.pad.legacy.cursor
        } else {
            usize::MAX
        };
        let mut layout = screen::layout(&view, &atlas_width);
        // An area the layout no longer has (a small view has no inventory; an empty row no grid):
        // the focus goes to one it has.
        if !layout
            .areas
            .iter()
            .any(|(a, _)| *a == self.pad.legacy.focus)
        {
            let to = layout.areas.first().map_or(Area::Row, |(a, _)| *a);
            self.pad.legacy.focus = to;
            self.pad.legacy.cursor = 0;
            view.focus = to;
            view.cursor = if pad { 0 } else { usize::MAX };
            layout = screen::layout(&view, &atlas_width);
        }
        let at = (self.cursor.0 as i32, self.cursor.1 as i32);
        let hover = (!pad).then(|| hit_at(&layout, at)).flatten();
        if let Some(hit @ Hit::Row(_)) = hover {
            let hovered = View {
                hover: Some(hit),
                ..view.clone()
            };
            layout = screen::layout(&hovered, &atlas_width);
        }
        self.paint_book(batch, &layout.calls);
        // The recipes in sight are seen: they lose their new marks (vanilla's `recipeShown` for
        // each button a page shows).
        let stock = self.crafting_stock();
        for display in self.legacy_in_sight(&layout, &stock) {
            self.recipe_seen(display);
        }
        // What's under the focus or the pointer gets its tooltip, as over a vanilla slot; none
        // while an item's carried, as vanilla shows none then.
        let focused = if pad {
            match view.focus {
                Area::Inventory => Some(Hit::Inventory(view.cursor)),
                Area::Grid => Some(Hit::Cell(view.cursor)),
                Area::Row => None,
            }
        } else {
            hover
        };
        let stack = focused.and_then(|hit| match hit {
            Hit::Inventory(i) => view.inventory.get(i).cloned().flatten(),
            Hit::Cell(i) => view.cells.get(i).cloned().flatten().map(|c| c.stack),
            Hit::Row(i) => view.row.get(i).map(|r| r.stack.clone()),
            Hit::Result => view.result.as_ref().map(|r| r.stack.clone()),
            _ => None,
        });
        let place = focused.and_then(|hit| layout.rect_of(hit));
        if self.carried.is_none()
            && let (Some(stack), Some([x, y, w, _])) = (stack, place)
        {
            let (tx, ty) = if pad {
                ((x + w) as f32, y as f32 + 8.0)
            } else {
                self.cursor
            };
            self.draw_stack_tooltip(batch, &stack, tx, ty);
        }
        // Over the focused cell, the controller's cursor, as over a vanilla inventory's slot.
        if pad && let Some([x, y, w, h]) = place {
            use horizon_client::pad_cursor;
            let (left, top) = pad_cursor::origin(((x + w / 2) as f32, (y + h / 2) as f32));
            let size = pad_cursor::DRAWN;
            batch.sprite_scaled(
                &self.gui.atlas,
                pad_cursor::SPRITE,
                left,
                top,
                size,
                size,
                u32::MAX,
            );
        }
        // The carried stack: at the pointer, or floating over the focused slot or recipe.
        if let Some(carried) = self.carried.clone() {
            if pad {
                let [x, y, ..] = place
                    .or_else(|| layout.rect_of(Hit::Row(view.selected)))
                    .unwrap_or(layout.bounds);
                self.draw_item(batch, &carried, x as f32 + 5.0, y as f32 - 3.0);
            } else {
                self.draw_carried(batch);
            }
        }
        if pad {
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
            "focus": format!("{:?}", state.focus).to_lowercase(),
            "mode": state.mode,
            "furnace": self.legacy_furnace().map(|b| format!("{b:?}")),
            "cutter": self.legacy_cutter(),
            "cutting": self.legacy_cutting(),
            "smith": self.legacy_smith(),
            "brewing": self.legacy_brewing(),
            "loom": self.legacy_loom(),
            "anvil": self.legacy_anvil().then(|| self.pad.legacy.anvil.status()),
            "building": self.legacy_build_running(),
            "cursor": state.cursor,
            "typing": state.typing,
            "only_craftable": state.only_craftable,
            "compact": state.layout.as_ref().is_some_and(|l| l.compact),
        })
    }

    /// The display ids of every recipe in the row's slots on screen.
    fn legacy_in_sight(&self, layout: &Layout, stock: &Stacked) -> Vec<i32> {
        // A builder's row, or a stonecutter's, holds no book recipes.
        if self.legacy_builder().is_some() || self.legacy_station() {
            return Vec::new();
        }
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
        let glyph = |job| self.menu_glyph(job);
        let row =
            |jobs: &[MenuJob], key: &str| HintRow::new(jobs.iter().map(|&j| glyph(j)), tr(key));
        // A smithing table stages its cells as a builder does.
        let builder = self.legacy_builder().is_some()
            || self.legacy_smith()
            || self.legacy_anvil()
            || self.legacy_loom();
        let furnace = self.legacy_furnace().is_some();
        let cutter = self.legacy_cutter();
        let (craft, craft_all) = if cutter {
            ("horizon.legacy_craft.cut", "horizon.legacy_craft.cut_all")
        } else if furnace {
            (
                "horizon.legacy_craft.smelt",
                "horizon.legacy_craft.smelt_all",
            )
        } else {
            (
                "horizon.legacy_craft.craft",
                "horizon.legacy_craft.craft_all",
            )
        };
        // Each row with how much it's needed: when they don't all fit, the highest number goes.
        let rows: Vec<(u8, HintRow)> = match self.pad.legacy.focus {
            // Vanilla's slot jobs, as the inventory's guide names them; a furnace's slots too.
            focus
                if focus == Area::Inventory
                    || ((furnace || cutter || self.legacy_brewing()) && focus == Area::Grid) =>
            {
                let carrying = self.carried.is_some();
                let pick = if carrying { "put" } else { "take" };
                let half = if carrying { "put_one" } else { "take_half" };
                vec![
                    (
                        0,
                        row(
                            &[MenuJob::Confirm],
                            &format!("horizon.controller.guide.{pick}"),
                        ),
                    ),
                    (
                        1,
                        row(
                            &[MenuJob::Alternate],
                            &format!("horizon.controller.guide.{half}"),
                        ),
                    ),
                    (
                        2,
                        row(&[MenuJob::Extra], "horizon.controller.guide.quick_move"),
                    ),
                    (3, row(&[MenuJob::View], "horizon.controller.guide.drop")),
                    (0, row(&[MenuJob::Back], "horizon.legacy_craft.recipes")),
                ]
            }
            Area::Grid if builder => vec![
                (
                    0,
                    row(&[MenuJob::Confirm], "horizon.legacy_craft.next_item"),
                ),
                (
                    1,
                    row(&[MenuJob::Alternate], "horizon.legacy_craft.prev_item"),
                ),
                (2, row(&[MenuJob::Extra], "horizon.legacy_craft.craft_all")),
                (0, row(&[MenuJob::Back], "horizon.legacy_craft.recipes")),
            ],
            Area::Grid => vec![
                (0, row(&[MenuJob::Confirm], "horizon.legacy_craft.craft")),
                (1, row(&[MenuJob::Extra], "horizon.legacy_craft.craft_all")),
                (0, row(&[MenuJob::Back], "horizon.legacy_craft.recipes")),
            ],
            Area::Row => {
                let filter = if self.pad.legacy.only_craftable {
                    "gui.recipebook.toggleRecipes.craftable"
                } else {
                    "gui.recipebook.toggleRecipes.all"
                };
                let mut rows = vec![
                    (0, row(&[MenuJob::Confirm], craft)),
                    (3, row(&[MenuJob::Extra], craft_all)),
                ];
                if furnace {
                    rows.push((
                        1,
                        row(&[MenuJob::Alternate], "horizon.legacy_craft.add_fuel"),
                    ));
                }
                if !builder {
                    if !furnace {
                        rows.push((6, row(&[MenuJob::Alternate], filter)));
                    }
                    rows.push((
                        1,
                        row(
                            &[MenuJob::PagePrev, MenuJob::PageNext],
                            "horizon.legacy_craft.category",
                        ),
                    ));
                }
                rows.extend([
                    (
                        2,
                        row(
                            &[MenuJob::GroupPrev, MenuJob::GroupNext],
                            "horizon.legacy_craft.modes",
                        ),
                    ),
                    (4, row(&[MenuJob::View], "horizon.legacy_craft.search")),
                    (
                        5,
                        row(&[MenuJob::LeftStick], "horizon.legacy_craft.classic"),
                    ),
                    (0, row(&[MenuJob::Back], "gui.back")),
                ]);
                rows
            }
            // Taken by the slot arm above.
            Area::Inventory => Vec::new(),
        };
        // LT and RT only with a type to turn to (a furnace has just itself).
        let mut rows = rows;
        // A loom: Weave for Craft, no tabs or search.
        if self.legacy_loom() {
            let (craft, all, search) = (
                tr("horizon.legacy_craft.craft"),
                tr("horizon.legacy_craft.craft_all"),
                tr("horizon.legacy_craft.search"),
            );
            rows.retain(|(_, r)| r.caption != search);
            for (_, r) in &mut rows {
                if r.caption == craft || r.caption == all {
                    r.caption = tr("horizon.legacy_craft.weave");
                }
            }
            rows.dedup_by(|a, b| a.1.caption == b.1.caption);
        }
        // A smithing table: Smith for Craft, its two tabs on LB and RB, no search.
        if self.legacy_smith() {
            let (craft, all, search) = (
                tr("horizon.legacy_craft.craft"),
                tr("horizon.legacy_craft.craft_all"),
                tr("horizon.legacy_craft.search"),
            );
            rows.retain(|(_, r)| r.caption != search);
            for (_, r) in &mut rows {
                if r.caption == craft || r.caption == all {
                    r.caption = tr("horizon.legacy_craft.smith");
                }
            }
            rows.dedup_by(|a, b| a.1.caption == b.1.caption);
            if self.pad.legacy.focus == Area::Row {
                let tabs = HintRow::new(
                    [
                        self.menu_glyph(MenuJob::PagePrev),
                        self.menu_glyph(MenuJob::PageNext),
                    ],
                    tr("horizon.legacy_craft.smith_tabs"),
                );
                rows.insert(1, (1, tabs));
            }
        }
        // A brewing stand: Add, Fill Bottles, Add Fuel; no tabs, filter or search.
        if self.legacy_brewing() && self.pad.legacy.focus == Area::Row {
            rows = vec![
                (
                    0,
                    HintRow::new(
                        [self.menu_glyph(MenuJob::Confirm)],
                        tr("horizon.legacy_craft.add"),
                    ),
                ),
                (
                    1,
                    HintRow::new(
                        [self.menu_glyph(MenuJob::Extra)],
                        tr("horizon.legacy_craft.fill_bottles"),
                    ),
                ),
                (
                    2,
                    HintRow::new(
                        [self.menu_glyph(MenuJob::Alternate)],
                        tr("horizon.legacy_craft.add_fuel"),
                    ),
                ),
                (
                    5,
                    HintRow::new(
                        [self.menu_glyph(MenuJob::LeftStick)],
                        tr("horizon.legacy_craft.classic"),
                    ),
                ),
                (
                    0,
                    HintRow::new([self.menu_glyph(MenuJob::Back)], tr("gui.back")),
                ),
            ];
        }
        // An anvil: Use Anvil for Craft, X and View rename, no tabs or search.
        if self.legacy_anvil() {
            let (craft, all, search) = (
                tr("horizon.legacy_craft.craft"),
                tr("horizon.legacy_craft.craft_all"),
                tr("horizon.legacy_craft.search"),
            );
            for (_, r) in &mut rows {
                if r.caption == craft || r.caption == all {
                    r.caption = tr("horizon.legacy_craft.use_anvil");
                } else if r.caption == search {
                    r.caption = tr("horizon.legacy_craft.rename");
                }
            }
            rows.dedup_by(|a, b| a.1.caption == b.1.caption);
        }
        // A stonecutter's tabs are its materials; it has no filter and no search.
        if self.legacy_cutter() {
            let (filters, search) = (
                [
                    tr("gui.recipebook.toggleRecipes.craftable"),
                    tr("gui.recipebook.toggleRecipes.all"),
                ],
                tr("horizon.legacy_craft.search"),
            );
            rows.retain(|(_, r)| !filters.contains(&r.caption) && r.caption != search);
            let category = tr("horizon.legacy_craft.category");
            for (_, r) in &mut rows {
                if r.caption == category {
                    r.caption = tr("horizon.legacy_craft.material");
                }
            }
        }
        if self.legacy_modes().len() <= 1 {
            let types = tr("horizon.legacy_craft.modes");
            rows.retain(|(_, r)| r.caption != types);
        }
        self.legacy_draw_hints(batch, y, rows)
    }

    fn legacy_draw_hints(&mut self, batch: &mut HudBatch, y: f32, mut rows: Vec<(u8, HintRow)>) {
        let atlas = &self.gui.atlas;
        let look = Look {
            side: Side::Leading,
            ..hint::GUIDE
        };
        const BETWEEN: f32 = 8.0;
        const EDGE: f32 = 6.0;
        // The rows that fit, from the right; the least needed (the filter) goes first.
        let room = batch.width - 2.0 * EDGE;
        let total = |rows: &[(u8, HintRow)]| {
            rows.iter()
                .map(|(_, r)| hint::plated_width(atlas, r, &look))
                .sum::<f32>()
                + BETWEEN * rows.len().saturating_sub(1) as f32
        };
        // The least needed go first until they fit; what's needed most (0) always stays.
        while total(&rows) > room {
            let Some(least) = rows
                .iter()
                .enumerate()
                .filter(|(_, (need, _))| *need > 0)
                .max_by_key(|(i, (need, _))| (*need, *i))
                .map(|(i, _)| i)
            else {
                break;
            };
            rows.remove(least);
        }
        // On the player's text background (Accessibility), as the gameplay guide's rows are.
        let alpha = (self.settings.text_background(0.4) >> 24) as u8;
        const ROW_H: f32 = 11.0;
        // Centred under the panel, as the screen is.
        let mut x = ((batch.width - total(&rows)) / 2.0).max(EDGE).round();
        for (_, row) in &rows {
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
        let shown = screen::tabs_shown(
            width,
            height,
            self.pad.legacy.tabs.len(),
            self.legacy_modes().len(),
        );
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
        // A stonecutter: materials for tabs, its cuts along the row.
        if self.legacy_cutter() {
            self.legacy_cutter_view(&mut view);
            return view;
        }
        if self.legacy_smith() {
            self.legacy_smith_view(&mut view);
            return view;
        }
        if self.legacy_anvil() {
            self.legacy_anvil_view(&mut view);
            return view;
        }
        if self.legacy_brewing() {
            self.legacy_brew_view(&mut view);
            return view;
        }
        if self.legacy_loom() {
            self.legacy_loom_view(&mut view);
            return view;
        }
        // A builder: its own row and grid, no category tabs.
        if let Some(kind) = self.legacy_builder() {
            view.tab_page = None;
            view.search = None;
            self.legacy_build_view(&mut view, kind);
            return view;
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
        if self.legacy_furnace().is_some() {
            self.legacy_furnace_view(&mut view, craftable);
            return view;
        }
        let (cells, missing) = grid_cells(book, entry_index, grid, &stock, cycle);
        view.cells = cells;
        let carrying = self.carried.is_some();
        view.note = Some(if carrying {
            // A craft can't start with something in hand (`direct_craft_start`): say so here.
            (tr("horizon.legacy_craft.holding"), screen::MISSING)
        } else if craftable {
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

/// A grid cell one step that way, stopping at the edges.
fn grid_step(cell: usize, side: usize, button: Button) -> usize {
    let (row, col) = (cell / side, cell % side);
    let (row, col) = match button {
        Button::DPadLeft => (row, col.saturating_sub(1)),
        Button::DPadRight => (row, (col + 1).min(side - 1)),
        Button::DPadUp => (row.saturating_sub(1), col),
        _ => ((row + 1).min(side - 1), col),
    };
    row * side + col
}

/// An inventory slot one step that way, as the slots are laid out: three rows of the inventory
/// (slots 9-35) over the hotbar (0-8). Left and right wrap along a row; up and down stop.
fn inventory_step(slot: usize, button: Button) -> usize {
    // Shown row (0-3, the hotbar last) and column.
    let (row, col) = if slot < 9 {
        (3, slot)
    } else {
        ((slot - 9) / 9, (slot - 9) % 9)
    };
    let (row, col) = match button {
        Button::DPadLeft => (row, (col + 8) % 9),
        Button::DPadRight => (row, (col + 1) % 9),
        Button::DPadUp => (row.saturating_sub(1), col),
        _ => ((row + 1).min(3), col),
    };
    if row == 3 { col } else { 9 + row * 9 + col }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inventory_cursor_walks_the_slots_as_they_are_drawn() {
        // The hotbar is the bottom row: up from its first slot is the inventory's last row.
        assert_eq!(inventory_step(0, Button::DPadUp), 27);
        assert_eq!(inventory_step(27, Button::DPadDown), 0);
        assert_eq!(inventory_step(9, Button::DPadUp), 9);
        assert_eq!(inventory_step(4, Button::DPadDown), 4);
        assert_eq!(inventory_step(8, Button::DPadRight), 0);
        assert_eq!(inventory_step(9, Button::DPadLeft), 17);
    }

    #[test]
    fn the_grid_cursor_stops_at_the_edges() {
        assert_eq!(grid_step(0, 3, Button::DPadLeft), 0);
        assert_eq!(grid_step(4, 3, Button::DPadRight), 5);
        assert_eq!(grid_step(5, 3, Button::DPadRight), 5);
        assert_eq!(grid_step(1, 2, Button::DPadDown), 3);
    }
}
