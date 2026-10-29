//! Splitscreen's profile page (Horizon's Splitscreen setting): a joining player picks a profile,
//! or makes one, before entering P1's world, and a player in it switches to another from its
//! pause menu (Splitscreen Settings), playing on until the new one can join. A profile is a name
//! (the launcher's offline names and Microsoft accounts), with its own options and inventory; the
//! launcher manages them. The game fills the list when the page opens (`filled`), takes the
//! choice, and says why a pick can't play (`refused`). Built from vanilla's widgets, as Horizon's
//! own pages are.
use crate::{
    graphics_style::STYLE,
    hud::{HudAtlas, HudBatch, rgba},
    lang,
    text::Text,
    ui,
};

/// ponytail: the first few profiles only; a scrolling list when someone has more.
const SHOWN: usize = 6;
pub const NAME_MAX: usize = 16;

/// A player name the server takes, and the launcher keeps as an offline name: 3 to 16
/// letters, digits or underscores.
pub fn valid_name(name: &str) -> bool {
    (3..=NAME_MAX).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `text` in lines no wider than `max` (by `width`), broken between words.
pub fn wrap(text: &str, max: f32, width: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let longer = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if width(&longer) > max && !line.is_empty() {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = longer;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[derive(Default)]
pub struct SplitProfiles {
    /// The game filled the page since it opened.
    pub filled: bool,
    /// The profiles; those playing now (none can be picked twice); the signed-in Microsoft
    /// accounts among them; and the one this player plays as (switching; none while joining).
    pub names: Vec<String>,
    pub taken: Vec<String>,
    pub accounts: Vec<String>,
    pub current: Option<String>,
    /// Why the last pick can't play (an online server, an expired sign-in), until the next.
    pub refused: Option<String>,
    /// A profile's name, or `None` for Cancel. Taken once.
    choice: Option<Option<String>>,
    /// New Profile...: the name being typed.
    new: Option<ui::EditBox>,
}

/// What a press did, for the options screen.
pub enum Click {
    Nothing,
    Pressed,
    /// Cancel: the page closes.
    Done,
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Profile(usize),
    New,
    Cancel,
    Create,
    Back,
}

/// A button: where, what it says, whether it can be pressed, what it does.
type Button = ([f32; 4], String, bool, Target);

/// The name field's top on the New Profile page.
fn field_top(h: f32) -> f32 {
    ((h.ceil() - 234.0) / 2.0).floor() + 39.0
}

impl SplitProfiles {
    /// Opened: empty until the game fills it.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The joining player's pick (`Some(name)`), or Cancel (`None`). Taken once.
    pub fn take_choice(&mut self) -> Option<Option<String>> {
        self.choice.take()
    }

    fn buttons(&self, w: f32, h: f32) -> Vec<Button> {
        let x = (w / 2.0).floor() - 100.0;
        let button = |y: f32, label: String, enabled: bool, target: Target| {
            ([x, y, 200.0, 20.0], label, enabled, target)
        };
        if let Some(field) = &self.new {
            let name = field.value.trim();
            // A new name: not one on the list already.
            let ok = valid_name(name) && !self.names.iter().any(|n| n.eq_ignore_ascii_case(name));
            let top = field_top(h);
            return vec![
                button(top + 25.0, "Create".into(), ok, Target::Create),
                button(
                    top + 50.0,
                    lang::get_or("gui.back", "Back"),
                    true,
                    Target::Back,
                ),
            ];
        }
        let mut buttons: Vec<_> = self
            .names
            .iter()
            .take(SHOWN)
            .enumerate()
            .map(|(i, name)| {
                // A Microsoft account signs in as itself.
                let mut label = name.clone();
                if self.accounts.contains(name) {
                    label.push_str(" (Microsoft)");
                }
                if self.current.as_ref() == Some(name) {
                    label.push_str(" - you");
                }
                let y = 44.0 + i as f32 * 24.0;
                button(y, label, !self.taken.contains(name), Target::Profile(i))
            })
            .collect();
        let y = 52.0 + self.names.len().min(SHOWN) as f32 * 24.0;
        buttons.push(button(y, "New Profile...".into(), true, Target::New));
        let cancel = lang::get_or("gui.cancel", "Cancel");
        buttons.push(button(y + 24.0, cancel, true, Target::Cancel));
        buttons
    }

    pub fn draw(
        &mut self,
        batch: &mut HudBatch,
        atlas: &HudAtlas,
        cursor: (f32, f32),
        in_world: bool,
    ) {
        let (w, h) = (batch.width.ceil(), batch.height.ceil());
        ui::menu_background(batch, atlas, 0.0, 0.0, w, h, in_world);
        let grey = rgba(160, 160, 160, 255);
        let line = |batch: &mut HudBatch, text: &str, y: f32, color: u32| {
            ui::centered_text(batch, atlas, &Text::literal(text), 0.0, y, w, color);
        };
        let buttons = self.buttons(w, h);
        if let Some(field) = &mut self.new {
            STYLE.title(batch, atlas, &Text::literal("New Profile"));
            let (x, top) = ((w / 2.0).floor() - 100.0, field_top(h));
            let label = "Name (3 to 16 letters, digits, _)";
            batch.text(atlas, label, x, top - 14.0, grey, true);
            (field.x, field.y, field.width, field.height) = (x, top, 200.0, 20.0);
            field.draw(batch, atlas, None);
        } else {
            let title = match self.current {
                Some(_) => "Switch profile",
                None => "Who's playing?",
            };
            STYLE.title(batch, atlas, &Text::literal(title));
            line(
                batch,
                "LB / RB on your controller: where you play",
                32.0,
                grey,
            );
            // Under Cancel: where names are renamed, skinned and forgotten.
            let y = 52.0 + self.names.len().min(SHOWN) as f32 * 24.0 + 54.0;
            line(batch, "Accounts are managed in the launcher", y, grey);
            // Why the last pick can't play, wrapped to the view (a quarter of a window is narrow).
            let width = |text: &str| HudBatch::text_width(atlas, text).ceil();
            let why = self.refused.as_deref().unwrap_or_default();
            for (k, text) in wrap(why, (w - 24.0).min(300.0), width).iter().enumerate() {
                line(
                    batch,
                    text,
                    y + 14.0 + 10.0 * k as f32,
                    rgba(255, 85, 85, 255),
                );
            }
        }
        for ([x, y, bw, bh], label, enabled, _) in &buttons {
            let hover = *enabled && ui::hovered(cursor, *x, *y, *bw, *bh);
            let text = Text::literal(label.as_str());
            ui::button(batch, atlas, *x, *y, *bw, *bh, &text, *enabled, hover);
        }
    }

    /// A press at `(x, y)` on a screen `w` x `h`.
    pub fn click(&mut self, atlas: &HudAtlas, (x, y): (f32, f32), (w, h): (f32, f32)) -> Click {
        let hit = self
            .buttons(w, h)
            .into_iter()
            .find(|([bx, by, bw, bh], _, enabled, _)| {
                *enabled && ui::hovered((x, y), *bx, *by, *bw, *bh)
            });
        let Some((_, _, _, target)) = hit else {
            // The name field takes the click on it, and the focus.
            if let Some(field) = &mut self.new {
                field.focused = ui::hovered((x, y), field.x, field.y, field.width, field.height);
                if field.focused {
                    field.click(atlas, x, false, false);
                }
            }
            return Click::Nothing;
        };
        match target {
            Target::Profile(i) => {
                self.refused = None;
                self.choice = self.names.get(i).cloned().map(Some);
            }
            Target::New => {
                let mut field = ui::EditBox::new(0.0, 0.0, 200.0, 20.0);
                field.max_length = NAME_MAX;
                self.new = Some(field);
            }
            Target::Create => self.create(),
            Target::Back => self.new = None,
            Target::Cancel => {
                self.choice = Some(None);
                return Click::Done;
            }
        }
        Click::Pressed
    }

    /// Create (or Enter on the name): the new profile plays, if the name will do.
    fn create(&mut self) {
        let can = self
            .buttons(0.0, 0.0)
            .iter()
            .any(|b| b.3 == Target::Create && b.2);
        if let (true, Some(field)) = (can, &self.new) {
            self.refused = None;
            self.choice = Some(Some(field.value.trim().to_owned()));
            self.new = None;
        }
    }

    pub fn type_text(&mut self, text: &str) {
        if let Some(field) = self.new.as_mut().filter(|f| f.focused) {
            field.insert_text(text);
        }
    }

    /// A key: true when the page took it. On New Profile Escape goes back to the list and Enter
    /// creates; on the list Escape is the options screen's (it closes the page).
    pub fn key(&mut self, key: winit::keyboard::KeyCode, text: Option<&str>) -> bool {
        use winit::keyboard::KeyCode as K;
        let Some(field) = &mut self.new else {
            return false;
        };
        match key {
            K::Escape => self.new = None,
            K::Enter | K::NumpadEnter => self.create(),
            _ if field.focused => {
                if !field.key(key, false, false)
                    && let Some(t) = text
                {
                    let typed: String = t.chars().filter(|c| !c.is_control()).collect();
                    field.insert_text(&typed);
                }
            }
            _ => {}
        }
        true
    }

    /// Ctrl+A/C/V/X in the name field; false when it isn't shown with the focus.
    pub fn shortcut(
        &mut self,
        shortcut: crate::clipboard::EditShortcut,
        clipboard: &mut dyn crate::clipboard::Clipboard,
    ) -> bool {
        match self.new.as_mut().filter(|f| f.focused) {
            Some(field) => {
                field.shortcut(shortcut, clipboard);
                true
            }
            None => false,
        }
    }

    /// For the control socket: the list, or the new name being typed.
    pub fn state(&self) -> serde_json::Value {
        match &self.new {
            Some(field) => serde_json::json!({
                "mode": "splitscreen-new-profile",
                "fields": { "name": field.value },
            }),
            None => serde_json::json!({
                "mode": "splitscreen-profiles",
                "profiles": self.names,
                "taken": self.taken,
                "refused": self.refused,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_wraps_to_the_view_between_words() {
        // Each character 6 wide: no line over 60.
        let width = |s: &str| 6.0 * s.len() as f32;
        let lines = wrap("This server needs each player's own account", 60.0, width);
        assert!(lines.iter().all(|l| width(l) <= 60.0));
        assert_eq!(
            lines.join(" "),
            "This server needs each player's own account"
        );
        // A word longer than the line keeps a line of its own.
        assert_eq!(
            wrap("a verylongwordindeed b", 30.0, width),
            ["a", "verylongwordindeed", "b"]
        );
        assert!(wrap("", 60.0, width).is_empty());
    }

    #[test]
    fn profile_names_are_player_names() {
        assert!(valid_name("Steve_2"));
        assert!(!valid_name(""));
        assert!(!valid_name("ab"));
        assert!(!valid_name("has space"));
        assert!(!valid_name("seventeen_chars_x"));
    }

    fn atlas() -> HudAtlas {
        HudAtlas::build(std::path::Path::new("/nonexistent/horizon-test-assets")).unwrap()
    }

    fn page() -> SplitProfiles {
        SplitProfiles {
            filled: true,
            names: vec!["Ann".into(), "Bea".into()],
            taken: vec!["Ann".into()],
            current: Some("Ann".into()),
            ..Default::default()
        }
    }

    fn press(page: &mut SplitProfiles, atlas: &HudAtlas, label: &str) -> Click {
        let ([x, y, _, _], ..) = page
            .buttons(427.0, 240.0)
            .into_iter()
            .find(|b| b.1 == label)
            .unwrap_or_else(|| panic!("no {label}"));
        page.click(atlas, (x + 1.0, y + 1.0), (427.0, 240.0))
    }

    #[test]
    fn the_one_playing_now_is_marked_and_cant_be_picked() {
        let p = page();
        let buttons = p.buttons(427.0, 240.0);
        let you = buttons.iter().find(|b| b.1 == "Ann - you").unwrap();
        assert!(!you.2);
        assert!(buttons.iter().any(|b| b.1 == "Bea" && b.2));
    }

    #[test]
    fn a_pick_clears_the_last_refusal_and_cancel_closes() {
        let atlas = atlas();
        let mut p = page();
        p.refused = Some("This server is in online mode".into());
        press(&mut p, &atlas, "Bea");
        assert_eq!(
            (p.take_choice(), p.refused.as_ref()),
            (Some(Some("Bea".into())), None)
        );
        assert!(p.take_choice().is_none());
        assert!(matches!(press(&mut p, &atlas, "Cancel"), Click::Done));
        assert_eq!(p.take_choice(), Some(None));
    }

    #[test]
    fn a_new_name_must_be_new_and_valid() {
        let atlas = atlas();
        let mut p = page();
        press(&mut p, &atlas, "New Profile...");
        let create = |p: &SplitProfiles| p.buttons(427.0, 240.0)[0].2;
        p.type_text("bea");
        assert!(!create(&p), "on the list already, whatever the case");
        p.key(winit::keyboard::KeyCode::Backspace, None);
        p.type_text("x");
        assert!(create(&p));
        p.key(winit::keyboard::KeyCode::Enter, None);
        assert_eq!(p.take_choice(), Some(Some("bex".into())));
        // Escape on New Profile goes back to the list, not out of the page.
        press(&mut p, &atlas, "New Profile...");
        assert!(p.key(winit::keyboard::KeyCode::Escape, None));
        assert!(!p.key(winit::keyboard::KeyCode::Escape, None));
    }
}
