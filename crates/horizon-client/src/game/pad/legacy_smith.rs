//! The controller crafting screen at a smithing table: two tabs, Upgrades (the transform
//! recipes: netherite gear) and Trims (one entry per trim template); the row is that tab's
//! recipes, the grid's place the table's template, base and material, staged from the inventory.
//!
//! With the focus on those three, A and X step a cell through what the inventory has that fits
//! it (another piece of armour, another material). The result is the game's own
//! (`StationRecipes::smithing_result`, vanilla's `SmithingMenu.createResult`). A smiths it: each
//! staged item goes into its slot by vanilla's clicks, the result is quick-moved out and the
//! slots emptied (`legacy_run_slots`), gated like Quick Craft.
use super::super::{
    Game,
    stations::{INPUT, Kind, RESULT},
};
use horizon_client::{
    crafting::Ingredient,
    gamepad::{self, Button},
    items::ItemStack,
    legacy_craft_screen::{self as screen, Area, Cell, RecipeSlot, View},
    stations::Smithing,
};
use std::collections::HashMap;

/// The smithing table's state between frames.
#[derive(Default)]
pub(in crate::game) struct Smith {
    /// 0: Upgrades, 1: Trims; each tab's selected recipe (none: the first that can be made).
    tab: usize,
    selected: [Option<usize>; 2],
    /// What's staged in each recipe's three cells, by recipe id.
    staged: HashMap<String, [Option<ItemStack>; 3]>,
}

/// A recipe's id and its three cells' ingredients (the template's and material's may be none).
fn parts(r: &Smithing) -> (&str, [Option<&Ingredient>; 3], bool) {
    match r {
        Smithing::Transform {
            id,
            template,
            base,
            addition,
            ..
        } => (
            id,
            [template.as_ref(), Some(base), addition.as_ref()],
            false,
        ),
        Smithing::Trim {
            id,
            template,
            base,
            addition,
            ..
        } => (id, [Some(template), Some(base), Some(addition)], true),
    }
}

impl Game<'_> {
    /// The open screen is a smithing table's.
    pub(in crate::game) fn legacy_smith(&self) -> bool {
        self.station
            .as_ref()
            .is_some_and(|s| s.kind == Kind::Smithing)
    }

    /// The selected tab's recipes (indices into the smithing recipes).
    fn legacy_smith_recipes(&self, tab: usize) -> Vec<usize> {
        let all = &self.stations().recipes.smithing;
        (0..all.len())
            .filter(|&i| parts(&all[i]).2 == (tab == 1))
            .collect()
    }

    /// The stacks of the inventory an ingredient takes, one of each.
    fn legacy_smith_candidates(&self, ingredient: &Ingredient) -> Vec<ItemStack> {
        let mut out: Vec<ItemStack> = Vec::new();
        for stack in self.inventory[..36].iter().flatten() {
            if ingredient.test(stack) && !out.iter().any(|s| s.same_item_same_components(stack)) {
                let mut one = stack.clone();
                one.count = 1;
                out.push(one);
            }
        }
        out
    }

    /// A recipe's staging: kept while the inventory holds it, else each cell's first candidate.
    fn legacy_smith_staged(&self, recipe: usize) -> [Option<ItemStack>; 3] {
        let r = &self.stations().recipes.smithing[recipe];
        let (id, cells, _) = parts(r);
        let held = |s: &ItemStack| {
            self.inventory[..36]
                .iter()
                .flatten()
                .any(|i| i.same_item_same_components(s))
        };
        if let Some(staged) = self.pad.legacy.smith.staged.get(id)
            && staged.iter().flatten().all(held)
            && staged.iter().any(Option::is_some)
        {
            return staged.clone();
        }
        let mut out: [Option<ItemStack>; 3] = Default::default();
        for (i, ingredient) in cells.iter().enumerate() {
            if let Some(ingredient) = ingredient {
                out[i] = self.legacy_smith_candidates(ingredient).into_iter().next();
            }
        }
        out
    }

    /// What a staging makes: the game's own smithing result.
    fn legacy_smith_result(&self, staged: &[Option<ItemStack>; 3]) -> Option<ItemStack> {
        let data = self.stations();
        data.recipes.smithing_result(
            &data.enchantments,
            staged[0].as_ref(),
            staged[1].as_ref(),
            staged[2].as_ref(),
        )
    }

    /// The selected recipe.
    fn legacy_smith_current(&self) -> Option<(Vec<usize>, usize)> {
        let state = &self.pad.legacy.smith;
        let recipes = self.legacy_smith_recipes(state.tab);
        let last = recipes.len().checked_sub(1)?;
        let selected = match state.selected[state.tab] {
            Some(s) => s.min(last),
            None => recipes
                .iter()
                .position(|&r| {
                    let staged = self.legacy_smith_staged(r);
                    self.legacy_smith_result(&staged).is_some()
                })
                .unwrap_or(0),
        };
        Some((recipes, selected))
    }

    /// LB and RB: Upgrades and Trims.
    pub(in crate::game) fn legacy_smith_turn(&mut self) {
        let state = &mut self.pad.legacy.smith;
        state.tab = 1 - state.tab.min(1);
    }

    /// Left and right along the recipes; in the grid, A and X step its cell.
    pub(in crate::game) fn legacy_smith_step(&mut self, button: Button) {
        let Some((recipes, selected)) = self.legacy_smith_current() else {
            return;
        };
        let n = recipes.len();
        let tab = self.pad.legacy.smith.tab;
        self.pad.legacy.smith.selected[tab] = Some(match button {
            Button::DPadLeft => (selected + n - 1) % n,
            Button::DPadRight => (selected + 1) % n,
            _ => return,
        });
    }

    /// A press at the smithing table: whether it was the table's.
    pub(in crate::game) fn legacy_smith_button(&mut self, button: Button) -> bool {
        match (self.pad.legacy.focus, button) {
            (Area::Grid, Button::South) => self.legacy_smith_cycle(true),
            (Area::Grid, Button::West) => self.legacy_smith_cycle(false),
            (Area::Row | Area::Grid, Button::North) | (Area::Row, Button::South) => {
                self.legacy_smith_go()
            }
            // No filter and no search here.
            (_, Button::Select) | (Area::Row, Button::West) => {}
            _ => return false,
        }
        true
    }

    /// The focused cell's staged item, stepped through what the inventory has that fits it.
    fn legacy_smith_cycle(&mut self, next: bool) {
        let Some((recipes, selected)) = self.legacy_smith_current() else {
            return;
        };
        let recipe = recipes[selected];
        let cell = self.pad.legacy.cursor.min(2);
        let r = self.stations().recipes.smithing[recipe].clone();
        let (id, cells, _) = parts(&r);
        let Some(ingredient) = cells[cell] else {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        };
        let choices = self.legacy_smith_candidates(ingredient);
        if choices.is_empty() {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        let mut staged = self.legacy_smith_staged(recipe);
        let at = staged[cell]
            .as_ref()
            .and_then(|s| choices.iter().position(|c| c.same_item_same_components(s)));
        let n = choices.len();
        let to = match (at, next) {
            (Some(at), true) => (at + 1) % n,
            (Some(at), false) => (at + n - 1) % n,
            (None, _) => 0,
        };
        staged[cell] = Some(choices[to].clone());
        self.pad.legacy.smith.staged.insert(id.to_owned(), staged);
    }

    /// A: the staged items into the table, the result out.
    fn legacy_smith_go(&mut self) {
        let Some((recipes, selected)) = self.legacy_smith_current() else {
            return;
        };
        let staged = self.legacy_smith_staged(recipes[selected]);
        if self.legacy_smith_result(&staged).is_none() {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        let puts = staged
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.clone().map(|s| (s, INPUT + i)))
            .collect();
        self.legacy_run_slots(puts, RESULT, (INPUT..INPUT + 3).collect(), None, None);
    }

    /// The smithing table's view: its two tabs, the recipes, the staged cells and the result.
    pub(in crate::game) fn legacy_smith_view(&mut self, view: &mut View) {
        let tr = |key: &str| horizon_client::lang::get(key);
        view.smith = true;
        view.search = None;
        view.tab_page = None;
        let tab = self.pad.legacy.smith.tab;
        view.tabs = vec![
            ItemStack::new("minecraft:netherite_upgrade_smithing_template", 1),
            ItemStack::new("minecraft:coast_armor_trim_smithing_template", 1),
        ];
        view.tab = tab;
        view.first_tab = 0;
        view.title = tr(if tab == 0 {
            "horizon.legacy_craft.upgrades"
        } else {
            "horizon.legacy_craft.trims"
        });
        view.empty = tr("horizon.legacy_craft.empty");
        let Some((recipes, selected)) = self.legacy_smith_current() else {
            return;
        };
        let all = self.stations().recipes.smithing.clone();
        let icon = |r: &Smithing, made: Option<ItemStack>| {
            made.unwrap_or_else(|| {
                let (_, cells, trim) = parts(r);
                // A trim's entry shows its template; an upgrade's, what it makes.
                let from = if trim { cells[0] } else { cells[1] };
                let item = from
                    .and_then(|i| i.order.first())
                    .map_or("minecraft:barrier", String::as_str);
                ItemStack::new(item, 1)
            })
        };
        view.row = recipes
            .iter()
            .map(|&r| {
                let staged = self.legacy_smith_staged(r);
                let made = self.legacy_smith_result(&staged);
                RecipeSlot {
                    craftable: made.is_some(),
                    stack: icon(&all[r], made),
                    fresh: false,
                }
            })
            .collect();
        view.selected = selected;
        let recipe = recipes[selected];
        let staged = self.legacy_smith_staged(recipe);
        self.pad
            .legacy
            .smith
            .staged
            .insert(parts(&all[recipe]).0.to_owned(), staged.clone());
        let made = self.legacy_smith_result(&staged);
        let (_, cells, _) = parts(&all[recipe]);
        view.cells = (0..3)
            .map(|i| match (&staged[i], cells[i]) {
                (Some(stack), _) => Some(Cell {
                    stack: stack.clone(),
                    missing: false,
                }),
                // Nothing of it held: what it takes, faded as missing.
                (None, Some(ingredient)) => ingredient.order.first().map(|item| Cell {
                    stack: ItemStack::new(item, 1),
                    missing: true,
                }),
                (None, None) => None,
            })
            .collect();
        view.name = horizon_client::stations::hover_name(
            &self.pack.names,
            &icon(&all[recipe], made.clone()),
        );
        view.result = Some(RecipeSlot {
            stack: icon(&all[recipe], made.clone()),
            craftable: made.is_some(),
            fresh: false,
        });
        view.note = Some(if self.carried.is_some() {
            (tr("horizon.legacy_craft.holding"), screen::MISSING)
        } else if self.legacy_build_running() {
            (tr("horizon.legacy_craft.crafting"), screen::HAVE)
        } else if made.is_none() {
            (tr("horizon.legacy_craft.missing_some"), screen::MISSING)
        } else {
            (tr("horizon.legacy_craft.ready"), screen::HAVE)
        });
    }
}
