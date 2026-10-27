//! Splitscreen's profile screens (Horizon's Splitscreen setting): a player joining on this
//! screen picks a profile, or makes one, before entering P1's world. A profile is a name, with
//! its own options and inventory. Built from vanilla's widgets, as Horizon's own pages are.
use super::{Button, Mode, Title, centered, edit_top};
use crate::{
    hud::{HudAtlas, HudBatch, rgba},
    lang,
};

const NEW: u8 = 70;
const CANCEL: u8 = 71;
const CREATE: u8 = 72;
const BACK: u8 = 73;
const PLAY_ON: u8 = 74;
/// Profile `i` is button `FIRST + i`.
const FIRST: u8 = 80;
/// ponytail: the first few profiles only; a scrolling list when someone has more.
const SHOWN: usize = 6;
pub(super) const NAME_FIELD: crate::clipboard::FieldId = ("splitscreen.profile-name", 0);
pub(super) const NAME_MAX: usize = 16;

/// A player name the server takes: 1 to 16 letters, digits or underscores.
pub fn valid_name(name: &str) -> bool {
    (1..=NAME_MAX).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Title {
    /// The profile list for a joining player; `taken` are playing now and can't be picked;
    /// `screens` are where it can play ("Window 1", open windows, "New window", the monitors).
    pub fn profile_picker(&mut self, names: Vec<String>, taken: Vec<String>, screens: Vec<String>) {
        self.mode = Mode::Profiles { names, taken };
        self.play_on = (screens, None);
    }

    /// What the joining player chose: a profile's name and where to play (`None`: where that
    /// profile played last), or `None` to leave. Taken once.
    pub fn take_profile_choice(&mut self) -> Option<Option<(String, Option<String>)>> {
        let choice = self.profile_choice.take()?;
        let (screens, picked) = &self.play_on;
        let screen = picked.and_then(|i| screens.get(i).cloned());
        Some(choice.map(|name| (name, screen)))
    }

    /// Where the player chose to play some other way (the controller's bumpers): the Play on
    /// button shows it, and choosing a profile keeps it.
    pub fn set_play_on(&mut self, label: &str) {
        let (screens, picked) = &mut self.play_on;
        *picked = screens.iter().position(|s| s == label);
    }

    /// The Play on button: "As last time" until the player picks.
    fn play_on_label(&self) -> String {
        let (screens, picked) = &self.play_on;
        let screen = picked
            .and_then(|i| screens.get(i))
            .map_or("As last time", String::as_str);
        format!("Play on: {screen}")
    }

    pub(super) fn profile_buttons(&self, w: f32, h: f32) -> Vec<Button> {
        let x = (w / 2.0).floor() - 100.0;
        let button = |y: f32, label: String, enabled: bool, id: u8| Button {
            x,
            y,
            w: 200.0,
            label,
            enabled,
            id,
            icon: None,
        };
        match &self.mode {
            Mode::Profiles { names, taken } => {
                let mut buttons: Vec<_> = names
                    .iter()
                    .take(SHOWN)
                    .enumerate()
                    .map(|(i, name)| {
                        button(
                            44.0 + i as f32 * 24.0,
                            name.clone(),
                            !taken.contains(name),
                            FIRST + i as u8,
                        )
                    })
                    .collect();
                let y = 52.0 + names.len().min(SHOWN) as f32 * 24.0;
                buttons.push(button(y, "New Profile...".into(), true, NEW));
                let screens = !self.play_on.0.is_empty();
                buttons.push(button(y + 24.0, self.play_on_label(), screens, PLAY_ON));
                buttons.push(button(
                    y + 56.0,
                    lang::get_or("gui.cancel", "Cancel"),
                    true,
                    CANCEL,
                ));
                buttons
            }
            Mode::NewProfile { name, taken, .. } => {
                let top = edit_top(h);
                let name = name.trim();
                vec![
                    button(
                        top + 64.0,
                        "Create".into(),
                        valid_name(name) && !taken.iter().any(|t| t == name),
                        CREATE,
                    ),
                    button(top + 89.0, lang::get_or("gui.back", "Back"), true, BACK),
                ]
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn draw_profiles(&self, atlas: &HudAtlas, batch: &mut HudBatch) {
        let (w, h) = (batch.width.ceil(), batch.height.ceil());
        crate::ui::menu_background(batch, atlas, 0.0, 0.0, w, h, false);
        match &self.mode {
            Mode::Profiles { .. } => {
                centered(atlas, batch, "Who's playing?", 20.0, u32::MAX);
                if !self.play_on.0.is_empty() {
                    let hint = "LB / RB on your controller: where you play";
                    centered(atlas, batch, hint, 32.0, rgba(160, 160, 160, 255));
                }
            }
            Mode::NewProfile { name, focused, .. } => {
                centered(atlas, batch, "New Profile", 20.0, u32::MAX);
                let top = edit_top(h);
                batch.text(
                    atlas,
                    "Name (letters, digits, _)",
                    (w / 2.0).floor() - 100.0,
                    top + 25.0,
                    rgba(160, 160, 160, 255),
                    true,
                );
                self.caret.borrow_mut().draw(
                    batch,
                    atlas,
                    (NAME_FIELD, name, NAME_MAX),
                    [(w / 2.0).floor() - 100.0, top + 39.0, 200.0, 20.0],
                    *focused,
                    self.started,
                );
            }
            _ => {}
        }
    }

    pub(super) fn profile_click(&mut self, x: f32, y: f32, (w, h): (f32, f32), hit: Option<u8>) {
        match (&mut self.mode, hit) {
            (Mode::Profiles { names, .. }, Some(id)) if id >= FIRST => {
                self.profile_choice = names.get(usize::from(id - FIRST)).cloned().map(Some);
            }
            (Mode::Profiles { names, taken }, Some(NEW)) => {
                self.mode = Mode::NewProfile {
                    name: String::new(),
                    focused: true,
                    names: std::mem::take(names),
                    taken: std::mem::take(taken),
                };
            }
            (Mode::Profiles { .. }, Some(CANCEL)) => self.profile_choice = Some(None),
            (Mode::Profiles { .. }, Some(PLAY_ON)) => {
                let (screens, picked) = &mut self.play_on;
                *picked = Some(picked.map_or(0, |i| i + 1) % screens.len().max(1));
            }
            (Mode::NewProfile { .. }, Some(CREATE)) => self.confirm_profile(),
            (Mode::NewProfile { .. }, Some(BACK)) => self.profile_back(),
            (Mode::NewProfile { focused, .. }, None) => {
                *focused = y >= edit_top(h) + 39.0
                    && y < edit_top(h) + 59.0
                    && (x - (w / 2.0).floor()).abs() < 100.0;
            }
            _ => {}
        }
    }

    /// Enter on the name: make the profile, if the name will do.
    pub(super) fn confirm_profile(&mut self) {
        if let Mode::NewProfile { name, taken, .. } = &self.mode {
            let name = name.trim();
            if valid_name(name) && !taken.iter().any(|t| t == name) {
                self.profile_choice = Some(Some(name.to_owned()));
            }
        }
    }

    /// Escape: from a new profile back to the list; from the list, leave.
    pub(super) fn profile_back(&mut self) {
        match &mut self.mode {
            Mode::NewProfile { names, taken, .. } => {
                self.mode = Mode::Profiles {
                    names: std::mem::take(names),
                    taken: std::mem::take(taken),
                };
            }
            _ => self.profile_choice = Some(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::valid_name;

    #[test]
    fn profile_names_are_player_names() {
        assert!(valid_name("Steve_2"));
        assert!(!valid_name(""));
        assert!(!valid_name("has space"));
        assert!(!valid_name("seventeen_chars_x"));
    }
}
