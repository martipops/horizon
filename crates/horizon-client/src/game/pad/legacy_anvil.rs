//! The controller crafting screen at an anvil: the row is what the player holds (anything can be
//! renamed), the grid's place the anvil itself: its rename box over the item and what's added to
//! it (a material to repair it, a book or a second one to combine), the result and its level cost.
//!
//! In the grid A and X step the addition through what the inventory has that does something to
//! the item (or nothing, for a rename alone); X on the row, or a click on the box, types the name
//! (the on-screen keyboard for a pad). The preview and its cost are the game's own
//! (`stations::anvil_result`, vanilla's `AnvilMenu.createResult`). A works it: the item and the
//! addition into the anvil by vanilla's clicks, the name sent as typing it there does, the result
//! quick-moved out. Too few levels, or too expensive, is refused before anything moves.
use super::super::{
    Game,
    stations::{INPUT, Kind, RESULT},
};
use horizon_client::{
    gamepad::{self, Button},
    items::ItemStack,
    legacy_craft_screen::{self as screen, Area, Cell, RecipeSlot, View},
    stations::{AnvilResult, anvil_result},
};

/// Vanilla's limit: a cost this high is "Too Expensive!" outside creative.
const TOO_EXPENSIVE: i32 = 40;

/// The anvil's state between frames.
#[derive(Default)]
pub(in crate::game) struct Anvil {
    /// The row's selected item (none: the first), and the addition staged for it.
    selected: usize,
    addition: Option<ItemStack>,
    /// The name the result gets, and the item it was started from.
    name: String,
    named: Option<ItemStack>,
}

impl Anvil {
    /// The anvil as `pad status` reports it.
    pub(in crate::game) fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "addition": self.addition.as_ref().map(|a| a.item.to_string()),
        })
    }
}

impl Game<'_> {
    /// The open screen is an anvil's.
    pub(in crate::game) fn legacy_anvil(&self) -> bool {
        self.station.as_ref().is_some_and(|s| s.kind == Kind::Anvil)
    }

    /// What the player holds, one of each kind (item and components): the row.
    fn legacy_anvil_items(&self) -> Vec<ItemStack> {
        let mut out: Vec<ItemStack> = Vec::new();
        for stack in self.inventory[..36].iter().flatten() {
            if !out.iter().any(|s| s.same_item_same_components(stack)) {
                let mut one = stack.clone();
                one.count = 1;
                out.push(one);
            }
        }
        out
    }

    /// The anvil's answer for an item, an addition and a name.
    fn legacy_anvil_result(
        &self,
        item: &ItemStack,
        addition: Option<&ItemStack>,
        name: &str,
    ) -> AnvilResult {
        let hover = horizon_client::stations::hover_name(&self.pack.names, item);
        anvil_result(
            &self.stations().enchantments,
            self.items(),
            Some(item),
            addition,
            Some(name),
            &hover,
            self.creative,
        )
    }

    /// The additions that do something to `item`: each held stack (one of each) the anvil gives a
    /// result for, `item`'s own stack only when there are two of it.
    fn legacy_anvil_additions(&self, item: &ItemStack) -> Vec<ItemStack> {
        let name = horizon_client::stations::hover_name(&self.pack.names, item);
        self.legacy_anvil_items()
            .into_iter()
            .filter(|a| {
                let held: u32 = self.inventory[..36]
                    .iter()
                    .flatten()
                    .filter(|s| s.same_item_same_components(a))
                    .map(|s| s.count)
                    .sum();
                (!a.same_item_same_components(item) || held >= 2)
                    && self
                        .legacy_anvil_result(item, Some(a), &name)
                        .result
                        .is_some()
            })
            .collect()
    }

    /// The selected item, keeping the staged addition and name only while they're its own.
    fn legacy_anvil_current(&mut self) -> Option<ItemStack> {
        let items = self.legacy_anvil_items();
        let selected = self
            .pad
            .legacy
            .anvil
            .selected
            .min(items.len().checked_sub(1)?);
        let item = items[selected].clone();
        let state = &self.pad.legacy.anvil;
        let same = state
            .named
            .as_ref()
            .is_some_and(|n| n.same_item_same_components(&item));
        if !same {
            let name = horizon_client::stations::hover_name(&self.pack.names, &item);
            let first = self.legacy_anvil_additions(&item).into_iter().next();
            let state = &mut self.pad.legacy.anvil;
            state.name = name;
            state.named = Some(item.clone());
            state.addition = first;
        }
        Some(item)
    }

    /// Left and right along the held items.
    pub(in crate::game) fn legacy_anvil_step(&mut self, button: Button) {
        let n = self.legacy_anvil_items().len();
        if n == 0 {
            return;
        }
        let state = &mut self.pad.legacy.anvil;
        let at = state.selected.min(n - 1);
        state.selected = match button {
            Button::DPadLeft => (at + n - 1) % n,
            Button::DPadRight => (at + 1) % n,
            _ => return,
        };
    }

    /// A press at the anvil: whether it was the anvil's.
    pub(in crate::game) fn legacy_anvil_button(&mut self, button: Button) -> bool {
        match (self.pad.legacy.focus, button) {
            (Area::Grid, Button::South) => self.legacy_anvil_cycle(true),
            (Area::Grid, Button::West) => self.legacy_anvil_cycle(false),
            (Area::Row | Area::Grid, Button::North) | (Area::Row, Button::South) => {
                self.legacy_anvil_go()
            }
            // X on the row, or View anywhere: type the name.
            (Area::Row, Button::West) | (_, Button::Select) => self.legacy_anvil_type(),
            _ => return false,
        }
        true
    }

    fn legacy_anvil_type(&mut self) {
        self.pad.legacy.typing = true;
        if self.pad.in_use {
            self.pad_open_keyboard();
        }
    }

    /// Typing into the name box (`legacy_type` at an anvil).
    pub(in crate::game) fn legacy_anvil_text(&mut self, text: Option<&str>, backspace: bool) {
        // AnvilMenu.MAX_NAME_LENGTH.
        const MOST: usize = 50;
        let name = &mut self.pad.legacy.anvil.name;
        if let Some(text) = text {
            for c in text.chars() {
                if name.chars().count() < MOST && !c.is_control() {
                    name.push(c);
                }
            }
        }
        if backspace {
            name.pop();
        }
    }

    /// The addition stepped through what does something to the item, and nothing.
    fn legacy_anvil_cycle(&mut self, next: bool) {
        let Some(item) = self.legacy_anvil_current() else {
            return;
        };
        // The second cell is the addition's; the first is the row's item.
        if self.pad.legacy.cursor != 1 {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        let mut choices: Vec<Option<ItemStack>> = vec![None];
        choices.extend(self.legacy_anvil_additions(&item).into_iter().map(Some));
        let now = &self.pad.legacy.anvil.addition;
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
        self.pad.legacy.anvil.addition = choices[to].clone();
    }

    /// A: the item and its addition into the anvil, the name, the result out.
    fn legacy_anvil_go(&mut self) {
        let Some(item) = self.legacy_anvil_current() else {
            return;
        };
        let state = &self.pad.legacy.anvil;
        let (addition, name) = (state.addition.clone(), state.name.clone());
        let made = self.legacy_anvil_result(&item, addition.as_ref(), &name);
        let affordable =
            self.creative || (made.cost < TOO_EXPENSIVE && self.experience.level >= made.cost);
        if made.result.is_none() || !affordable {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        let mut puts = vec![(item.clone(), INPUT)];
        puts.extend(addition.map(|a| (a, INPUT + 1)));
        let own = horizon_client::stations::hover_name(&self.pack.names, &item);
        let rename = (name != own).then_some(name);
        self.legacy_run_slots(puts, RESULT, vec![INPUT, INPUT + 1], rename);
    }

    /// The anvil's view: the held items along the row, the anvil itself below.
    pub(in crate::game) fn legacy_anvil_view(&mut self, view: &mut View) {
        let tr = |key: &str| horizon_client::lang::get(key);
        view.search = None;
        view.tab_page = None;
        view.tabs.clear();
        view.title = tr("block.minecraft.anvil");
        view.empty = tr("horizon.legacy_craft.empty");
        let typing = self.pad.legacy.typing;
        let Some(item) = self.legacy_anvil_current() else {
            view.anvil = Some((String::new(), false));
            return;
        };
        let items = self.legacy_anvil_items();
        view.row = items
            .iter()
            .map(|s| RecipeSlot {
                stack: s.clone(),
                craftable: true,
                fresh: false,
            })
            .collect();
        view.selected = self.pad.legacy.anvil.selected.min(items.len() - 1);
        let state = &self.pad.legacy.anvil;
        let (addition, name) = (state.addition.clone(), state.name.clone());
        let made = self.legacy_anvil_result(&item, addition.as_ref(), &name);
        view.anvil = Some((name, typing));
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
        let affordable =
            self.creative || (made.cost < TOO_EXPENSIVE && self.experience.level >= made.cost);
        view.result = made.result.clone().map(|stack| RecipeSlot {
            stack,
            craftable: affordable,
            fresh: false,
        });
        // Vanilla's words for the cost: green when it can be paid, red when it can't.
        view.note = Some(if self.carried.is_some() {
            (tr("horizon.legacy_craft.holding"), screen::MISSING)
        } else if self.legacy_build_running() {
            (tr("horizon.legacy_craft.crafting"), screen::HAVE)
        } else if made.result.is_none() {
            (tr("horizon.legacy_craft.not_yet"), screen::MISSING)
        } else if made.cost >= TOO_EXPENSIVE && !self.creative {
            (tr("container.repair.expensive"), screen::MISSING)
        } else {
            let cost =
                horizon_client::text::Text::tr("container.repair.cost", &[&made.cost]).plain();
            (
                cost,
                if affordable {
                    screen::HAVE
                } else {
                    screen::MISSING
                },
            )
        });
    }
}
