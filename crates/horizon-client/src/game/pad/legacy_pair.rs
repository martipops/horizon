//! The controller crafting screen at the two-slot stations: a grindstone and a cartography table.
//! Both take an item and something with it; one module serves both.
//!
//! The row is what the player holds that the station works on (an enchanted or worn item, a
//! filled map); the grid's place is its two slots, the item and the addition staged from the
//! inventory, A and X stepping the addition with the focus there. A works it: both into the
//! station by vanilla's clicks, the result quick-moved out, the station emptied
//! (`legacy_run_slots`). A grindstone's result is the game's own (`grindstone_result`); a
//! cartography table's comes from its menu, so its preview names what the addition does.
use super::super::{
    Game,
    stations::{INPUT, Kind, RESULT},
};
use horizon_client::{
    gamepad::{self, Button},
    items::ItemStack,
    legacy_craft_screen::{self as screen, Area, Cell, RecipeSlot, View},
    stations::{grindstone_accepts, grindstone_result},
};

/// What a cartography table does with each addition: the item, its action's language key.
const CARTOGRAPHY: [(&str, &str); 3] = [
    ("minecraft:paper", "horizon.legacy_craft.map_zoom"),
    ("minecraft:map", "horizon.legacy_craft.map_copy"),
    ("minecraft:glass_pane", "horizon.legacy_craft.map_lock"),
];

/// A two-slot station's state between frames.
#[derive(Default)]
pub(in crate::game) struct Pair {
    selected: usize,
    addition: Option<ItemStack>,
    /// The item the addition was picked for.
    for_item: Option<ItemStack>,
}

impl super::legacy_craft::LegacyCraft {
    /// The staged addition, for `pad status`.
    pub(in crate::game) fn pair_addition(&self) -> Option<String> {
        self.pair.addition.as_ref().map(|a| a.item.to_string())
    }
}

impl Game<'_> {
    /// The open screen is a grindstone's or a cartography table's.
    pub(in crate::game) fn legacy_pair(&self) -> Option<Kind> {
        self.station
            .as_ref()
            .map(|s| s.kind)
            .filter(|k| matches!(k, Kind::Grindstone | Kind::Cartography))
    }

    /// The held stacks, one of each kind, that pass a test.
    fn legacy_held_kinds(&self, test: impl Fn(&ItemStack) -> bool) -> Vec<ItemStack> {
        let mut out: Vec<ItemStack> = Vec::new();
        for stack in self.inventory[..36].iter().flatten() {
            if test(stack) && !out.iter().any(|s| s.same_item_same_components(stack)) {
                let mut one = stack.clone();
                one.count = 1;
                out.push(one);
            }
        }
        out
    }

    /// The row: what the station works on.
    fn legacy_pair_items(&self, kind: Kind) -> Vec<ItemStack> {
        let items = self.items();
        match kind {
            Kind::Grindstone => self.legacy_held_kinds(|s| grindstone_accepts(items, s)),
            _ => self.legacy_held_kinds(|s| &*s.item == "minecraft:filled_map"),
        }
    }

    /// What can go with `item`: for a grindstone another of it (they combine), for a
    /// cartography table paper, an empty map or a glass pane.
    fn legacy_pair_additions(&self, kind: Kind, item: &ItemStack) -> Vec<ItemStack> {
        match kind {
            Kind::Grindstone => {
                let held: u32 = self.inventory[..36]
                    .iter()
                    .flatten()
                    .filter(|s| s.same_item_same_components(item))
                    .map(|s| s.count)
                    .sum();
                let data = self.stations();
                self.legacy_held_kinds(|s| {
                    s.item == item.item
                        && (!s.same_item_same_components(item) || held >= 2)
                        && grindstone_result(&data.enchantments, self.items(), Some(item), Some(s))
                            .is_some()
                })
            }
            _ => self.legacy_held_kinds(|s| CARTOGRAPHY.iter().any(|(i, _)| *i == &*s.item)),
        }
    }

    /// What the station makes of the item and addition (a cartography table's: the map, as a
    /// stand-in for the menu's result), and a note on what it does.
    fn legacy_pair_result(
        &self,
        kind: Kind,
        item: &ItemStack,
        addition: Option<&ItemStack>,
    ) -> (Option<ItemStack>, Option<&'static str>) {
        match kind {
            Kind::Grindstone => {
                let data = self.stations();
                let made =
                    grindstone_result(&data.enchantments, self.items(), Some(item), addition);
                (made, Some("horizon.legacy_craft.grind"))
            }
            _ => {
                let action = addition.and_then(|a| {
                    CARTOGRAPHY
                        .iter()
                        .find(|(i, _)| *i == &*a.item)
                        .map(|(_, key)| *key)
                });
                let mut made = item.clone();
                // A copy makes two.
                if action == Some(CARTOGRAPHY[1].1) {
                    made.count = 2;
                }
                (action.map(|_| made), action)
            }
        }
    }

    /// The selected item, the addition kept only while it's for that item.
    fn legacy_pair_current(&mut self, kind: Kind) -> Option<ItemStack> {
        let items = self.legacy_pair_items(kind);
        let selected = self
            .pad
            .legacy
            .pair
            .selected
            .min(items.len().checked_sub(1)?);
        let item = items[selected].clone();
        let same = self
            .pad
            .legacy
            .pair
            .for_item
            .as_ref()
            .is_some_and(|f| f.same_item_same_components(&item));
        if !same {
            // A grindstone grinds the item alone until a second is picked (merging two uses one
            // up); a cartography table needs its addition, so the first held comes in.
            let first = match kind {
                Kind::Grindstone => None,
                _ => self.legacy_pair_additions(kind, &item).into_iter().next(),
            };
            let state = &mut self.pad.legacy.pair;
            state.addition = first;
            state.for_item = Some(item.clone());
        }
        Some(item)
    }

    pub(in crate::game) fn legacy_pair_step(&mut self, button: Button) {
        let Some(kind) = self.legacy_pair() else {
            return;
        };
        let n = self.legacy_pair_items(kind).len();
        if n == 0 {
            return;
        }
        let state = &mut self.pad.legacy.pair;
        let at = state.selected.min(n - 1);
        state.selected = match button {
            Button::DPadLeft => (at + n - 1) % n,
            Button::DPadRight => (at + 1) % n,
            _ => return,
        };
    }

    /// A press at a two-slot station: whether it was the station's.
    pub(in crate::game) fn legacy_pair_button(&mut self, button: Button) -> bool {
        let Some(kind) = self.legacy_pair() else {
            return false;
        };
        match (self.pad.legacy.focus, button) {
            (Area::Grid, Button::South) => self.legacy_pair_cycle(kind, true),
            (Area::Grid, Button::West) => self.legacy_pair_cycle(kind, false),
            (Area::Row | Area::Grid, Button::North) | (Area::Row, Button::South) => {
                self.legacy_pair_go(kind)
            }
            (_, Button::Select) | (Area::Row, Button::West) => {}
            _ => return false,
        }
        true
    }

    /// The addition stepped through what fits (and nothing, for a grindstone).
    fn legacy_pair_cycle(&mut self, kind: Kind, next: bool) {
        let Some(item) = self.legacy_pair_current(kind) else {
            return;
        };
        if self.pad.legacy.cursor != 1 {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        let mut choices: Vec<Option<ItemStack>> = self
            .legacy_pair_additions(kind, &item)
            .into_iter()
            .map(Some)
            .collect();
        if kind == Kind::Grindstone {
            choices.insert(0, None);
        }
        if choices.is_empty() {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        let now = &self.pad.legacy.pair.addition;
        let at = choices.iter().position(|c| match (c, now) {
            (Some(a), Some(b)) => a.same_item_same_components(b),
            (None, None) => true,
            _ => false,
        });
        let n = choices.len();
        let to = match (at, next) {
            (Some(at), true) => (at + 1) % n,
            (Some(at), false) => (at + n - 1) % n,
            (None, _) => 0,
        };
        self.pad.legacy.pair.addition = choices[to].clone();
    }

    /// A: the item and its addition in, the result out.
    fn legacy_pair_go(&mut self, kind: Kind) {
        let Some(item) = self.legacy_pair_current(kind) else {
            return;
        };
        let addition = self.pad.legacy.pair.addition.clone();
        if self
            .legacy_pair_result(kind, &item, addition.as_ref())
            .0
            .is_none()
        {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        let mut puts = vec![(item, INPUT)];
        puts.extend(addition.map(|a| (a, INPUT + 1)));
        self.legacy_run_slots(puts, RESULT, vec![INPUT, INPUT + 1], None, None);
    }

    /// A two-slot station's view: the items it works on along the row, its slots below.
    pub(in crate::game) fn legacy_pair_view(&mut self, view: &mut View, kind: Kind) {
        let tr = |key: &str| horizon_client::lang::get(key);
        view.smith = true;
        view.slot_hints = Some(match kind {
            Kind::Grindstone => ["", "", ""],
            _ => ["", "", ""],
        });
        view.search = None;
        view.tab_page = None;
        view.tabs.clear();
        view.title = tr(if kind == Kind::Grindstone {
            "container.grindstone_title"
        } else {
            "container.cartography_table"
        });
        view.empty = tr("horizon.legacy_craft.nothing_to_work");
        view.cells = vec![None, None];
        let Some(item) = self.legacy_pair_current(kind) else {
            return;
        };
        let items = self.legacy_pair_items(kind);
        view.row = items
            .iter()
            .map(|s| RecipeSlot {
                stack: s.clone(),
                craftable: true,
                fresh: false,
            })
            .collect();
        view.selected = self.pad.legacy.pair.selected.min(items.len() - 1);
        let addition = self.pad.legacy.pair.addition.clone();
        let (made, action) = self.legacy_pair_result(kind, &item, addition.as_ref());
        view.cells = vec![
            Some(Cell {
                stack: item.clone(),
                missing: false,
            }),
            addition.map(|stack| Cell {
                stack,
                missing: false,
            }),
        ];
        view.name = horizon_client::stations::hover_name(&self.pack.names, &item);
        view.result = made.clone().map(|stack| RecipeSlot {
            stack,
            craftable: true,
            fresh: false,
        });
        view.note = Some(if self.carried.is_some() {
            (tr("horizon.legacy_craft.holding"), screen::MISSING)
        } else if self.legacy_build_running() {
            (tr("horizon.legacy_craft.crafting"), screen::HAVE)
        } else if made.is_none() {
            (tr("horizon.legacy_craft.not_yet"), screen::MISSING)
        } else {
            (
                tr(action.unwrap_or("horizon.legacy_craft.ready")),
                screen::HAVE,
            )
        });
    }
}
