//! The controller crafting screen at a loom: the row is the patterns the loom offers (vanilla's
//! `selectablePatterns`, more with a pattern item staged), each shown on the staged banner in the
//! staged dye; the grid's place is the loom's banner, dye and pattern item, staged from the
//! inventory, A and X stepping a cell through what fits it with the focus there.
//!
//! The preview is the game's own (`banner::loom_result`). A weaves: banner, dye and pattern item
//! into the loom by vanilla's clicks, the pattern's button pressed as a click on it does
//! (`ServerboundContainerButtonClickPacket`), the result quick-moved out and the loom emptied.
use super::super::{
    Game,
    stations::{INPUT, Kind, RESULT},
};
use horizon_client::{
    banner,
    gamepad::{self, Button},
    items::ItemStack,
    legacy_craft_screen::{self as screen, Area, Cell, RecipeSlot, View},
};

/// The loom's state between frames.
#[derive(Default)]
pub(in crate::game) struct Loom {
    selected: usize,
    /// Banner, dye and pattern item staged (none: the first held of each, the pattern item
    /// none).
    staged: [Option<ItemStack>; 3],
}

impl Game<'_> {
    /// The open screen is a loom's.
    pub(in crate::game) fn legacy_loom(&self) -> bool {
        self.station.as_ref().is_some_and(|s| s.kind == Kind::Loom)
    }

    /// What each loom slot takes from the inventory, one of each kind.
    fn legacy_loom_candidates(&self, cell: usize) -> Vec<ItemStack> {
        let banners = &self.stations().banners;
        let items = self.items();
        let mut out: Vec<ItemStack> = Vec::new();
        for stack in self.inventory[..36].iter().flatten() {
            let fits = match cell {
                0 => banner::banner_color(&stack.item).is_some() && !banner::has_max_layers(stack),
                1 => banners.loom_dye(items, stack).is_some(),
                _ => banners.is_pattern_item(items, stack),
            };
            if fits && !out.iter().any(|s| s.same_item_same_components(stack)) {
                let mut one = stack.clone();
                one.count = 1;
                out.push(one);
            }
        }
        out
    }

    /// The staged banner, dye and pattern item, kept while held, else the first of each.
    fn legacy_loom_staged(&self) -> [Option<ItemStack>; 3] {
        let mut staged = self.pad.legacy.loom.staged.clone();
        for (cell, slot) in staged.iter_mut().enumerate() {
            let candidates = self.legacy_loom_candidates(cell);
            let held = slot
                .as_ref()
                .is_some_and(|s| candidates.iter().any(|c| c.same_item_same_components(s)));
            if !held {
                // The pattern item stays empty until picked.
                *slot = if cell == 2 {
                    None
                } else {
                    candidates.into_iter().next()
                };
            }
        }
        staged
    }

    /// The patterns the loom offers with this pattern item, in the order its buttons go.
    fn legacy_loom_patterns(&self, pattern_item: Option<&ItemStack>) -> Vec<std::sync::Arc<str>> {
        self.stations().banners.selectable(pattern_item)
    }

    /// A banner with a pattern woven on in a dye: what the loom would make.
    fn legacy_loom_makes(
        &self,
        staged: &[Option<ItemStack>; 3],
        pattern: &str,
    ) -> Option<ItemStack> {
        let (banner_stack, dye) = (staged[0].as_ref()?, staged[1].as_ref()?);
        let colour = self.stations().banners.loom_dye(self.items(), dye)?;
        Some(banner::loom_result(banner_stack, &colour, pattern))
    }

    pub(in crate::game) fn legacy_loom_step(&mut self, button: Button) {
        let staged = self.legacy_loom_staged();
        let n = self.legacy_loom_patterns(staged[2].as_ref()).len();
        if n == 0 {
            return;
        }
        let state = &mut self.pad.legacy.loom;
        let at = state.selected.min(n - 1);
        state.selected = match button {
            Button::DPadLeft => (at + n - 1) % n,
            Button::DPadRight => (at + 1) % n,
            _ => return,
        };
    }

    /// A press at the loom: whether it was the loom's.
    pub(in crate::game) fn legacy_loom_button(&mut self, button: Button) -> bool {
        match (self.pad.legacy.focus, button) {
            (Area::Grid, Button::South) => self.legacy_loom_cycle(true),
            (Area::Grid, Button::West) => self.legacy_loom_cycle(false),
            (Area::Row | Area::Grid, Button::North) | (Area::Row, Button::South) => {
                self.legacy_loom_go()
            }
            (_, Button::Select) | (Area::Row, Button::West) => {}
            _ => return false,
        }
        true
    }

    /// The focused cell stepped through what the inventory has for it (the pattern item's
    /// through nothing too).
    fn legacy_loom_cycle(&mut self, next: bool) {
        let cell = self.pad.legacy.cursor.min(2);
        let mut choices: Vec<Option<ItemStack>> = self
            .legacy_loom_candidates(cell)
            .into_iter()
            .map(Some)
            .collect();
        if cell == 2 {
            choices.insert(0, None);
        }
        if choices.is_empty() {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        let mut staged = self.legacy_loom_staged();
        let at = choices.iter().position(|c| match (c, &staged[cell]) {
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
        staged[cell] = choices[to].clone();
        self.pad.legacy.loom.staged = staged;
    }

    /// A: the banner, dye and pattern item in, the pattern's button, the result out.
    fn legacy_loom_go(&mut self) {
        let staged = self.legacy_loom_staged();
        let patterns = self.legacy_loom_patterns(staged[2].as_ref());
        let index = self
            .pad
            .legacy
            .loom
            .selected
            .min(patterns.len().saturating_sub(1));
        let makes = patterns
            .get(index)
            .and_then(|p| self.legacy_loom_makes(&staged, p));
        if makes.is_none() {
            self.pad_rumble(gamepad::Rumble::RadialNothing);
            return;
        }
        let puts = staged
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.clone().map(|s| (s, INPUT + i)))
            .collect();
        self.legacy_run_slots(
            puts,
            RESULT,
            (INPUT..INPUT + 3).collect(),
            None,
            Some(index),
        );
    }

    /// The loom's view: the patterns on the staged banner along the row, the loom below.
    pub(in crate::game) fn legacy_loom_view(&mut self, view: &mut View) {
        let tr = |key: &str| horizon_client::lang::get(key);
        view.smith = true;
        view.slot_hints = Some([
            "container/slot/banner",
            "container/slot/dye",
            "container/slot/banner_pattern",
        ]);
        view.search = None;
        view.tab_page = None;
        view.tabs.clear();
        view.title = tr("container.loom");
        view.empty = tr("horizon.legacy_craft.empty");
        let staged = self.legacy_loom_staged();
        self.pad.legacy.loom.staged = staged.clone();
        let patterns = self.legacy_loom_patterns(staged[2].as_ref());
        let fallback = ItemStack::new("minecraft:white_banner", 1);
        view.row = patterns
            .iter()
            .map(|p| {
                let made = self.legacy_loom_makes(&staged, p);
                RecipeSlot {
                    craftable: made.is_some(),
                    stack: made.unwrap_or_else(|| fallback.clone()),
                    fresh: false,
                }
            })
            .collect();
        let selected = self
            .pad
            .legacy
            .loom
            .selected
            .min(patterns.len().saturating_sub(1));
        view.selected = selected;
        view.cells = staged
            .iter()
            .map(|s| {
                s.clone().map(|stack| Cell {
                    stack,
                    missing: false,
                })
            })
            .collect();
        let made = patterns
            .get(selected)
            .and_then(|p| self.legacy_loom_makes(&staged, p));
        view.name = patterns
            .get(selected)
            .map(|p| {
                let colour = staged[1]
                    .as_ref()
                    .and_then(|d| self.stations().banners.loom_dye(self.items(), d))
                    .unwrap_or_else(|| "white".into());
                let layer = banner::BannerLayer::new(p, &colour);
                self.stations().banners.layer_key(&layer)
            })
            .map(|key| tr(&key))
            .unwrap_or_default();
        view.result = made.clone().map(|stack| RecipeSlot {
            stack,
            craftable: true,
            fresh: false,
        });
        view.note = Some(if self.carried.is_some() {
            (tr("horizon.legacy_craft.holding"), screen::MISSING)
        } else if self.legacy_build_running() {
            (tr("horizon.legacy_craft.crafting"), screen::HAVE)
        } else if staged[0].is_none() {
            (tr("horizon.legacy_craft.needs_banner"), screen::MISSING)
        } else if staged[1].is_none() {
            (tr("horizon.legacy_craft.needs_dye"), screen::MISSING)
        } else {
            (tr("horizon.legacy_craft.ready"), screen::HAVE)
        });
    }
}
