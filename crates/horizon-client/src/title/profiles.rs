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
const DELETE: u8 = 74;
const KEEP: u8 = 75;
/// The edit view's Delete: a button with red words (`Title`'s button drawing).
pub(super) const DELETE_ASK: u8 = 76;
/// Profile `i` is button `FIRST + i`, its Edit `EDIT + i`.
const FIRST: u8 = 80;
const EDIT: u8 = 90;
/// ponytail: the first few profiles only; a scrolling list when someone has more.
const SHOWN: usize = 6;
pub(super) const NAME_FIELD: crate::clipboard::FieldId = ("splitscreen.profile-name", 0);
pub(super) const NAME_MAX: usize = 16;

/// A player name the server takes, and the launcher keeps as an offline name: 3 to 16
/// letters, digits or underscores.
pub fn valid_name(name: &str) -> bool {
    (3..=NAME_MAX).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Title {
    /// The profile list for a joining player; `taken` are playing now and can't be picked.
    pub fn profile_picker(&mut self, names: Vec<String>, taken: Vec<String>) {
        self.mode = Mode::Profiles {
            names,
            taken,
            deleting: None,
        };
    }

    /// A profile deleted on the picker, for the game to remove its folder. Taken once.
    pub fn take_deleted_profile(&mut self) -> Option<String> {
        self.profile_deleted.take()
    }

    /// A profile renamed on the picker (before, after), for the game to move its folder. Taken
    /// once.
    pub fn take_renamed_profile(&mut self) -> Option<(String, String)> {
        self.profile_renamed.take()
    }

    /// What the joining player chose: a profile's name, or `None` to leave. Taken once.
    pub fn take_profile_choice(&mut self) -> Option<Option<String>> {
        self.profile_choice.take()
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
            Mode::Profiles {
                deleting: Some(_), ..
            } => vec![
                button(h / 2.0, "Delete".into(), true, DELETE),
                button(
                    h / 2.0 + 24.0,
                    lang::get_or("gui.cancel", "Cancel"),
                    true,
                    KEEP,
                ),
            ],
            Mode::Profiles { names, taken, .. } => {
                let mut buttons = Vec::new();
                for (i, name) in names.iter().take(SHOWN).enumerate() {
                    // The name, and its Edit beside it; not while it plays.
                    let (y, free) = (44.0 + i as f32 * 24.0, !taken.contains(name));
                    let pick = button(y, name.clone(), free, FIRST + i as u8);
                    let edit = Button {
                        x: x + 160.0,
                        w: 40.0,
                        ..button(y, "Edit".into(), free, EDIT + i as u8)
                    };
                    buttons.push(Button { w: 156.0, ..pick });
                    buttons.push(edit);
                }
                let y = 52.0 + names.len().min(SHOWN) as f32 * 24.0;
                buttons.push(button(y, "New Profile...".into(), true, NEW));
                buttons.push(button(
                    y + 24.0,
                    lang::get_or("gui.cancel", "Cancel"),
                    true,
                    CANCEL,
                ));
                buttons
            }
            Mode::NewProfile {
                name,
                taken,
                names,
                editing,
                ..
            } => {
                let top = edit_top(h);
                let name = name.trim();
                let label = if editing.is_some() { "Save" } else { "Create" };
                let ok = valid_name(name)
                    && !taken.iter().any(|t| t.eq_ignore_ascii_case(name))
                    // Renamed: not to another profile's name.
                    && !editing.as_ref().is_some_and(|before| {
                        names
                            .iter()
                            .any(|n| n != before && n.eq_ignore_ascii_case(name))
                    });
                let mut buttons = vec![
                    button(top + 64.0, label.into(), ok, CREATE),
                    button(top + 89.0, lang::get_or("gui.back", "Back"), true, BACK),
                ];
                if editing.is_some() {
                    buttons.push(button(top + 120.0, "Delete".into(), true, DELETE_ASK));
                }
                buttons
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn draw_profiles(&self, atlas: &HudAtlas, batch: &mut HudBatch) {
        let (w, h) = (batch.width.ceil(), batch.height.ceil());
        crate::ui::menu_background(batch, atlas, 0.0, 0.0, w, h, false);
        match &self.mode {
            Mode::Profiles {
                deleting: Some(name),
                ..
            } => {
                let top = (h / 2.0 - 40.0).floor();
                centered(atlas, batch, &format!("Delete {name}?"), top, u32::MAX);
                let grey = rgba(160, 160, 160, 255);
                let lines = [
                    "Its settings on this computer are deleted.",
                    "Worlds keep what it had there.",
                ];
                for (k, line) in lines.into_iter().enumerate() {
                    centered(atlas, batch, line, top + 14.0 + 11.0 * k as f32, grey);
                }
            }
            Mode::Profiles { .. } => {
                centered(atlas, batch, "Who's playing?", 20.0, u32::MAX);
                let hint = "LB / RB on your controller: where you play";
                centered(atlas, batch, hint, 32.0, rgba(160, 160, 160, 255));
            }
            Mode::NewProfile {
                name,
                focused,
                editing,
                ..
            } => {
                let title = if editing.is_some() {
                    "Edit Profile"
                } else {
                    "New Profile"
                };
                centered(atlas, batch, title, 20.0, u32::MAX);
                let top = edit_top(h);
                if editing.is_some() {
                    // Worlds know an offline player by its name.
                    let note = "Renamed, it starts afresh in worlds it played";
                    centered(atlas, batch, note, top + 6.0, rgba(160, 160, 160, 255));
                }
                batch.text(
                    atlas,
                    "Name (3 to 16 letters, digits, _)",
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
            (
                Mode::Profiles {
                    names, deleting, ..
                },
                Some(DELETE),
            ) => {
                if let Some(name) = deleting.take() {
                    names.retain(|n| *n != name);
                    self.profile_deleted = Some(name);
                }
            }
            (Mode::Profiles { deleting, .. }, Some(KEEP)) => *deleting = None,
            (
                Mode::Profiles {
                    deleting: Some(_), ..
                },
                _,
            ) => {}
            (Mode::Profiles { names, taken, .. }, Some(id)) if id >= EDIT => {
                if let Some(before) = names.get(usize::from(id - EDIT)).cloned() {
                    self.mode = Mode::NewProfile {
                        name: before.clone(),
                        focused: true,
                        names: std::mem::take(names),
                        taken: std::mem::take(taken),
                        editing: Some(before),
                    };
                }
            }
            (Mode::Profiles { names, .. }, Some(id)) if id >= FIRST => {
                self.profile_choice = names.get(usize::from(id - FIRST)).cloned().map(Some);
            }
            (Mode::Profiles { names, taken, .. }, Some(NEW)) => {
                self.mode = Mode::NewProfile {
                    name: String::new(),
                    focused: true,
                    names: std::mem::take(names),
                    taken: std::mem::take(taken),
                    editing: None,
                };
            }
            (Mode::Profiles { .. }, Some(CANCEL)) => self.profile_choice = Some(None),
            (Mode::NewProfile { .. }, Some(CREATE)) => self.confirm_profile(),
            (Mode::NewProfile { .. }, Some(BACK)) => self.profile_back(),
            // Delete, while editing: "Delete it?", asked first.
            (
                Mode::NewProfile {
                    names,
                    taken,
                    editing: Some(before),
                    ..
                },
                Some(DELETE_ASK),
            ) => {
                self.mode = Mode::Profiles {
                    names: std::mem::take(names),
                    taken: std::mem::take(taken),
                    deleting: Some(before.clone()),
                };
            }
            (Mode::NewProfile { focused, .. }, None) => {
                *focused = y >= edit_top(h) + 39.0
                    && y < edit_top(h) + 59.0
                    && (x - (w / 2.0).floor()).abs() < 100.0;
            }
            _ => {}
        }
    }

    /// Create or Save (or Enter on the name), if the name will do: a new profile plays; an
    /// edited one is renamed, back on the list.
    pub(super) fn confirm_profile(&mut self) {
        let (w, h) = (0.0, 0.0);
        if !self
            .profile_buttons(w, h)
            .iter()
            .any(|b| b.id == CREATE && b.enabled)
        {
            return;
        }
        let Mode::NewProfile {
            name,
            names,
            taken,
            editing,
            ..
        } = &mut self.mode
        else {
            return;
        };
        let name = name.trim().to_owned();
        match editing.take() {
            None => self.profile_choice = Some(Some(name)),
            Some(before) => {
                let mut names = std::mem::take(names);
                if name != before {
                    for n in names.iter_mut().filter(|n| **n == before) {
                        n.clone_from(&name);
                    }
                    names.sort_by_key(|n| n.to_lowercase());
                    self.profile_renamed = Some((before, name));
                }
                self.mode = Mode::Profiles {
                    names,
                    taken: std::mem::take(taken),
                    deleting: None,
                };
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
                    deleting: None,
                };
            }
            // Escape on "Delete it?": keep it.
            Mode::Profiles {
                deleting: deleting @ Some(_),
                ..
            } => *deleting = None,
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
        assert!(!valid_name("ab"));
        assert!(!valid_name("has space"));
        assert!(!valid_name("seventeen_chars_x"));
    }
}
