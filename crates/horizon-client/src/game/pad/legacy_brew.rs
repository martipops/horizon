//! The controller crafting screen at a brewing stand: the row is the brewing ingredients the
//! player holds, the grid's place the stand itself (its three bottles, ingredient and fuel, with
//! its fuel bar, bubbles and brew arrow), vanilla's clicks with the focus there.
//!
//! A puts one of the selected ingredient in (pick up, right-click one in, put the rest back),
//! Y fills the empty bottle places from the bottles held, X puts blaze powder in: vanilla's own
//! clicks and quick moves, as a player makes them. What the ingredient brews from the bottles in
//! the stand is the game's own (`Brewing::find`).
use super::super::Game;
use horizon_client::{
    containers::BlockEntity,
    gamepad::{self, Button},
    items::ItemStack,
    legacy_craft_screen::{self as screen, Area, Cell, RecipeSlot, View},
};

/// The stand's slots, as the game numbers an open container's: bottles, ingredient, fuel.
pub(super) const SLOTS: [usize; 5] = [300, 301, 302, 303, 304];
/// Vanilla's brew time (`BrewingStandBlockEntity.BREWING_TIME_DEFAULT`) and fuel per powder.
const BREW_TICKS: f32 = 400.0;
const FUEL_USES: f32 = 20.0;

/// The brewing stand's state between frames.
#[derive(Default)]
pub(in crate::game) struct Brew {
    selected: usize,
}

impl Game<'_> {
    /// The open screen is a brewing stand's.
    pub(in crate::game) fn legacy_brewing(&self) -> bool {
        self.screen == super::super::Screen::Inventory
            && matches!(self.container_kind(), Some(BlockEntity::Brewing(_)))
    }

    /// The brewing ingredients held, one of each.
    fn legacy_reagents(&self) -> Vec<ItemStack> {
        let brewing = &self.stations().brewing;
        let mut out: Vec<ItemStack> = Vec::new();
        for stack in self.inventory[..36].iter().flatten() {
            if brewing.is_reagent(stack) && !out.iter().any(|s| s.same_item_same_components(stack))
            {
                let mut one = stack.clone();
                one.count = 1;
                out.push(one);
            }
        }
        out
    }

    /// What an ingredient brews from the bottles in the stand (the first it changes).
    fn legacy_brews(&mut self, reagent: &ItemStack) -> Option<ItemStack> {
        let bottles: Vec<ItemStack> = SLOTS[..3]
            .iter()
            .filter_map(|&s| self.slot_view(s))
            .collect();
        let brewing = &self.stations().brewing;
        bottles
            .iter()
            .find_map(|b| brewing.find(b, reagent).map(|r| r.output.clone()))
    }

    pub(in crate::game) fn legacy_brew_step(&mut self, button: Button) {
        let n = self.legacy_reagents().len();
        if n == 0 {
            return;
        }
        let state = &mut self.pad.legacy.brew;
        let at = state.selected.min(n - 1);
        state.selected = match button {
            Button::DPadLeft => (at + n - 1) % n,
            Button::DPadRight => (at + 1) % n,
            _ => return,
        };
    }

    /// A press on the stand's row: whether it was the stand's.
    pub(in crate::game) fn legacy_brew_button(&mut self, button: Button) -> bool {
        if self.pad.legacy.focus != Area::Row {
            return false;
        }
        match button {
            Button::South => self.legacy_brew_add(),
            Button::North => self.legacy_brew_bottles(),
            Button::West => self.legacy_brew_fuel(),
            Button::Select => {}
            _ => return false,
        }
        true
    }

    /// One of the selected ingredient into the stand's ingredient slot.
    fn legacy_brew_add(&mut self) {
        let reagents = self.legacy_reagents();
        let Some(reagent) = reagents.get(
            self.pad
                .legacy
                .brew
                .selected
                .min(reagents.len().saturating_sub(1)),
        ) else {
            return;
        };
        let taken = self.slot_view(SLOTS[3]);
        let fits = taken
            .as_ref()
            .is_none_or(|t| t.same_item_same_components(reagent));
        let from = (0..36).find(|&i| {
            self.inventory[i]
                .as_ref()
                .is_some_and(|s| s.same_item_same_components(reagent))
        });
        let (Some(from), true, None) = (from, fits, self.carried.as_ref()) else {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        };
        self.legacy_slot_click(from, false, false);
        self.legacy_slot_click(SLOTS[3], true, false);
        self.legacy_slot_click(from, false, false);
    }

    /// The empty bottle places filled from the bottles held: a quick move each, as vanilla's
    /// stand sends a potion to a free bottle place.
    fn legacy_brew_bottles(&mut self) {
        let brewing = &self.stations().brewing;
        let held: Vec<usize> = (0..36)
            .filter(|&i| {
                self.inventory[i]
                    .as_ref()
                    .is_some_and(|s| brewing.is_bottle(s))
            })
            .collect();
        let free = SLOTS[..3]
            .iter()
            .filter(|&&s| self.slot_view(s).is_none())
            .count();
        if held.is_empty() || free == 0 || self.carried.is_some() {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        for slot in held.into_iter().take(free) {
            self.legacy_slot_click(slot, false, true);
        }
    }

    /// Blaze powder (any stand fuel) into the fuel slot, by quick move.
    fn legacy_brew_fuel(&mut self) {
        let items = self.items();
        let from = (0..36).find(|&i| {
            self.inventory[i]
                .as_ref()
                .is_some_and(|s| horizon_client::brewing::brewing_fuel(items, s).is_some())
        });
        match (from, self.carried.is_some()) {
            (Some(from), false) => {
                self.legacy_slot_click(from, false, true);
            }
            _ => self.pad_rumble(gamepad::Rumble::RadialNothing),
        }
    }

    /// The stand's view: the ingredients held along the row, the stand below.
    pub(in crate::game) fn legacy_brew_view(&mut self, view: &mut View) {
        let tr = |key: &str| horizon_client::lang::get(key);
        view.search = None;
        view.tab_page = None;
        view.tabs.clear();
        view.title = tr("container.brewing");
        view.empty = tr("horizon.legacy_craft.no_ingredients");
        let (fuel, brew) = match self.container_kind() {
            Some(BlockEntity::Brewing(b)) => {
                let fuel = b.fuel as f32 / FUEL_USES;
                let brew = if b.brew_time > 0 {
                    1.0 - b.brew_time as f32 / BREW_TICKS
                } else {
                    0.0
                };
                (fuel.clamp(0.0, 1.0), brew.clamp(0.0, 1.0))
            }
            _ => (0.0, 0.0),
        };
        view.brewing = Some((fuel, brew));
        view.cells = SLOTS
            .iter()
            .map(|&s| {
                self.slot_view(s).map(|stack| Cell {
                    stack,
                    missing: false,
                })
            })
            .collect();
        let reagents = self.legacy_reagents();
        let selected = self
            .pad
            .legacy
            .brew
            .selected
            .min(reagents.len().saturating_sub(1));
        let mut row = Vec::new();
        for r in &reagents {
            let made = self.legacy_brews(r);
            row.push(RecipeSlot {
                craftable: made.is_some(),
                stack: r.clone(),
                fresh: false,
            });
        }
        view.row = row;
        view.selected = selected;
        let Some(reagent) = reagents.get(selected) else {
            return;
        };
        let made = self.legacy_brews(reagent);
        view.name = horizon_client::stations::hover_name(
            &self.pack.names,
            made.as_ref().unwrap_or(reagent),
        );
        let fuelled = fuel > 0.0 || self.slot_view(SLOTS[4]).is_some();
        view.note = Some(if self.carried.is_some() {
            (tr("horizon.legacy_craft.holding"), screen::MISSING)
        } else if brew > 0.0 {
            (tr("horizon.legacy_craft.brewing"), screen::HAVE)
        } else if !fuelled {
            (tr("horizon.legacy_craft.no_fuel"), screen::MISSING)
        } else if made.is_none() {
            (tr("horizon.legacy_craft.no_bottles"), screen::MISSING)
        } else {
            (tr("horizon.legacy_craft.ready"), screen::HAVE)
        });
    }
}

/// A brewing stand slot one step that way, as they're laid out: fuel and ingredient on top, the
/// three bottles below.
pub(super) fn brew_step(cell: usize, button: Button) -> usize {
    match (cell, button) {
        (4, Button::DPadRight) => 3,
        (3, Button::DPadLeft) => 4,
        (3 | 4, Button::DPadDown) => 1,
        (0..=2, Button::DPadUp) => 3,
        (0, Button::DPadRight) => 1,
        (1, Button::DPadRight) => 2,
        (2, Button::DPadLeft) => 1,
        (1, Button::DPadLeft) => 0,
        (cell, _) => cell.min(4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stand_cursor_moves_as_its_slots_are_laid_out() {
        assert_eq!(brew_step(4, Button::DPadRight), 3);
        assert_eq!(brew_step(3, Button::DPadDown), 1);
        assert_eq!(brew_step(1, Button::DPadLeft), 0);
        assert_eq!(brew_step(0, Button::DPadUp), 3);
        assert_eq!(brew_step(2, Button::DPadRight), 2);
    }
}
