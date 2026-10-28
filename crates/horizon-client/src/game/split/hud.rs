//! What splitscreen draws over a player's view: its badge, questions answered with the pad and
//! notices; opening the Setup screen.
use super::*;

/// What splitscreen draws over a player's view (`Game::split_hud`): its corner badge (who, and a
/// moment where), a question its controller answers, and notices nobody has to answer.
#[derive(Default)]
pub(in crate::game) struct Hud {
    pub(super) badge: Option<String>,
    pub(super) place: Option<(String, Instant)>,
    pub(super) prompt: Option<Prompt>,
    pub(super) notice: Option<horizon_client::toasts::SystemToast>,
}

/// A question on a player's view, answered with its controller's buttons.
#[derive(Clone, PartialEq)]
pub(super) struct Prompt {
    pub(super) ask: Ask,
    pub(super) title: String,
    pub(super) choices: Vec<(Button, &'static str)>,
}

/// What a prompt asks: a controller may join (every view, a strip at the top: nobody stops
/// playing for it), this player leaving, P1's controller going (the view dimmed beneath).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Ask {
    Join,
    Leave,
    Controller,
    /// Its controller disconnected: any controller answers.
    Lost,
}

/// The question on a waiting player's view.
pub(super) fn lost_prompt(seat: usize) -> Prompt {
    Prompt {
        ask: Ask::Lost,
        title: format!("P{seat}: controller disconnected"),
        choices: vec![(Button::South, "Use another"), (Button::East, "Leave")],
    }
}

impl Game<'_> {
    /// The Splitscreen Setup screen, from the controller: Done goes back to playing.
    pub(super) fn open_split_setup(&mut self) {
        if self.screen == crate::game::Screen::Playing && self.title.is_none() {
            self.capture(false);
        }
        self.open_options_at(false, horizon_client::options_ui::Page::SplitSetup);
    }

    /// An error stops the event loop, as ever, but not in another splitscreen player's game:
    /// that player drops out (`Split::drop_out`), the others play on.
    pub(in crate::game) fn exit_on_error(&self, event_loop: &ActiveEventLoop) {
        if !self.options.split_guest {
            event_loop.exit();
        }
    }

    /// Splitscreen over this player's view, above the other toasts (`ToastManager`'s slots).
    pub(in crate::game) fn draw_split_hud(
        &mut self,
        batch: &mut HudBatch,
        slot: usize,
        hidden: bool,
    ) {
        let atlas = &self.gui.atlas;
        let (w, h) = (batch.width, batch.height);
        let width = |text: &str| HudBatch::text_width(atlas, text).ceil();
        // The badge, top left: who, and a moment where. F1 hides it with the rest of the HUD.
        if let (false, Some(badge)) = (hidden, &self.split_hud.badge) {
            let place = self.split_hud.place.as_ref();
            let text = match place.filter(|(_, until)| Instant::now() < *until) {
                Some((place, _)) => format!("{badge} · {place}"),
                None => badge.clone(),
            };
            batch.text(atlas, &text, 4.0, 4.0, rgba(255, 255, 255, 230), true);
        }
        // A question: centred over the view dimmed, or a strip at the top (a controller joining).
        if let Some(prompt) = &self.split_hud.prompt {
            let style = self.ui.options.pad_style;
            let dim = prompt.ask != Ask::Join;
            if dim {
                batch.fill(atlas, 0.0, 0.0, w, h, rgba(0, 0, 0, 120));
            }
            let glyphs: Vec<_> = prompt
                .choices
                .iter()
                .map(|&(b, what)| (b.label(style), b.color(style), what))
                .collect();
            let line: f32 = glyphs
                .iter()
                .map(|(g, _, what)| width(g) + 8.0 + width(what) + 12.0)
                .sum::<f32>()
                - 12.0;
            let pw = width(&prompt.title).max(line) + 16.0;
            let ph = 34.0;
            let (px, py) = (
                ((w - pw) / 2.0).floor(),
                if dim { ((h - ph) / 2.0).floor() } else { 18.0 },
            );
            batch.fill(atlas, px, py, pw, ph, rgba(0, 0, 0, 210));
            let edge = rgba(255, 255, 255, 90);
            for [x, y, fw, fh] in [
                [px, py, pw, 1.0],
                [px, py + ph - 1.0, pw, 1.0],
                [px, py, 1.0, ph],
                [px + pw - 1.0, py, 1.0, ph],
            ] {
                batch.fill(atlas, x, y, fw, fh, edge);
            }
            let tx = (px + (pw - width(&prompt.title)) / 2.0).floor();
            batch.text(
                atlas,
                &prompt.title,
                tx,
                py + 6.0,
                rgba(255, 255, 255, 255),
                true,
            );
            let mut x = (px + (pw - line) / 2.0).floor();
            let y = py + 20.0;
            for (glyph, [r, g, b], what) in glyphs {
                // The button as printed on the pad, in its colour, on a dark key.
                let gw = width(glyph) + 4.0;
                batch.fill(atlas, x - 1.0, y - 2.0, gw, 11.0, rgba(40, 40, 40, 255));
                batch.text(atlas, glyph, x + 1.0, y, rgba(r, g, b, 255), false);
                x += gw + 4.0;
                batch.text(atlas, what, x, y, rgba(220, 220, 220, 255), false);
                x += width(what) + 12.0;
            }
        }
        // Notices nobody answers: a toast.
        let multiplier = f64::from(self.settings.notification_display_time);
        let Some(notice) = &mut self.split_hud.notice else {
            return;
        };
        let (_, finished) = notice.update(multiplier);
        if finished {
            self.split_hud.notice = None;
            return;
        }
        notice.draw(&self.gui.atlas, batch, 32.0 * slot as f32);
    }
}

impl<'a> Split<'a> {
    /// The join question on every player's view in the world (a strip at the top), or gone.
    pub(super) fn prompt_join(&mut self, prompt: Option<Prompt>) {
        let views = std::iter::once(&mut self.host.split_hud).chain(
            self.guests
                .iter_mut()
                .filter(|g| g.name.is_some())
                .map(|g| &mut g.game.split_hud),
        );
        for hud in views {
            match &prompt {
                Some(p) => {
                    if hud.prompt.as_ref().is_none_or(|q| q.ask == Ask::Join) {
                        hud.prompt = Some(p.clone());
                    }
                }
                None if hud.prompt.as_ref().is_some_and(|q| q.ask == Ask::Join) => {
                    hud.prompt = None
                }
                None => {}
            }
        }
    }

    /// Player `i`'s badge says where it plays (or `note`) for a few seconds.
    pub(super) fn tag(&mut self, i: usize, note: Option<&str>) {
        // A player still choosing its profile has nowhere to tell yet.
        if i > 0 && self.guests[i - 1].name.is_none() {
            return;
        }
        let note = note.map_or_else(|| self.label(i), str::to_owned);
        let place = Some((note, Instant::now() + Duration::from_secs(3)));
        if let Some(game) = self.game_mut(i) {
            game.split_hud.place = place;
        }
    }
}
