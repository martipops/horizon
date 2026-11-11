//! The controller crafting screen at a furnace, blast furnace or smoker: its recipe book's
//! categories as the tabs, the smelting recipes the player knows along the row, and the furnace
//! itself in the grid's place: its input, fuel and output slots (vanilla's clicks with the focus
//! there) under its flame and arrow.
//!
//! A puts one smelt's ingredient in and Y a stack, as a click on the recipe book's button does
//! (`ServerboundPlaceRecipePacket`: what any player's book sends, so no server setting gates it).
//! X puts fuel in: vanilla's quick move of a fuel stack, which the furnace menu sends to its fuel
//! slot.
use super::super::Game;
use horizon_client::{
    containers::BlockEntity,
    gamepad::Button,
    items::ItemStack,
    legacy_craft::CraftTab,
    legacy_craft_screen::{self as screen, Cell, Furnace, View},
    recipe_book::{BookType, Category, Tab},
};

/// The furnace menu's slots, as the game numbers an open container's: input, fuel, output.
pub(super) const SLOTS: [usize; 3] = [300, 301, 302];

impl Game<'_> {
    /// The furnace-family book the open screen has: none at a crafting grid.
    pub(in crate::game) fn legacy_furnace(&self) -> Option<BookType> {
        self.active_book().filter(|b| *b != BookType::Crafting)
    }

    /// The furnace book's tabs: search first, then its categories, each with the recipes the
    /// player knows.
    pub(in crate::game) fn legacy_furnace_tabs(&self, book_type: BookType) -> Vec<CraftTab> {
        let Some((book, client)) = self.crafting_book() else {
            return Vec::new();
        };
        let tr = |key: &str| horizon_client::lang::get(key);
        book_type
            .tabs()
            .into_iter()
            .map(|tab| {
                let (id, title) = match tab {
                    Tab::Search => (
                        horizon_client::legacy_craft::SEARCH.to_owned(),
                        tr("itemGroup.search"),
                    ),
                    Tab::Category(c) => (c.name().to_owned(), tr(category_key(c))),
                };
                CraftTab {
                    id,
                    title,
                    icon: tab.icons().0.to_owned(),
                    groups: client.collections(book, book_type, tab),
                }
            })
            .filter(|t| !t.groups.is_empty() || t.id == horizon_client::legacy_craft::SEARCH)
            .collect()
    }

    /// A or Y on a smelting recipe: its ingredient into the furnace, one smelt's or a stack.
    pub(in crate::game) fn legacy_smelt(&mut self, book_type: BookType, entry: usize, all: bool) {
        if self.carried.is_some() || !self.recipe_craftable_in(book_type, entry) {
            self.pad_rumble(horizon_client::gamepad::Rumble::RadialNothing);
            return;
        }
        self.recipe_place_in(book_type, entry, all);
    }

    /// X: fuel into the furnace, a stack quick-moved there; not the selected recipe's own
    /// ingredient (logs burn, but here they're to smelt) while another fuel's at hand.
    pub(in crate::game) fn legacy_add_fuel(&mut self, ingredient: Option<&str>) {
        let fuel = |s: &ItemStack| self.items().get(&s.item).is_some_and(|i| i.fuel.is_some());
        let slots: Vec<usize> = (0..36)
            .filter(|&i| self.inventory[i].as_ref().is_some_and(fuel))
            .collect();
        let pick = slots
            .iter()
            .copied()
            .find(|&i| {
                self.inventory[i]
                    .as_ref()
                    .is_some_and(|s| Some(&*s.item) != ingredient)
            })
            .or_else(|| slots.first().copied());
        // Picked up and put on the fuel slot: vanilla's quick move would send a smeltable fuel
        // (logs) to the input. What doesn't fit, or the fuel it swaps out, goes back.
        match (pick, self.carried.is_some()) {
            (Some(slot), false) => {
                self.legacy_slot_click(slot, false, false);
                self.legacy_slot_click(SLOTS[1], false, false);
                if self.carried.is_some() {
                    self.legacy_slot_click(slot, false, false);
                }
            }
            _ => self.pad_rumble(horizon_client::gamepad::Rumble::RadialNothing),
        }
    }

    /// The furnace in the grid's place: its slots, flame and arrow, and a note on its fuel.
    pub(in crate::game) fn legacy_furnace_view(&mut self, view: &mut View, craftable: bool) {
        let tr = |key: &str| horizon_client::lang::get(key);
        let (lit, cook) = match self.container_kind() {
            Some(BlockEntity::Furnace(f)) => {
                let total = if f.lit_total == 0 { 200 } else { f.lit_total };
                let lit = f.lit_remaining as f32 / total as f32;
                let cook = if f.cooking_total > 0 {
                    f.cooking_timer as f32 / f.cooking_total as f32
                } else {
                    0.0
                };
                (lit.clamp(0.0, 1.0), cook.clamp(0.0, 1.0))
            }
            _ => (0.0, 0.0),
        };
        view.furnace = Some(Furnace { lit, cook });
        view.cells = SLOTS
            .iter()
            .map(|&slot| {
                self.slot_view(slot).map(|stack| Cell {
                    stack,
                    missing: false,
                })
            })
            .collect();
        // The recipe's preview is the row's; the grid's place is the furnace's own.
        view.result = None;
        let fuel_here = self.slot_view(SLOTS[1]).is_some();
        let fuel_held = (0..36).any(|i| {
            self.inventory[i]
                .as_ref()
                .is_some_and(|s| self.items().get(&s.item).is_some_and(|i| i.fuel.is_some()))
        });
        view.note = Some(if self.carried.is_some() {
            (tr("horizon.legacy_craft.holding"), screen::MISSING)
        } else if lit > 0.0 && cook > 0.0 {
            (tr("horizon.legacy_craft.smelting"), screen::HAVE)
        } else if !fuel_here && lit == 0.0 && !fuel_held {
            (tr("horizon.legacy_craft.no_fuel"), screen::MISSING)
        } else if !craftable {
            (tr("horizon.legacy_craft.missing_some"), screen::MISSING)
        } else {
            (tr("horizon.legacy_craft.ready"), screen::HAVE)
        });
    }
}

/// A furnace slot one step that way: up and down between the input and the fuel, right to the
/// output, left back to the input.
pub(super) fn furnace_step(cell: usize, button: Button) -> usize {
    match (cell, button) {
        (0, Button::DPadDown) => 1,
        (1, Button::DPadUp) => 0,
        (0 | 1, Button::DPadRight) => 2,
        (2, Button::DPadLeft) => 0,
        (cell, _) => cell.min(2),
    }
}

/// A furnace book category's title.
fn category_key(c: Category) -> &'static str {
    match c {
        Category::FurnaceFood | Category::SmokerFood => "horizon.legacy_craft.furnace.food",
        Category::FurnaceBlocks | Category::BlastBlocks => "horizon.legacy_craft.furnace.blocks",
        _ => "horizon.legacy_craft.furnace.misc",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_furnace_cursor_moves_as_its_slots_are_laid_out() {
        assert_eq!(furnace_step(0, Button::DPadDown), 1);
        assert_eq!(furnace_step(1, Button::DPadRight), 2);
        assert_eq!(furnace_step(2, Button::DPadLeft), 0);
        assert_eq!(furnace_step(2, Button::DPadDown), 2);
    }
}
