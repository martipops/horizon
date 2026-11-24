//! Which player each controller drives, offers to join included: plain state, moved by what the
//! controllers do (`pads` reads them and carries out the `Effect`s). A controller is in one place
//! (`Hold`), so it can't join twice, drive two players, or keep an offer once it has a player.
use horizon_client::gamepad::Button;
use std::time::{Duration, Instant};

/// How long a controller's offer to join stays up.
pub(super) const OFFER: Duration = Duration::from_secs(10);

/// P1's seat; the others' are 2 and up.
pub(super) const P1: usize = 1;

/// Where a connected controller is. One nobody has and nobody asks about isn't listed.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Hold {
    /// Offered to join, since when.
    Offered(Instant),
    /// Its player accepted and joins at the end of this frame: no press changes that.
    Joining,
    /// It drives the player in this seat.
    Seat(usize),
}

/// A player whose controller disconnected, waiting for one: its controller's id (none once
/// another player has the controller that id names now) and kind (`PadRouter::uuid`).
struct Lost<Id> {
    seat: usize,
    id: Option<Id>,
    uuid: [u8; 16],
}

/// The controller P1 gave away (Give Up Pad). Until its new player is in the world, a hand-off
/// that ends gives that same controller back, never another player's.
struct HandOff<Id> {
    id: Id,
    away: Option<Away>,
}

/// It disconnected with no player driving it yet: back, it is offered again while its offer runs,
/// else it is P1's.
struct Away {
    uuid: [u8; 16],
    offer: Option<Instant>,
}

/// How a controller is known back after it disconnects.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Identity {
    /// Its id stays its own (Windows).
    Id,
    /// Its id goes to the next controller of its model that connects (Linux: gilrs reuses the
    /// slot and can't tell two of one model apart), so it means nothing while two of that model
    /// are away.
    Slot,
    /// It gets a new id, so only its model says (macOS), while one of that model is away.
    Model,
}

impl Identity {
    pub(super) const HERE: Self = if cfg!(target_os = "macos") {
        Self::Model
    } else if cfg!(target_os = "linux") {
        Self::Slot
    } else {
        Self::Id
    };
}

/// What the game says this frame.
pub(super) struct Rules {
    pub(super) now: Instant,
    /// P1 is in a world others can join.
    pub(super) in_world: bool,
    /// P1's Gamepad setting.
    pub(super) p1_controllers: bool,
    /// Players who may still join.
    pub(super) free_seats: usize,
    pub(super) identity: Identity,
}

/// What a change of hands means to the games.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Effect<Id> {
    /// P1 has it now: the press that did it isn't P1's.
    Fresh(Id),
    /// The player waiting in `seat` has a controller again.
    Found { seat: usize, id: Id },
    /// The player in `seat` waits for a controller.
    Waits(usize),
    /// The player waiting in `seat` leaves instead.
    Leave(usize),
    /// The player in `seat` lost it choosing a profile: as if it pressed Cancel.
    Cancel(usize),
}

/// What a button on a controller no player has does while players wait for one.
#[derive(Debug, PartialEq)]
enum Claim {
    /// It goes to the player in this seat.
    Give(usize),
    /// That player leaves instead.
    Leave(usize),
    /// Nothing: the others wait (no offer to join meanwhile).
    Wait,
    /// Nobody but P1 waits, and P1 has the keyboard: as if nobody did.
    Pass,
}

pub(super) struct Owners<Id> {
    held: Vec<(Id, Hold)>,
    /// Longest waiting first.
    lost: Vec<Lost<Id>>,
    given_up: Option<HandOff<Id>>,
    effects: Vec<Effect<Id>>,
    /// The join question on the views: the controllers it is about, and whether P1 may take one.
    pub(super) asked: (Vec<Id>, bool),
}

impl<Id> Default for Owners<Id> {
    fn default() -> Self {
        Self {
            held: Vec::new(),
            lost: Vec::new(),
            given_up: None,
            effects: Vec::new(),
            asked: (Vec::new(), false),
        }
    }
}

impl<Id: Copy + Eq> Owners<Id> {
    fn hold(&self, id: Id) -> Option<Hold> {
        self.held.iter().find(|(p, _)| *p == id).map(|(_, h)| *h)
    }

    fn put(&mut self, id: Id, hold: Hold) {
        self.free(id);
        self.held.push((id, hold));
    }

    fn free(&mut self, id: Id) {
        self.held.retain(|(p, _)| *p != id);
    }

    /// The controller driving the player in `seat`.
    pub(super) fn pad_of(&self, seat: usize) -> Option<Id> {
        let (id, _) = self.held.iter().find(|(_, h)| *h == Hold::Seat(seat))?;
        Some(*id)
    }

    /// The player in `seat` waits for a controller.
    pub(super) fn waits(&self, seat: usize) -> bool {
        self.lost.iter().any(|l| l.seat == seat)
    }

    /// The controllers offered to join, in the order they asked.
    pub(super) fn offers(&self) -> impl Iterator<Item = Id> {
        let offered = |hold: &Hold| matches!(hold, Hold::Offered(_));
        self.held
            .iter()
            .filter(move |(_, h)| offered(h))
            .map(|(id, _)| *id)
    }

    /// The controllers whose players accepted, in the order they did.
    pub(super) fn joining(&self) -> Vec<Id> {
        let joining = self.held.iter().filter(|(_, h)| *h == Hold::Joining);
        joining.map(|(id, _)| *id).collect()
    }

    pub(super) fn effects(&mut self) -> Vec<Effect<Id>> {
        std::mem::take(&mut self.effects)
    }

    /// Whether another controller may be offered: each offer keeps a seat for its answer.
    fn may_offer(&self, rules: &Rules) -> bool {
        let seated = |hold: &Hold| matches!(hold, Hold::Seat(_));
        let asking = self.held.iter().filter(|(_, h)| !seated(h)).count();
        rules.in_world && asking < rules.free_seats
    }

    /// `id` drives the player in `seat`, and only it. Any offer or wait this settles ends.
    pub(super) fn seat(&mut self, id: Id, seat: usize) {
        self.held.retain(|(_, h)| *h != Hold::Seat(seat));
        self.put(id, Hold::Seat(seat));
        // Another controller takes over: the one P1 gave up may still come back to P1.
        let replaced = self.lost.iter().find(|l| l.seat == seat);
        if let Some((lost, hand_off)) = replaced.zip(self.given_up.as_mut())
            && lost.id == Some(hand_off.id)
            && hand_off.id != id
        {
            hand_off.away = Some(Away {
                uuid: lost.uuid,
                offer: None,
            });
        }
        self.lost.retain(|l| l.seat != seat);
        // Its id stands for another player's controller now.
        for lost in self.lost.iter_mut().filter(|l| l.id == Some(id)) {
            lost.id = None;
        }
        if let Some(hand_off) = self.given_up.as_mut().filter(|h| h.id == id) {
            hand_off.away = None;
            if seat == P1 {
                self.given_up = None;
            }
        }
    }

    fn seat_p1(&mut self, id: Id) {
        self.seat(id, P1);
        self.effects.push(Effect::Fresh(id));
    }

    /// `id`'s offer, or its player before it got into the world, is over: nobody's, or P1's again
    /// when P1 gave it up and has no other.
    pub(super) fn end_offer(&mut self, id: Id) {
        self.free(id);
        if self.given_up.take_if(|h| h.id == id).is_some() && self.pad_of(P1).is_none() {
            self.seat_p1(id);
        }
    }

    /// P1's Give Up Pad: its controller is offered to join.
    pub(super) fn give_up(&mut self, now: Instant) {
        if let Some(id) = self.pad_of(P1) {
            self.put(id, Hold::Offered(now));
            self.given_up = Some(HandOff { id, away: None });
        }
    }

    /// The player in `seat` is in the world: a controller P1 gave up is its for good.
    pub(super) fn entered(&mut self, seat: usize) {
        let pad = self.pad_of(seat);
        let lost = self.lost.iter().find(|l| l.seat == seat).and_then(|l| l.id);
        self.given_up
            .take_if(|h| Some(h.id) == pad || Some(h.id) == lost);
    }

    /// The player in `seat` left.
    pub(super) fn left(&mut self, seat: usize) {
        if let Some(id) = self.pad_of(seat) {
            self.end_offer(id);
        }
        let Some(k) = self.lost.iter().position(|l| l.seat == seat) else {
            return;
        };
        let lost = self.lost.remove(k);
        if let Some(hand_off) = &mut self.given_up
            && lost.id == Some(hand_off.id)
        {
            hand_off.away = Some(Away {
                uuid: lost.uuid,
                offer: None,
            });
        }
    }

    /// Before the frame's events: offers that ran out end, so nothing answers them, and P1 waits
    /// for no controller out of a world.
    pub(super) fn expire(&mut self, rules: &Rules) {
        let over = |since: Instant| !rules.in_world || rules.now - since > OFFER;
        let ended = |(id, hold): &(Id, Hold)| match hold {
            Hold::Offered(since) if over(*since) => Some(*id),
            _ => None,
        };
        for id in self.held.iter().filter_map(ended).collect::<Vec<_>>() {
            self.end_offer(id);
        }
        if let Some(away) = self.given_up.as_mut().and_then(|h| h.away.as_mut()) {
            away.offer.take_if(|since| over(*since));
        }
        let p1_waits = rules.in_world && self.pad_of(P1).is_none();
        self.lost.retain(|l| l.seat != P1 || p1_waits);
    }

    /// A button went down.
    pub(super) fn press(&mut self, id: Id, uuid: [u8; 16], button: Button, rules: &Rules) {
        match self.hold(id) {
            // P1 turned controllers off: its own is dead to it, so it may join as any other.
            Some(Hold::Seat(P1)) if !rules.p1_controllers && self.may_offer(rules) => {
                return self.give_up(rules.now);
            }
            Some(Hold::Seat(_) | Hold::Joining) => return,
            Some(Hold::Offered(_)) | None => {}
        }
        // The players waiting for a controller come first: the same kind back takes back its
        // player, another takes over the one waiting longest (`claim`).
        if rules.in_world {
            match self.claim(uuid, button, rules.identity) {
                Claim::Give(seat) => {
                    self.seat(id, seat);
                    return self.effects.push(Effect::Found { seat, id });
                }
                Claim::Leave(seat) => {
                    self.left(seat);
                    return self.effects.push(Effect::Leave(seat));
                }
                Claim::Wait => return,
                Claim::Pass => {}
            }
        }
        if self.hold(id).is_none() && self.hand_off_back(id, uuid, rules.identity) {
            return self.resume(id, rules);
        }
        let p1_has_pad = self.pad_of(P1).is_some();
        if p1_takes(
            rules.in_world,
            self.waits(P1),
            rules.p1_controllers,
            p1_has_pad,
        ) {
            return self.seat_p1(id);
        }
        match (self.hold(id), button) {
            (Some(_), Button::South) => self.put(id, Hold::Joining),
            (Some(_), Button::North) if !p1_has_pad && rules.p1_controllers => self.seat_p1(id),
            // No: a pad P1 gave up goes back to P1.
            (Some(_), Button::East) => self.end_offer(id),
            (Some(_), _) => {}
            (None, _) if self.may_offer(rules) => self.put(id, Hold::Offered(rules.now)),
            (None, _) => {}
        }
    }

    /// A controller disconnected. `choosing`: its player is still choosing a profile.
    pub(super) fn gone(&mut self, id: Id, uuid: [u8; 16], choosing: bool, rules: &Rules) {
        let Some(hold) = self.hold(id) else {
            return;
        };
        self.free(id);
        let hand_off = self.given_up.as_mut().filter(|h| h.id == id);
        let seat = match hold {
            Hold::Seat(seat) => seat,
            Hold::Offered(_) | Hold::Joining => {
                if let Some(hand_off) = hand_off {
                    let offer = match hold {
                        Hold::Offered(since) => Some(since),
                        _ => None,
                    };
                    hand_off.away = Some(Away { uuid, offer });
                }
                return;
            }
        };
        if seat != P1 && choosing {
            if let Some(hand_off) = hand_off {
                hand_off.away = Some(Away { uuid, offer: None });
            }
            return self.effects.push(Effect::Cancel(seat));
        }
        if seat != P1 || rules.in_world {
            self.lost.push(Lost {
                seat,
                id: Some(id),
                uuid,
            });
            self.effects.push(Effect::Waits(seat));
        }
    }

    /// A controller connected. One whose id says nothing (`Identity`) waits for a press.
    pub(super) fn back(&mut self, id: Id, uuid: [u8; 16], rules: &Rules) {
        if self.hold(id).is_some()
            || (rules.identity == Identity::Slot && self.away_of_model(uuid) > 1)
        {
            return;
        }
        if let Some(seat) = self.lost.iter().find(|l| l.id == Some(id)).map(|l| l.seat) {
            self.seat(id, seat);
            self.effects.push(Effect::Found { seat, id });
        } else if self.hand_off_back(id, uuid, Identity::Id) {
            self.resume(id, rules);
        }
    }

    /// The controller P1 gave up is back: offered again while its offer ran, else P1's.
    fn resume(&mut self, id: Id, rules: &Rules) {
        let Some(hand_off) = &mut self.given_up else {
            return;
        };
        hand_off.id = id;
        let offer = hand_off.away.take().and_then(|away| away.offer);
        if offer.is_some() && self.may_offer(rules) {
            self.put(id, Hold::Offered(rules.now));
        } else {
            self.end_offer(id);
        }
    }

    /// Whether `id`, connected, is the controller P1 gave up that went away.
    fn hand_off_back(&self, id: Id, uuid: [u8; 16], identity: Identity) -> bool {
        self.given_up.as_ref().is_some_and(|h| {
            h.away.is_some() && (h.id == id || (identity == Identity::Model && self.one_away(uuid)))
        })
    }

    /// How many controllers of this kind are away.
    fn away_of_model(&self, uuid: [u8; 16]) -> usize {
        let given_up = self.given_up.as_ref().and_then(|h| h.away.as_ref());
        let lost = self.lost.iter().filter(|l| l.uuid == uuid).count();
        lost + usize::from(given_up.is_some_and(|a| a.uuid == uuid))
    }

    /// This kind of controller is known, and one of it is away.
    fn one_away(&self, uuid: [u8; 16]) -> bool {
        uuid != [0; 16] && self.away_of_model(uuid) == 1
    }

    /// The same kind of controller back takes back its player with any button; another takes over
    /// the player waiting longest with A, or lets it leave with B. P1 is only given its own back
    /// (another joins as P1 through the offer to join).
    ///
    /// `uuid` names the model, not the device (gilrs' SDL-style GUID), so it stands for "the same
    /// controller" only where a reconnect gets a new id (`Identity::Model`; elsewhere the id
    /// survives and `back` takes it), and only when one waiting player has that model: otherwise
    /// another DualSense would silently take a DualSense player's seat.
    fn claim(&self, uuid: [u8; 16], button: Button, identity: Identity) -> Claim {
        if identity == Identity::Model
            && self.one_away(uuid)
            && let Some(lost) = self.lost.iter().find(|l| l.uuid == uuid)
        {
            return Claim::Give(lost.seat);
        }
        let Some(lost) = self.lost.iter().find(|l| l.seat != P1) else {
            return Claim::Pass;
        };
        match button {
            Button::South => Claim::Give(lost.seat),
            Button::East => Claim::Leave(lost.seat),
            _ => Claim::Wait,
        }
    }
}

/// Whether a controller nobody holds becomes P1's on its first press: in a menu, where it drives
/// the menus, or in a world while P1's own is gone and P1's view asks for another ("Use
/// another"). Elsewhere in a world it is offered to join, so a controller pressed while P1 plays
/// with the keyboard and mouse starts the join flow instead of taking P1's controls; Y in that
/// offer gives the pad to P1 when it is free. P1 with controllers off plays on the keyboard and
/// mouse: a pad given to it is dead either way.
fn p1_takes(in_world: bool, p1_waits: bool, controllers: bool, p1_has_pad: bool) -> bool {
    (!in_world || p1_waits) && controllers && !p1_has_pad
}

#[cfg(test)]
mod tests {
    use super::*;
    use Button::{East, North, South, West};

    const A: usize = 10;
    const B: usize = 11;
    const C: usize = 12;
    const NONE: [usize; 0] = [];

    fn rules() -> Rules {
        Rules {
            now: Instant::now(),
            in_world: true,
            p1_controllers: true,
            free_seats: 3,
            identity: Identity::Id,
        }
    }

    fn model(id: usize) -> [u8; 16] {
        [id as u8; 16]
    }

    /// P1 in its world without a controller, and players with one in seats 2 and up.
    fn guests(pads: &[usize]) -> Owners<usize> {
        let mut owners = Owners::default();
        for (k, pad) in pads.iter().enumerate() {
            owners.seat(*pad, k + 2);
        }
        owners
    }

    fn p1_with(pad: usize) -> Owners<usize> {
        let mut owners = Owners::default();
        owners.seat(pad, P1);
        owners
    }

    fn press(owners: &mut Owners<usize>, id: usize, button: Button, rules: &Rules) {
        owners.press(id, model(id), button, rules);
    }

    fn gone(owners: &mut Owners<usize>, id: usize, rules: &Rules) {
        owners.gone(id, model(id), false, rules);
    }

    fn later(rules: Rules) -> Rules {
        Rules {
            now: rules.now + OFFER + Duration::from_secs(1),
            ..rules
        }
    }

    fn offers(owners: &Owners<usize>) -> Vec<usize> {
        owners.offers().collect()
    }

    #[test]
    fn a_controller_pressed_in_a_world_is_offered_to_join_not_taken_by_p1() {
        let rules = rules();
        let mut owners = Owners::default();
        press(&mut owners, A, South, &rules);
        assert_eq!(offers(&owners), [A], "the first press only asks");
        assert_eq!(owners.pad_of(P1), None);
        press(&mut owners, A, North, &rules);
        assert_eq!(owners.pad_of(P1), Some(A), "Y: it is P1's");
        assert_eq!(offers(&owners), NONE);
    }

    #[test]
    fn a_menus_first_controller_is_p1s_unless_p1_has_controllers_off() {
        let menu = Rules {
            in_world: false,
            ..rules()
        };
        let mut owners = Owners::default();
        press(&mut owners, A, West, &menu);
        press(&mut owners, B, West, &menu);
        assert_eq!(owners.pad_of(P1), Some(A));
        assert_eq!(owners.effects(), [Effect::Fresh(A)]);

        let off = Rules {
            p1_controllers: false,
            ..menu
        };
        let mut owners = Owners::default();
        press(&mut owners, A, South, &off);
        assert_eq!(owners.pad_of(P1), None);
    }

    #[test]
    fn p1_waiting_takes_the_next_controller_pressed() {
        let rules = rules();
        let mut owners = p1_with(A);
        gone(&mut owners, A, &rules);
        assert_eq!(owners.effects(), [Effect::Waits(P1)]);
        press(&mut owners, B, West, &rules);
        assert_eq!(owners.pad_of(P1), Some(B));
        assert!(!owners.waits(P1));
    }

    #[test]
    fn one_controller_accepting_twice_in_a_poll_joins_once() {
        let rules = rules();
        let mut owners = Owners::default();
        for button in [West, South, West, South] {
            press(&mut owners, A, button, &rules);
        }
        assert_eq!(owners.joining(), [A]);
        assert_eq!(offers(&owners), NONE);
    }

    #[test]
    fn a_controller_that_accepted_cannot_also_become_p1s() {
        let rules = rules();
        let mut owners = Owners::default();
        for button in [West, South, West, North] {
            press(&mut owners, A, button, &rules);
        }
        assert_eq!(owners.joining(), [A]);
        assert_eq!(owners.pad_of(P1), None);
    }

    #[test]
    fn two_controllers_accepting_in_a_poll_each_join() {
        let rules = rules();
        let mut owners = Owners::default();
        for (pad, button) in [(A, West), (B, West), (B, South), (A, South)] {
            press(&mut owners, pad, button, &rules);
        }
        assert_eq!(owners.joining(), [B, A], "in the order they accepted");
        owners.seat(B, 2);
        owners.seat(A, 3);
        assert_eq!((owners.pad_of(2), owners.pad_of(3)), (Some(B), Some(A)));
        assert_eq!(owners.joining(), NONE);
    }

    #[test]
    fn an_offer_keeps_a_seat_for_its_answer() {
        let rules = Rules {
            free_seats: 1,
            ..rules()
        };
        let mut owners = Owners::default();
        press(&mut owners, A, West, &rules);
        press(&mut owners, B, West, &rules);
        assert_eq!(offers(&owners), [A], "one seat, one question");
        press(&mut owners, A, South, &rules);
        press(&mut owners, B, West, &rules);
        assert_eq!(offers(&owners), NONE, "the seat is spoken for");
    }

    #[test]
    fn one_gone_before_its_player_joined_joins_nobody() {
        let rules = rules();
        let mut owners = Owners::default();
        press(&mut owners, A, West, &rules);
        press(&mut owners, A, South, &rules);
        gone(&mut owners, A, &rules);
        assert_eq!(owners.joining(), NONE);
        assert_eq!(owners.effects(), []);
    }

    #[test]
    fn an_offer_that_ran_out_cannot_be_accepted() {
        let rules = rules();
        let mut owners = Owners::default();
        press(&mut owners, A, West, &rules);
        let late = later(rules);
        owners.expire(&late);
        press(&mut owners, A, South, &late);
        assert_eq!(owners.joining(), NONE, "the press asks again");
        assert_eq!(offers(&owners), [A]);
    }

    #[test]
    fn a_given_up_controller_comes_back_when_its_offer_ends() {
        let end: [fn(&mut Owners<usize>); 4] = [
            // Cancelled on its view, or no seat to join.
            |owners| owners.end_offer(A),
            |owners| press(owners, A, East, &rules()),
            |owners| owners.expire(&later(rules())),
            // Accepted, then its player left before it got into the world.
            |owners| {
                press(owners, A, South, &rules());
                owners.seat(A, 2);
                owners.left(2);
            },
        ];
        for (k, end) in end.iter().enumerate() {
            let mut owners = p1_with(A);
            owners.give_up(rules().now);
            assert_eq!((owners.pad_of(P1), offers(&owners)), (None, vec![A]));
            end(&mut owners);
            assert_eq!(owners.pad_of(P1), Some(A), "end {k}");
            assert_eq!(offers(&owners), NONE, "end {k}");
            assert_eq!(owners.effects(), [Effect::Fresh(A)], "end {k}");
        }
    }

    #[test]
    fn a_given_up_controller_stays_with_a_player_in_the_world() {
        let mut owners = p1_with(A);
        owners.give_up(rules().now);
        owners.seat(A, 2);
        owners.entered(2);
        owners.left(2);
        assert_eq!(owners.pad_of(P1), None);
    }

    #[test]
    fn p1_with_another_controller_never_gets_the_given_up_one_too() {
        let rules = rules();
        let mut owners = p1_with(A);
        owners.give_up(rules.now);
        press(&mut owners, B, West, &rules);
        press(&mut owners, B, North, &rules);
        press(&mut owners, A, East, &rules);
        assert_eq!(owners.pad_of(P1), Some(B));
        assert_eq!(owners.hold(A), None);
    }

    /// P1 gives A up, P2's controller disconnects and A takes P2's seat.
    #[test]
    fn an_offer_ends_when_its_controller_takes_a_waiting_players_seat() {
        let rules = rules();
        let mut owners = guests(&[B]);
        owners.seat(A, P1);
        owners.give_up(rules.now);
        gone(&mut owners, B, &rules);
        press(&mut owners, A, South, &rules);
        assert_eq!(owners.pad_of(2), Some(A));
        assert_eq!(offers(&owners), NONE, "nobody is asked about it any more");
        owners.entered(2);
        owners.expire(&later(rules));
        assert_eq!(owners.pad_of(2), Some(A), "and it stays P2's");
        assert_eq!(owners.pad_of(P1), None);
    }

    #[test]
    fn a_hand_off_goes_on_when_its_controller_comes_back() {
        let rules = rules();
        let mut owners = p1_with(A);
        owners.give_up(rules.now);
        gone(&mut owners, A, &rules);
        owners.expire(&rules);
        assert_eq!(offers(&owners), NONE, "nobody can answer it meanwhile");
        owners.back(A, model(A), &rules);
        assert_eq!(offers(&owners), [A]);
        press(&mut owners, A, East, &rules);
        assert_eq!(owners.pad_of(P1), Some(A), "it still goes back to P1");
    }

    #[test]
    fn a_hand_off_that_ran_out_while_away_gives_p1_its_controller_back() {
        let rules = rules();
        let mut owners = p1_with(A);
        owners.give_up(rules.now);
        gone(&mut owners, A, &rules);
        let late = later(rules);
        owners.expire(&late);
        owners.back(A, model(A), &late);
        assert_eq!((owners.pad_of(P1), offers(&owners)), (Some(A), vec![]));
    }

    #[test]
    fn a_hand_off_lost_on_the_profile_page_gives_p1_its_controller_back() {
        let rules = rules();
        let mut owners = p1_with(A);
        owners.give_up(rules.now);
        owners.seat(A, 2);
        owners.gone(A, model(A), true, &rules);
        assert_eq!(owners.effects(), [Effect::Cancel(2)]);
        owners.left(2);
        owners.back(A, model(A), &rules);
        assert_eq!(owners.pad_of(P1), Some(A));
    }

    #[test]
    fn a_hand_off_away_waits_for_its_own_controller_only() {
        let rules = rules();
        let mut owners = p1_with(A);
        owners.give_up(rules.now);
        gone(&mut owners, A, &rules);
        press(&mut owners, B, West, &rules);
        assert_eq!(owners.pad_of(P1), None, "another one asks to join as ever");
        assert_eq!(offers(&owners), [B]);
        press(&mut owners, B, North, &rules);
        owners.expire(&later(rules));
        owners.back(A, model(A), &self::rules());
        assert_eq!(owners.pad_of(P1), Some(B), "P1 took another meanwhile");
        assert_eq!(owners.hold(A), None);
    }

    /// The player joining with it loses it, and takes another before getting into the world.
    #[test]
    fn a_replaced_hand_off_gives_p1_its_controller_back() {
        let rules = rules();
        let mut owners = p1_with(A);
        owners.give_up(rules.now);
        owners.seat(A, 2);
        gone(&mut owners, A, &rules);
        press(&mut owners, B, South, &rules);
        assert_eq!(owners.pad_of(2), Some(B));
        owners.back(A, model(A), &rules);
        assert_eq!((owners.pad_of(P1), owners.pad_of(2)), (Some(A), Some(B)));
    }

    #[test]
    fn p1s_controller_may_join_while_p1_has_controllers_off() {
        let off = Rules {
            p1_controllers: false,
            ..rules()
        };
        let mut owners = p1_with(A);
        press(&mut owners, A, South, &off);
        assert_eq!(offers(&owners), [A]);
        press(&mut owners, A, East, &off);
        assert_eq!(owners.pad_of(P1), Some(A), "refused, it is kept for P1");
    }

    #[test]
    fn p1s_controller_back_in_a_menu_is_nobodys() {
        let rules = rules();
        let mut owners = p1_with(A);
        gone(&mut owners, A, &rules);
        let menu = Rules {
            in_world: false,
            ..rules
        };
        owners.expire(&menu);
        owners.back(A, model(A), &menu);
        assert_eq!(owners.pad_of(P1), None, "its first press there is P1's");
    }

    #[test]
    fn a_controller_back_takes_back_its_player() {
        let rules = rules();
        let mut owners = guests(&[A, B]);
        gone(&mut owners, A, &rules);
        gone(&mut owners, B, &rules);
        owners.back(B, model(B), &rules);
        assert_eq!((owners.pad_of(2), owners.pad_of(3)), (None, Some(B)));
        assert_eq!(
            owners.effects(),
            [
                Effect::Waits(2),
                Effect::Waits(3),
                Effect::Found { seat: 3, id: B }
            ]
        );
    }

    #[test]
    fn another_controller_takes_over_or_lets_go_the_longest_waiting() {
        let rules = rules();
        let mut owners = guests(&[A, B]);
        gone(&mut owners, B, &rules);
        gone(&mut owners, A, &rules);
        press(&mut owners, C, West, &rules);
        assert_eq!(owners.hold(C), None, "no offer to join meanwhile");
        press(&mut owners, C, South, &rules);
        assert_eq!(owners.pad_of(3), Some(C));
        press(&mut owners, 13, East, &rules);
        assert!(!owners.waits(2));
        assert_eq!(owners.effects().last(), Some(&Effect::Leave(2)));
    }

    #[test]
    fn a_controller_in_another_players_seat_is_not_taken_back() {
        let rules = rules();
        let mut owners = guests(&[A, B]);
        gone(&mut owners, A, &rules);
        gone(&mut owners, B, &rules);
        // As on Linux, where B's player may be back with A's id.
        owners.seat(A, 3);
        owners.back(B, model(B), &rules);
        assert!(owners.waits(2), "P2 still waits");
        assert_eq!(owners.pad_of(3), Some(A));
    }

    /// Where a reconnect reuses ids (Linux), two of one model away can't be told apart.
    #[test]
    fn two_of_one_model_away_are_not_guessed_on_linux() {
        let linux = Rules {
            identity: Identity::Slot,
            ..rules()
        };
        let same = [7; 16];
        let mut owners = guests(&[A, B]);
        owners.gone(A, same, false, &linux);
        owners.gone(B, same, false, &linux);
        owners.back(A, same, &linux);
        assert_eq!(owners.pad_of(2), None, "it could be P3's");
        owners.press(A, same, South, &linux);
        assert_eq!(owners.pad_of(2), Some(A), "its player said so");
        owners.back(B, same, &linux);
        assert_eq!(owners.pad_of(3), Some(B), "only one left: that one");
    }

    #[test]
    fn the_same_model_is_only_known_back_where_ids_change_and_unambiguous() {
        let mac = Rules {
            identity: Identity::Model,
            ..rules()
        };
        let mut owners = guests(&[A]);
        gone(&mut owners, A, &mac);
        owners.press(C, model(A), West, &mac);
        assert_eq!(owners.pad_of(2), Some(C), "any button, under a new id");

        // Windows and Linux keep the id across a reconnect: another of the same model asks.
        let rules = rules();
        let mut owners = guests(&[A]);
        gone(&mut owners, A, &rules);
        owners.press(C, model(A), West, &rules);
        assert_eq!(owners.pad_of(2), None);

        // Two wait with the same model, or a kind the system doesn't describe: no guess.
        for (pads, uuid) in [(&[A, B][..], [7; 16]), (&[A][..], [0; 16])] {
            let mut owners = guests(pads);
            for pad in pads {
                owners.gone(*pad, uuid, false, &mac);
            }
            owners.press(C, uuid, West, &mac);
            assert_eq!(owners.hold(C), None);
        }
    }

    #[test]
    fn a_hand_off_is_known_back_under_a_new_id_on_macos() {
        let mac = Rules {
            identity: Identity::Model,
            ..rules()
        };
        let mut owners = p1_with(A);
        owners.give_up(mac.now);
        gone(&mut owners, A, &mac);
        owners.press(C, model(A), South, &mac);
        assert_eq!(offers(&owners), [C], "its press only says it is back");
        owners.press(C, model(A), East, &mac);
        assert_eq!(owners.pad_of(P1), Some(C));
    }

    /// P1 alone waiting leaves the offer to join; with P2 waiting too, P2's answer comes first.
    #[test]
    fn a_guest_waiting_is_answered_before_p1() {
        let rules = rules();
        let mut owners = guests(&[B]);
        owners.seat(A, P1);
        gone(&mut owners, A, &rules);
        gone(&mut owners, B, &rules);
        press(&mut owners, C, South, &rules);
        assert_eq!((owners.pad_of(P1), owners.pad_of(2)), (None, Some(C)));
        press(&mut owners, 13, South, &rules);
        assert_eq!(owners.pad_of(P1), Some(13));
    }
}
