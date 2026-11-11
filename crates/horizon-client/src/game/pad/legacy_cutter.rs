//! The controller crafting screen at a stonecutter: the materials the player holds that it cuts
//! are the tabs, what the selected one cuts into is the row, and the stonecutter's input and
//! result are in the grid's place (vanilla's clicks with the focus there).
//!
//! A cuts one, Y as many as the input makes: the material goes into the input by vanilla's quick
//! move (whatever else was there comes out first), the recipe is picked by the menu's own button
//! (`ServerboundContainerButtonClickPacket`, as a click on the stonecutter's list), and the
//! result is taken by a click and put down in the inventory, or quick-moved for all of them.
use super::super::{
    Game,
    stations::{INPUT, Kind, RESULT},
};
use horizon_client::{
    gamepad::{self, Button},
    items::ItemStack,
    legacy_craft_screen::{self as screen, Cell, RecipeSlot, View},
};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

/// The gap between two clicks on any server but the window's own: one a game tick.
const CLICK_GAP: Duration = Duration::from_millis(50);
/// How long the result may take to show.
const RESULT_WAIT: Duration = Duration::from_secs(2);

/// The stonecutter's state between frames.
#[derive(Default)]
pub(in crate::game) struct Cutter {
    /// The selected material's tab, and each material's selected cut (by item id).
    tab: usize,
    selected: HashMap<String, usize>,
    run: Option<Run>,
}

struct Run {
    steps: VecDeque<Step>,
    next_at: Instant,
}

#[derive(Clone, Debug)]
enum Step {
    /// The material into the input, whatever else is there out first.
    Load {
        material: ItemStack,
    },
    /// The menu's button for the cut that makes this item.
    Pick {
        result: String,
    },
    Result {
        until: Option<Instant>,
    },
    /// The result: one craft into the hand and down in the inventory, or all quick-moved.
    Take {
        all: bool,
    },
    /// What's in hand, down in the inventory: on a stack of it with room, else an empty slot.
    Stow,
}

impl Game<'_> {
    /// The open screen is a station's this screen shows (a stonecutter's, a smithing table's...).
    pub(in crate::game) fn legacy_station(&self) -> bool {
        self.legacy_cutter() || self.legacy_smith() || self.legacy_anvil()
    }

    /// The open screen is a stonecutter's.
    pub(in crate::game) fn legacy_cutter(&self) -> bool {
        self.station
            .as_ref()
            .is_some_and(|s| s.kind == Kind::Stonecutter)
    }

    /// The stonecutter's recipes a stack of `material` can be cut by, in the order its list shows.
    fn legacy_cuts(&self, material: &ItemStack) -> Vec<usize> {
        let recipes = &self.stations().recipes.stonecutting;
        (0..recipes.len())
            .filter(|&i| recipes[i].input.test(material))
            .collect()
    }

    /// The materials the player holds (and the one in the input) that it can cut, one of each.
    fn legacy_materials(&self) -> Vec<ItemStack> {
        let input = self.station.as_ref().and_then(|s| s.inputs[0].clone());
        let mut out: Vec<ItemStack> = Vec::new();
        for stack in input.iter().chain(self.inventory[..36].iter().flatten()) {
            if out.iter().any(|s| s.item == stack.item) || self.legacy_cuts(stack).is_empty() {
                continue;
            }
            out.push(ItemStack::new(&stack.item, 1));
        }
        out
    }

    /// How many of `item` the inventory and the input hold.
    fn legacy_cutter_held(&self, item: &str) -> u32 {
        let input = self.station.as_ref().and_then(|s| s.inputs[0].clone());
        input
            .iter()
            .chain(self.inventory[..36].iter().flatten())
            .filter(|s| &*s.item == item)
            .map(|s| s.count)
            .sum()
    }

    /// The selected material and cut (an index into the stonecutting recipes).
    fn legacy_cutter_current(&self) -> Option<(Vec<ItemStack>, usize, Vec<usize>, usize)> {
        let materials = self.legacy_materials();
        let state = &self.pad.legacy.cutter;
        let tab = state.tab.min(materials.len().checked_sub(1)?);
        let cuts = self.legacy_cuts(&materials[tab]);
        let selected = state
            .selected
            .get(&*materials[tab].item)
            .copied()
            .unwrap_or(0)
            .min(cuts.len().checked_sub(1)?);
        Some((materials, tab, cuts, selected))
    }

    /// LB and RB: the material.
    pub(in crate::game) fn legacy_cutter_turn(&mut self, next: bool) {
        let n = self.legacy_materials().len();
        if n == 0 {
            return;
        }
        let state = &mut self.pad.legacy.cutter;
        state.tab = if next {
            (state.tab + 1) % n
        } else {
            (state.tab + n - 1) % n
        };
    }

    /// Left and right along the cuts.
    pub(in crate::game) fn legacy_cutter_step(&mut self, button: Button) {
        let Some((materials, tab, cuts, selected)) = self.legacy_cutter_current() else {
            return;
        };
        let n = cuts.len();
        let to = match button {
            Button::DPadLeft => (selected + n - 1) % n,
            Button::DPadRight => (selected + 1) % n,
            _ => return,
        };
        let item = materials[tab].item.to_string();
        self.pad.legacy.cutter.selected.insert(item, to);
    }

    /// A on the row cuts one, Y as many as the material makes. Refused with something in hand,
    /// a cut already running, or where crafting isn't allowed.
    pub(in crate::game) fn legacy_cut(&mut self, all: bool) {
        let Some((materials, tab, cuts, selected)) = self.legacy_cutter_current() else {
            return;
        };
        let busy = self.pad.legacy.cutter.run.is_some();
        if busy || self.carried.is_some() || !self.direct_craft_allowed() {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        let result = self.stations().recipes.stonecutting[cuts[selected]]
            .result
            .item
            .to_string();
        let mut steps = VecDeque::from([
            Step::Load {
                material: materials[tab].clone(),
            },
            Step::Pick { result },
            Step::Result { until: None },
            Step::Take { all },
        ]);
        if !all {
            steps.push_back(Step::Stow);
        }
        self.pad.legacy.cutter.run = Some(Run {
            steps,
            next_at: Instant::now(),
        });
    }

    /// Whether a cut is running.
    pub(in crate::game) fn legacy_cutting(&self) -> bool {
        self.pad.legacy.cutter.run.is_some()
    }

    /// Once a frame: the running cut's next step, when its gap is up.
    pub(in crate::game) fn legacy_cutter_poll(&mut self) {
        let Some(mut run) = self.pad.legacy.cutter.run.take() else {
            return;
        };
        let now = Instant::now();
        if now < run.next_at || !self.legacy_cutter() {
            if self.legacy_cutter() {
                self.pad.legacy.cutter.run = Some(run);
            }
            return;
        }
        let gap = if self.integrated.is_some() {
            Duration::ZERO
        } else {
            CLICK_GAP
        };
        let Some(step) = run.steps.pop_front() else {
            return;
        };
        let input = self.station.as_ref().and_then(|s| s.inputs[0].clone());
        match step {
            Step::Load { material } => {
                let loaded = input.as_ref().is_some_and(|s| s.item == material.item);
                if !loaded {
                    if input.is_some() {
                        // Something else in the input: out first, then look again.
                        self.legacy_slot_click(INPUT, false, true);
                        run.steps.push_front(Step::Load { material });
                    } else if let Some(from) = (0..36).find(|&i| {
                        self.inventory[i]
                            .as_ref()
                            .is_some_and(|s| s.item == material.item)
                    }) {
                        self.legacy_slot_click(from, false, true);
                    } else {
                        run.steps.clear();
                    }
                    run.next_at = now + gap;
                }
            }
            Step::Pick { result } => {
                let recipes = &self.stations().recipes.stonecutting;
                let index = self.station.as_ref().and_then(|s| {
                    s.stone
                        .recipes
                        .iter()
                        .position(|&r| *recipes[r].result.item == *result)
                });
                match index {
                    Some(index) => {
                        self.legacy_cutter_button(index);
                        run.next_at = now + gap;
                    }
                    // The menu's list isn't there yet (the input just went in): a frame later.
                    None if input.is_some() => run.steps.push_front(Step::Pick { result }),
                    None => run.steps.clear(),
                }
            }
            Step::Result { until } => {
                let until = until.unwrap_or(now + RESULT_WAIT);
                if self.slot_view(RESULT).is_none() {
                    if now < until {
                        run.steps.push_front(Step::Result { until: Some(until) });
                    } else {
                        run.steps.clear();
                    }
                }
            }
            Step::Take { all } => {
                self.legacy_slot_click(RESULT, false, all);
                run.next_at = now + gap;
            }
            Step::Stow => {
                if let Some(held) = self.carried.clone() {
                    let to = (0..36)
                        .find(|&i| {
                            self.inventory[i].as_ref().is_some_and(|s| {
                                s.same_item_same_components(&held)
                                    && s.count + held.count <= self.stack_max_size(s)
                            })
                        })
                        .or_else(|| (0..36).find(|&i| self.inventory[i].is_none()));
                    if let Some(to) = to {
                        self.legacy_slot_click(to, false, false);
                    }
                }
            }
        }
        self.pad.legacy.cutter.run = Some(run);
    }

    /// The stonecutter list's button, as a click on its recipe does.
    fn legacy_cutter_button(&mut self, index: usize) {
        let mut taken = false;
        let flag = &mut taken;
        self.menu_session(|menu, env, _| {
            *flag = menu.button(env, index as i32);
            Some(*flag)
        });
        if taken {
            self.audio
                .play("minecraft:ui.stonecutter.select_recipe", None, 1.0, 1.0);
            self.remote_station_button(index as i32);
        }
    }

    /// The stonecutter's view: materials as tabs, their cuts along the row, its slots below.
    pub(in crate::game) fn legacy_cutter_view(&mut self, view: &mut View) {
        let tr = |key: &str| horizon_client::lang::get(key);
        view.cutter = true;
        view.search = None;
        view.tab_page = None;
        view.empty = tr("horizon.legacy_craft.nothing_to_cut");
        view.title = tr("block.minecraft.stonecutter");
        let input = self.station.as_ref().and_then(|s| s.inputs[0].clone());
        let result = self.slot_view(RESULT);
        view.cells = vec![
            input.clone().map(|stack| Cell {
                stack,
                missing: false,
            }),
            result.map(|stack| Cell {
                stack,
                missing: false,
            }),
        ];
        let Some((materials, tab, cuts, selected)) = self.legacy_cutter_current() else {
            view.tabs.clear();
            return;
        };
        let held = self.legacy_cutter_held(&materials[tab].item);
        let names = |s: &ItemStack| horizon_client::stations::hover_name(&self.pack.names, s);
        view.title = names(&materials[tab]);
        view.tabs = materials.clone();
        view.tab = tab;
        let shown = screen::tabs_shown(view.width, view.height, materials.len(), 1).max(1);
        view.first_tab = tab.saturating_sub(shown - 1);
        let recipes = &self.stations().recipes.stonecutting;
        view.row = cuts
            .iter()
            .map(|&r| RecipeSlot {
                stack: recipes[r].result.clone(),
                craftable: held > 0,
                fresh: false,
            })
            .collect();
        view.selected = selected;
        let made = recipes[cuts[selected]].result.clone();
        view.name = names(&made);
        // The empty input shows what goes in, faded; the empty result what comes out.
        if view.cells[0].is_none() {
            view.cells[0] = Some(Cell {
                stack: materials[tab].clone(),
                missing: true,
            });
        }
        if view.cells[1].is_none() {
            view.cells[1] = Some(Cell {
                stack: made.clone(),
                missing: true,
            });
        }
        let each = made.count.max(1);
        view.note = Some(if self.carried.is_some() {
            (tr("horizon.legacy_craft.holding"), screen::MISSING)
        } else if self.legacy_cutting() {
            (tr("horizon.legacy_craft.crafting"), screen::HAVE)
        } else {
            (
                horizon_client::lang::tr("horizon.legacy_craft.can_make", &[&(held * each)]),
                screen::HAVE,
            )
        });
    }
}
