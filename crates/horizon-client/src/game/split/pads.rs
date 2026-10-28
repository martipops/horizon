//! Controllers: which player each drives, joining with one, holding Select, and one
//! disconnecting (its player waits for another).
use super::*;

/// A player whose controller disconnected, waiting for one: its seat (1 = P1), and the
/// controller's id and kind (`PadRouter::uuid`) to know it back.
pub(super) struct Lost<Id> {
    pub(super) seat: usize,
    pub(super) id: Id,
    pub(super) uuid: [u8; 16],
}

/// What a button on a controller no player has does while players wait for one.
#[derive(Debug, PartialEq)]
pub(super) enum Claim {
    /// It goes to `lost[k]`.
    Give(usize),
    /// `lost[k]` leaves instead.
    Leave(usize),
    /// Nothing: the others wait (no offer to join meanwhile).
    Wait,
    /// Nobody but P1 waits, and P1 has the keyboard: as if nobody did.
    Pass,
}

/// The same kind of controller back (a new id on macOS) takes back its player with any button;
/// another takes over the player waiting longest with A, or lets it leave with B. P1 is only
/// given its own back (another joins as P1 through the offer to join).
pub(super) fn claim<Id>(lost: &[Lost<Id>], uuid: [u8; 16], button: Button) -> Claim {
    if uuid != [0; 16]
        && let Some(k) = lost.iter().position(|l| l.uuid == uuid)
    {
        return Claim::Give(k);
    }
    let Some(k) = lost.iter().position(|l| l.seat != 1) else {
        return Claim::Pass;
    };
    match button {
        Button::South => Claim::Give(k),
        Button::East => Claim::Leave(k),
        _ => Claim::Wait,
    }
}

/// Holding Select 2 s asks a question the controller then answers; meanwhile its buttons answer
/// that, not the game.
#[derive(Default)]
pub(super) struct HoldSelect {
    pub(super) since: Option<Instant>,
    pub(super) last: PadState,
    pub(super) asking: bool,
}

/// What holding Select did this frame.
pub(super) enum Hold {
    Nothing,
    /// Held 2 s: ask now.
    Ask,
    /// One of the answers, pressed while asked.
    Answer(Button),
    /// A bumper while asked (`true`: LB, back): the player's view moves, the question stays.
    Move(bool),
}

impl HoldSelect {
    fn update(&mut self, pad: PadState, answers: &[Button]) -> Hold {
        let last = std::mem::replace(&mut self.last, pad);
        if self.asking {
            for (bumper, back) in [(Button::LeftBumper, true), (Button::RightBumper, false)] {
                if pad.held(bumper) && !last.held(bumper) {
                    return Hold::Move(back);
                }
            }
            return match answers.iter().find(|&&b| pad.held(b) && !last.held(b)) {
                Some(&b) => {
                    self.asking = false;
                    Hold::Answer(b)
                }
                None => Hold::Nothing,
            };
        }
        if !pad.held(Button::Select) {
            self.since = None;
            return Hold::Nothing;
        }
        if self.since.get_or_insert_with(Instant::now).elapsed() < Duration::from_secs(2) {
            return Hold::Nothing;
        }
        self.since = None;
        self.asking = true;
        Hold::Ask
    }
}

impl<'a> Split<'a> {
    /// Player `i`'s pad as its game read it (the controller or `horizon-ctl pad`), with the
    /// controller itself while its buttons are kept from the game.
    pub(super) fn pad_now(&self, i: usize) -> PadState {
        let (state, pad) = match i {
            0 => (self.host.pad.state, self.p1_pad),
            _ => (self.guests[i - 1].game.pad.state, self.guests[i - 1].pad),
        };
        pad.map_or(state, |p| state.merge(&self.pads.state(p)))
    }

    /// Player `i` (not P1) holding Select 2 s is asked "Leave splitscreen?": A leaves, X
    /// switches profile (leaves, then chooses again where it was), Y opens the Setup screen, B
    /// stays; the bumpers move its view meanwhile, as when it joined.
    pub(super) fn leave_by_controller(&mut self, i: usize) {
        let pad = self.pad_now(i);
        let answers = [Button::South, Button::West, Button::North, Button::East];
        let hold = self.guests[i - 1].hold.update(pad, &answers);
        if let Hold::Move(back) = hold {
            return self.move_with_bumpers(i, back);
        }
        let g = &mut self.guests[i - 1];
        match hold {
            Hold::Ask => {
                g.game.split_hud.prompt = Some(Prompt {
                    ask: Ask::Leave,
                    title: "Leave splitscreen?".to_owned(),
                    choices: vec![
                        (Button::South, "Leave"),
                        (Button::West, "Switch profile"),
                        (Button::North, "Setup"),
                        (Button::RightBumper, "Move"),
                        (Button::East, "Stay"),
                    ],
                });
            }
            Hold::Answer(answer) => {
                g.game.split_hud.prompt = None;
                if answer == Button::North {
                    g.game.open_split_setup();
                }
                if matches!(answer, Button::East | Button::North) {
                    return;
                }
                if answer == Button::West {
                    let number = g.window;
                    let window = self.windows.iter().find(|w| w.number == number).cloned();
                    self.rejoin.push((
                        g.pad,
                        window.unwrap_or(layout::Window {
                            number: 1,
                            placement: Placement::Free(None),
                        }),
                    ));
                }
                if let Err(e) = g.game.leave_server(None) {
                    log::warn!("Horizon: splitscreen: {e}");
                }
            }
            Hold::Move(_) | Hold::Nothing => {}
        }
    }

    /// P1 holding Select 2 s, as the others do to leave: P1 hosts the world (it ends splitscreen
    /// from the pause menu), but its controller can go: A takes it from P1 (keyboard and mouse
    /// stay), and in a world a button on it then offers to join as a new player; Y opens the
    /// Setup screen; B keeps it; the bumpers move P1's view meanwhile.
    pub(super) fn p1_holds_select(&mut self) {
        let pad = self.pad_now(0);
        match self
            .p1_hold
            .update(pad, &[Button::South, Button::North, Button::East])
        {
            Hold::Move(back) => self.move_with_bumpers(0, back),
            Hold::Ask => {
                self.host.split_hud.prompt = Some(Prompt {
                    ask: Ask::Controller,
                    title: "P1's controller".to_owned(),
                    choices: vec![
                        (Button::South, "Remove it from P1"),
                        (Button::North, "Setup"),
                        (Button::RightBumper, "Move"),
                        (Button::East, "Keep it"),
                    ],
                });
            }
            Hold::Answer(answer) => {
                self.host.split_hud.prompt = None;
                if answer == Button::North {
                    self.host.open_split_setup();
                }
                if answer == Button::South
                    && let Some(id) = self.p1_pad.take()
                {
                    self.fresh.retain(|&p| p != id);
                    self.host.split_hud.notice = Some(horizon_client::toasts::SystemToast::new(
                        "Controller removed".to_owned(),
                        Some("Press a button on it to join as a new player".to_owned()),
                    ));
                }
            }
            Hold::Nothing => {}
        }
    }

    /// The pad's state for its player: nothing while the press that joined it is still held.
    pub(super) fn pad_state(&mut self, pad: Option<PadId>) -> PadState {
        let Some(id) = pad else {
            return Default::default();
        };
        let state = self.pads.state(id);
        if self.fresh.contains(&id) {
            if state.buttons != 0 {
                return Default::default();
            }
            self.fresh.retain(|&p| p != id);
        }
        state
    }

    /// Controllers: P1's is the first used in a menu; in a world another one is offered to join
    /// (A), or to drive P1 (Y) while P1 has none. The pad whose player should join now.
    pub(super) fn route_pads(&mut self) -> Vec<PadId> {
        let now = Instant::now();
        let mut joins = Vec::new();
        let mut presses = Vec::new();
        for event in self.pads.events() {
            match event {
                PadEvent::Press(id, button) => presses.push((id, button)),
                PadEvent::Gone(id) => self.pad_gone(id),
                PadEvent::Back(id) => {
                    if let Some(k) = self.lost.iter().position(|l| l.id == id) {
                        let seat = self.lost.remove(k).seat;
                        self.give_pad(seat, id);
                    }
                }
            }
        }
        self.forget_lost();
        for (id, button) in presses {
            if Some(id) == self.p1_pad || self.guests.iter().any(|g| g.pad == Some(id)) {
                continue;
            }
            // Pressed, then gone in the same frame.
            if !self.pads.connected(id) {
                continue;
            }
            if !self.in_world() {
                if self.p1_pad.is_none() {
                    self.p1_pad = Some(id);
                    self.fresh.push(id);
                }
                continue;
            }
            if self.claim_for_lost(id, button) {
                continue;
            }
            match self.offers.iter().position(|(o, _)| *o == id) {
                Some(k) => match button {
                    // Two pressing A together both join, in the order they pressed.
                    Button::South => {
                        self.offers.remove(k);
                        joins.push(id);
                    }
                    Button::North if self.p1_pad.is_none() => {
                        self.offers.remove(k);
                        self.p1_pad = Some(id);
                        self.fresh.push(id);
                    }
                    Button::East => {
                        self.offers.remove(k);
                    }
                    _ => {}
                },
                // An offer for each controller while seats are left.
                None if self.players() + joins.len() + self.offers.len() < MAX_PLAYERS => {
                    self.offers.push((id, now));
                    self.ask_to_join();
                }
                None => {}
            }
        }
        let in_world = self.in_world();
        let before = self.offers.len();
        self.offers
            .retain(|(_, since)| now - *since <= OFFER && in_world);
        if self.offers.is_empty() {
            self.prompt_join(None);
        } else if self.offers.len() != before || !joins.is_empty() {
            self.ask_to_join();
        }
        // Each player's glyphs follow its own controller (its game never reads the pads itself).
        if let Some(id) = self.p1_pad {
            self.host.pad.style = self.pads.style(id);
        }
        for g in &mut self.guests {
            if let Some(id) = g.pad {
                g.game.pad.style = self.pads.style(id);
            }
        }
        self.host.pad.release_hardware();
        let p1 = self.pad_state(self.p1_pad);
        // Asked about its controller, P1's buttons answer that, not the game.
        self.host.pad.feed = Some(if self.p1_hold.asking {
            PadState::default()
        } else {
            p1
        });
        for i in 0..self.guests.len() {
            let state = self.pad_state(self.guests[i].pad);
            // Asked whether to leave, its buttons answer that, not the game.
            let state = if self.guests[i].hold.asking {
                PadState::default()
            } else {
                state
            };
            self.guests[i].game.pad.feed = Some(state);
        }
        joins
    }

    /// A controller disconnected: its offer, rejoin and fresh press go; its player waits for one
    /// (P1 keeps the keyboard and mouse; a player picking a profile leaves).
    pub(super) fn pad_gone(&mut self, id: PadId) {
        self.fresh.retain(|&p| p != id);
        self.rejoin.retain(|(p, _)| *p != Some(id));
        let offered = self.offers.len();
        self.offers.retain(|(p, _)| *p != id);
        if self.offers.len() != offered && !self.offers.is_empty() {
            self.ask_to_join();
        }
        let uuid = self.pads.uuid(id);
        if self.p1_pad == Some(id) {
            self.p1_pad = None;
            self.p1_hold = HoldSelect::default();
            if self
                .host
                .split_hud
                .prompt
                .as_ref()
                .is_some_and(|p| p.ask == Ask::Controller)
            {
                self.host.split_hud.prompt = None;
            }
            if self.in_world() {
                self.lost.push(Lost { seat: 1, id, uuid });
                self.host.split_hud.notice = Some(horizon_client::toasts::SystemToast::new(
                    "Controller disconnected".to_owned(),
                    Some("Reconnect it, or press a button on another".to_owned()),
                ));
            }
            return;
        }
        let Some(g) = self.guests.iter_mut().find(|g| g.pad == Some(id)) else {
            return;
        };
        g.pad = None;
        g.hold = HoldSelect::default();
        if g.name.is_none() {
            // Still picking a profile: as if it pressed Cancel.
            if let Some(title) = g.game.title.as_mut() {
                title.escape();
            }
            return;
        }
        self.lost.push(Lost {
            seat: g.seat,
            id,
            uuid,
        });
        g.game.split_hud.prompt = Some(lost_prompt(g.seat));
    }

    /// Player in `seat` plays with controller `id` (back, or another): its question goes.
    pub(super) fn give_pad(&mut self, seat: usize, id: PadId) {
        self.fresh.push(id);
        let hud = if seat == 1 {
            self.p1_pad = Some(id);
            &mut self.host.split_hud
        } else {
            let Some(g) = self.guests.iter_mut().find(|g| g.seat == seat) else {
                return;
            };
            g.pad = Some(id);
            &mut g.game.split_hud
        };
        if hud.prompt.as_ref().is_some_and(|p| p.ask == Ask::Lost) {
            hud.prompt = None;
        }
        hud.notice = Some(horizon_client::toasts::SystemToast::new(
            "Controller connected".to_owned(),
            None,
        ));
    }

    /// A button on a controller nobody has while players wait for one: true when it answered
    /// them (`claim`).
    pub(super) fn claim_for_lost(&mut self, id: PadId, button: Button) -> bool {
        match claim(&self.lost, self.pads.uuid(id), button) {
            Claim::Give(k) => {
                let seat = self.lost.remove(k).seat;
                self.give_pad(seat, id);
            }
            Claim::Leave(k) => {
                let seat = self.lost.remove(k).seat;
                if let Some(g) = self.guests.iter_mut().find(|g| g.seat == seat) {
                    g.game.split_hud.prompt = None;
                    if let Err(e) = g.game.leave_server(None) {
                        log::warn!("Horizon: splitscreen: {e}");
                    }
                }
            }
            Claim::Wait => {}
            Claim::Pass => return false,
        }
        true
    }

    /// Nobody waits for a player who left or has a controller again (another way), nor for P1
    /// out of a world.
    pub(super) fn forget_lost(&mut self) {
        let p1 = self.in_world() && self.p1_pad.is_none();
        let guests = &self.guests;
        self.lost.retain(|l| match l.seat {
            1 => p1,
            seat => guests.iter().any(|g| g.seat == seat && g.pad.is_none()),
        });
    }

    /// Every player sees the controllers waiting to join, whatever it is doing.
    pub(super) fn ask_to_join(&mut self) {
        let title = match self.offers.as_slice() {
            [(id, _)] => format!("{} wants to join", self.pads.name(*id)),
            offers => format!("{} controllers want to join", offers.len()),
        };
        let mut choices = vec![(Button::South, "Join")];
        if self.p1_pad.is_none() {
            choices.push((Button::North, "Play as P1"));
        }
        choices.push((Button::East, "No"));
        self.prompt_join(Some(Prompt {
            ask: Ask::Join,
            title,
            choices,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAD: [u8; 16] = [7; 16];
    const OTHER: [u8; 16] = [9; 16];

    fn lost(seat: usize, uuid: [u8; 16]) -> Lost<usize> {
        Lost {
            seat,
            id: seat,
            uuid,
        }
    }

    #[test]
    fn the_same_controller_back_takes_its_player_with_any_button() {
        let waiting = [lost(2, OTHER), lost(3, PAD)];
        assert_eq!(claim(&waiting, PAD, Button::DPadUp), Claim::Give(1));
        // P1's own comes back too.
        assert_eq!(claim(&[lost(1, PAD)], PAD, Button::West), Claim::Give(0));
    }

    #[test]
    fn another_controller_takes_over_or_lets_go_the_longest_waiting() {
        let waiting = [lost(1, OTHER), lost(3, OTHER), lost(2, OTHER)];
        assert_eq!(claim(&waiting, PAD, Button::South), Claim::Give(1));
        assert_eq!(claim(&waiting, PAD, Button::East), Claim::Leave(1));
        // Any other button: the others wait, nobody is offered to join meanwhile.
        assert_eq!(claim(&waiting, PAD, Button::North), Claim::Wait);
    }

    #[test]
    fn only_p1_waiting_leaves_the_offer_to_join() {
        assert_eq!(claim(&[lost(1, OTHER)], PAD, Button::South), Claim::Pass);
        assert_eq!(claim::<usize>(&[], PAD, Button::South), Claim::Pass);
    }

    #[test]
    fn a_controller_the_system_does_not_describe_is_not_known_back() {
        let unknown = [0; 16];
        assert_eq!(
            claim(&[lost(2, unknown)], unknown, Button::West),
            Claim::Wait
        );
    }
}
