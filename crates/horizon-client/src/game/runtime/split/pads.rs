//! Controllers: which player each drives, joining with one, and one disconnecting (its player
//! waits for another). One reader for every player's game (`PadRouter`) while splitscreen
//! watches the controllers; otherwise P1 reads them itself, as without splitscreen.
use super::hud::tr;
use super::owners::{Effect, Identity, P1, Rules};
use super::*;
use crate::game::pad::{PadEvent, PadId};
use horizon_client::controller_notice::{Notice, NoticeId, Priority};
use horizon_client::gamepad::{MenuJob, PadState};

/// The `turn` index of the guest found at `seat`, if any: `turn(i)` selects guest `i - 1`.
fn guest_turn(seats: impl IntoIterator<Item = usize>, seat: usize) -> Option<usize> {
    seats
        .into_iter()
        .position(|s| s == seat)
        .map(|index| index + 1)
}

impl Hud {
    /// What the question on this view says, for `pad status`.
    pub(in crate::game) fn asks(&self) -> Option<&str> {
        self.prompt.as_ref().map(|p| p.title.as_str())
    }
}

impl Split<'_> {
    /// Give P1 its normal controller reader again and discard pending joins and handoffs.
    pub(super) fn stop_routing(&mut self) {
        if self.host.pad.feed.take().is_some() {
            self.host.pad.release_hardware();
            self.host.split_hud = Default::default();
            self.fresh.clear();
            let owners = std::mem::take(&mut self.owners);
            self.pads.hand_back(&mut self.host.pad, owners.pad_of(P1));
        }
    }

    /// The furthest a player's pad leant its sticks since the last frame: nothing for a pad just
    /// joined whose buttons are still being let go.
    fn pad_peaks(&self, pad: Option<PadId>) -> [[f32; 2]; 2] {
        pad.filter(|id| !self.fresh.contains(id))
            .map_or([[0.0; 2]; 2], |id| self.pads.peaks(id))
    }
    /// The pad's state for its player: nothing while the press that gave it is still held.
    fn pad_state(&mut self, pad: Option<PadId>) -> PadState {
        let Some(id) = pad else {
            return Default::default();
        };
        let state = self.pads.state(id);
        if self.fresh.contains(&id) {
            if pressing(&state) {
                return Default::default();
            }
            self.fresh.retain(|&p| p != id);
        }
        state
    }

    /// Controllers: P1's is the one it plays with as splitscreen turns on, else the first used in
    /// a menu; in a world a controller nobody holds is offered to join (A), or to drive P1 (Y)
    /// while P1 has none (`owners`). The pads whose players should join now.
    /// `solo`: no compositor, P1 alone (its controller's output is lent to it after this).
    pub(super) fn route_pads(&mut self, solo: bool) -> Vec<PadId> {
        if let Some(id) = self.pads.adopt(&mut self.host.pad) {
            self.owners.seat(id, P1);
        }
        let rules = Rules {
            now: Instant::now(),
            in_world: self.in_world(),
            p1_controllers: self.host.settings.controller.enabled,
            free_seats: MAX_PLAYERS.saturating_sub(self.players()),
            identity: Identity::HERE,
        };
        self.owners.expire(&rules);
        // Presses in another window aren't asking to join this one.
        let has_window = self.host.pad_has_window();
        for event in self.pads.events() {
            match event {
                PadEvent::Press(id, button) if has_window => {
                    let uuid = self.pads.uuid(id);
                    self.owners.press(id, uuid, button, &rules);
                }
                PadEvent::Press(..) => {}
                PadEvent::Gone(id) => {
                    self.fresh.retain(|&p| p != id);
                    let choosing = self
                        .guests
                        .iter()
                        .any(|g| self.owners.pad_of(g.seat) == Some(id) && g.name.is_none());
                    let uuid = self.pads.uuid(id);
                    self.owners.gone(id, uuid, choosing, &rules);
                }
                PadEvent::Back(id) => self.owners.back(id, self.pads.uuid(id), &rules),
            }
        }
        self.pad_effects();
        self.ask_to_join();
        // Each player's glyphs follow its own controller (its game never reads the pads itself).
        let owner = self.owners.pad_of(P1);
        if let Some(id) = owner {
            self.host.pad.style = self.pads.style(id);
        }
        self.host.pad.device = owner.and_then(|id| self.pads.device(id));
        for g in &mut self.guests {
            if let Some(id) = g.pad {
                g.game.pad.style = self.pads.style(id);
            }
            g.game.pad.device = g.pad.and_then(|id| self.pads.device(id));
        }
        // P1 alone keeps its controller's output (rumble, the DualSense's triggers): the router
        // lends it its reader for P1's turn (`lend_output`), so no second reader opens. Only
        // when nothing is lent does P1's own reader close: releasing a lent one each frame
        // would stop its rumble a frame after it starts.
        if !lends_output(solo, owner.is_some()) {
            self.host.pad.release_hardware();
        }
        // Only something on the control socket can ask.
        if self.host.control.is_some() {
            self.pads.report_status(&mut self.host.pad, owner);
        }
        let state = self.pad_state(owner);
        let peaks = self.pad_peaks(owner);
        self.host.pad.feed_from(owner, state, peaks);
        for i in 0..self.guests.len() {
            let pad = self.guests[i].pad;
            if self.guests[i].game.control.is_some() {
                self.pads.report_status(&mut self.guests[i].game.pad, pad);
            }
            let state = self.pad_state(pad);
            let peaks = self.pad_peaks(pad);
            self.guests[i].game.pad.feed_from(pad, state, peaks);
        }
        self.owners.joining()
    }

    /// P1's Give Up Pad: its controller stops driving P1 and is offered to join, for a friend
    /// to press A (P1 plays on with the keyboard and mouse).
    pub(super) fn give_up_pad(&mut self) {
        if self.owners.pad_of(P1).is_none() {
            return;
        }
        self.owners.give_up(Instant::now());
        self.host.pad.release_hardware();
        self.host.pad.feed = Some(PadState::default());
        self.ask_to_join();
    }

    /// A controller that accepted but couldn't join: nobody's, or P1's again (`end_offer`).
    pub(super) fn restore_given_up_pad(&mut self, id: PadId) {
        self.owners.end_offer(id);
        self.pad_effects();
    }

    /// What the controllers' changes of hands mean to the players' games, and the controller each
    /// plays with now.
    pub(super) fn pad_effects(&mut self) {
        for effect in self.owners.effects() {
            match effect {
                Effect::Fresh(id) => self.fresh.push(id),
                Effect::Found { seat, id } => self.pad_found(seat, id),
                Effect::Waits(seat) => self.pad_waits(seat),
                Effect::Leave(seat) => self.leave_waiting(seat),
                Effect::Cancel(seat) => {
                    // `pick_profiles` sees its page closed, and no pick from before that.
                    if let Some(g) = self.guests.iter_mut().find(|g| g.seat == seat) {
                        g.game.ui.options.split_profiles.cancel();
                        g.game.set_screen(crate::game::Screen::Playing);
                    }
                }
            }
        }
        for g in &mut self.guests {
            g.pad = self.owners.pad_of(g.seat);
        }
    }

    /// The controller of the player in `seat` disconnected (P1 keeps the keyboard and mouse).
    fn pad_waits(&mut self, seat: usize) {
        if seat == P1 {
            self.host.split_hud.notice.raise(Notice::new(
                NoticeId::Connection,
                Priority::Attention,
                tr("controller_disconnected", &[]),
                tr("controller_disconnected.hint", &[]),
            ));
        } else if let Some(g) = self.guests.iter_mut().find(|g| g.seat == seat)
            && let Some(name) = &g.name
        {
            g.game.split_hud.prompt = Some(lost_prompt(name));
        }
    }

    /// The player in `seat` plays with controller `id` (back, or another): its question goes.
    fn pad_found(&mut self, seat: usize, id: PadId) {
        self.fresh.push(id);
        let hud = if seat == P1 {
            &mut self.host.split_hud
        } else {
            let Some(g) = self.guests.iter_mut().find(|g| g.seat == seat) else {
                return;
            };
            &mut g.game.split_hud
        };
        if hud.prompt.as_ref().is_some_and(|p| p.ask == Ask::Lost) {
            hud.prompt = None;
        }
        hud.notice.raise(Notice::new(
            NoticeId::Connection,
            Priority::Info,
            tr("controller_connected", &[]),
            "",
        ));
    }

    /// The player in `seat`, waiting for a controller, leaves instead.
    fn leave_waiting(&mut self, seat: usize) {
        let Some(turn) = guest_turn(self.guests.iter().map(|g| g.seat), seat) else {
            return;
        };
        self.guests[turn - 1].game.split_hud.prompt = None;
        // The leave hooks see the guest's seat, as `switch` and `close_window` do: P2 leaving
        // must not run them as P1. `turn` takes the guest's index plus one, not its seat.
        self.turn(turn);
        let left = self.guests[turn - 1].game.leave_server(None);
        self.turn(0);
        if let Err(e) = left {
            log::warn!("Horizon: splitscreen: {e}");
        }
    }

    /// Every player sees the controllers waiting to join, whatever it is doing; what it says
    /// changes with them.
    fn ask_to_join(&mut self) {
        let p1_may_take = self.owners.pad_of(P1).is_none() && self.host.settings.controller.enabled;
        let ask: (Vec<_>, _) = (self.owners.offers().collect(), p1_may_take);
        if ask.0.is_empty() {
            self.owners.asked = ask;
            return self.prompt_join(None);
        }
        if ask == self.owners.asked {
            return;
        }
        let title = match ask.0.as_slice() {
            [id] => tr("join.one", &[&self.pads.name(*id)]),
            offers => tr("join.many", &[&offers.len()]),
        };
        let mut choices = vec![(MenuJob::Confirm, tr("join.yes", &[]))];
        if p1_may_take {
            choices.push((MenuJob::Extra, tr("join.as_p1", &[])));
        }
        choices.push((MenuJob::Back, tr("join.no", &[])));
        // Answered on the asking controller: the first one's family when several ask.
        let family = ask.0.first().map(|id| self.pads.style(*id));
        self.owners.asked = ask;
        self.prompt_join(Some(Prompt {
            ask: Ask::Join,
            title,
            choices,
            family,
        }));
    }
}

/// Whether P1 gets the split's reader for its turn (`lend_output`): solo, with a controller.
fn lends_output(solo: bool, p1_has_pad: bool) -> bool {
    solo && p1_has_pad
}

/// Still holding the press that gave the pad its player: a button, or a trigger off its rest.
/// Mapped triggers may also report a gilrs button press; unmapped axis triggers
/// only appear in `PadState::triggers` and do not create the join `Press` event.
fn pressing(state: &PadState) -> bool {
    state.buttons != 0 || state.triggers.iter().any(|&t| t > 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guest_turn_is_its_index_in_the_guest_list_not_its_seat() {
        assert_eq!(guest_turn([2, 3], 2), Some(1));
        assert_eq!(guest_turn([2, 3], 3), Some(2));
        assert_eq!(guest_turn([2, 3], 1), None);
    }

    #[test]
    fn a_lone_p1_keeps_its_lent_output_between_frames() {
        assert!(
            lends_output(true, true),
            "rumble and triggers survive the next frame"
        );
        assert!(
            !lends_output(true, false),
            "no controller: P1's own reader closes"
        );
        assert!(
            !lends_output(false, true),
            "others play: the router owns every pad"
        );
    }

    #[test]
    fn the_trigger_that_gave_a_pad_is_held_back_too() {
        assert!(!pressing(&PadState::default()));
        let trigger = PadState {
            triggers: [0.0, 0.8],
            ..Default::default()
        };
        assert!(pressing(&trigger), "a held trigger would attack or use");
        let button = PadState {
            buttons: 1,
            ..Default::default()
        };
        assert!(pressing(&button));
    }
}
