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

/// A controller P1 gave away (Give Up Pad). Until its new player is in the world, a hand-off that
/// ends gives that same controller back if P1 has no other, never another player's.
struct HandOff<Id> {
    id: Id,
    away: Option<Away<Id>>,
}

/// It disconnected with no player driving it yet: back, it is offered again while its offer runs,
/// else it is P1's.
struct Away<Id> {
    uuid: [u8; 16],
    offer: Option<Instant>,
    /// The controllers of its kind connected since: it may be one of them, under another id.
    arrived: Vec<Id>,
}

impl<Id> Away<Id> {
    fn new(uuid: [u8; 16], offer: Option<Instant>) -> Self {
        Self {
            uuid,
            offer,
            arrived: Vec::new(),
        }
    }
}

/// How a controller is known back after it disconnects.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Identity {
    /// Its id stays its own (Windows).
    Id,
    /// Its id goes to the next controller of its model that connects (Linux: gilrs reuses the
    /// first slot of that model free, whoever had it), so it means nothing while two of that
    /// model are disconnected.
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
    given_up: Vec<HandOff<Id>>,
    /// The disconnected controllers whose ids another may come back with (`Identity::Slot`),
    /// whoever they had.
    absent: Vec<(Id, [u8; 16])>,
    effects: Vec<Effect<Id>>,
    /// The join question on the views: the controllers it is about, and whether P1 may take one.
    pub(super) asked: (Vec<Id>, bool),
}

impl<Id> Default for Owners<Id> {
    fn default() -> Self {
        Self {
            held: Vec::new(),
            lost: Vec::new(),
            given_up: Vec::new(),
            absent: Vec::new(),
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
        if let Some(k) = self.lost.iter().position(|l| l.seat == seat) {
            let lost = self.lost.remove(k);
            if lost.id != Some(id) {
                self.hand_off_away(lost.id, lost.uuid, None);
            }
        }
        // Its id stands for another player's controller now.
        for lost in self.lost.iter_mut().filter(|l| l.id == Some(id)) {
            lost.id = None;
        }
        if seat == P1 {
            self.given_up.retain(|h| h.id != id);
        } else if let Some(hand_off) = self.given_up.iter_mut().find(|h| h.id == id) {
            hand_off.away = None;
        }
    }

    /// The controller P1 gave up, `id`, disconnected and drives nobody; nothing for another.
    fn hand_off_away(&mut self, id: Option<Id>, uuid: [u8; 16], offer: Option<Instant>) {
        if let Some(hand_off) = self.given_up.iter_mut().find(|h| Some(h.id) == id) {
            hand_off.away = Some(Away::new(uuid, offer));
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
        let before = self.given_up.len();
        self.given_up.retain(|h| h.id != id);
        if self.given_up.len() < before && self.pad_of(P1).is_none() {
            self.seat_p1(id);
        }
    }

    /// P1's Give Up Pad: its controller is offered to join.
    pub(super) fn give_up(&mut self, now: Instant) {
        if let Some(id) = self.pad_of(P1) {
            self.put(id, Hold::Offered(now));
            self.given_up.push(HandOff { id, away: None });
        }
    }

    /// The player in `seat` is in the world: a controller P1 gave up is its for good.
    pub(super) fn entered(&mut self, seat: usize) {
        let pad = self.pad_of(seat);
        let lost = self.lost.iter().find(|l| l.seat == seat).and_then(|l| l.id);
        self.given_up
            .retain(|h| Some(h.id) != pad && Some(h.id) != lost);
    }

    /// The player in `seat` left.
    pub(super) fn left(&mut self, seat: usize) {
        if let Some(id) = self.pad_of(seat) {
            self.end_offer(id);
        }
        if let Some(k) = self.lost.iter().position(|l| l.seat == seat) {
            let lost = self.lost.remove(k);
            self.hand_off_away(lost.id, lost.uuid, None);
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
        for away in self.given_up.iter_mut().filter_map(|h| h.away.as_mut()) {
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
        // One P1 gave up, back under another id: the press only says so, whoever else waits.
        let returning = match self.hold(id) {
            None => self.returning(id, uuid, rules.identity),
            Some(_) => None,
        };
        if let Some(k) = returning
            && self.one_away(uuid)
        {
            return self.resume(k, id, rules);
        }
        // Then the players waiting for a controller: the same kind back takes back its player,
        // another takes over the one waiting longest (`claim`).
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
        let p1_has_pad = self.pad_of(P1).is_some();
        if p1_takes(
            rules.in_world,
            self.waits(P1),
            rules.p1_controllers,
            p1_has_pad,
        ) {
            return self.seat_p1(id);
        }
        if let Some(k) = returning {
            return self.resume(k, id, rules);
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
        if rules.identity == Identity::Slot {
            self.absent.push((id, uuid));
        }
        for away in self.given_up.iter_mut().filter_map(|h| h.away.as_mut()) {
            away.arrived.retain(|p| *p != id);
        }
        let Some(hold) = self.hold(id) else {
            return;
        };
        self.free(id);
        let seat = match hold {
            Hold::Seat(seat) => seat,
            Hold::Offered(since) => return self.hand_off_away(Some(id), uuid, Some(since)),
            Hold::Joining => return self.hand_off_away(Some(id), uuid, None),
        };
        if seat != P1 && choosing {
            self.hand_off_away(Some(id), uuid, None);
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
        // Before its own is off the list: whether another could have this id.
        let shared = self.absent.iter().filter(|(_, u)| *u == uuid).count() > 1;
        self.absent.retain(|(p, _)| *p != id);
        let away = self.given_up.iter_mut().filter_map(|h| h.away.as_mut());
        for away in away.filter(|a| a.uuid == uuid && !a.arrived.contains(&id)) {
            away.arrived.push(id);
        }
        if self.hold(id).is_some() || shared {
            return;
        }
        if let Some(seat) = self.lost.iter().find(|l| l.id == Some(id)).map(|l| l.seat) {
            self.seat(id, seat);
            self.effects.push(Effect::Found { seat, id });
        } else if let Some(k) = self.returning(id, uuid, Identity::Id) {
            self.resume(k, id, rules);
        }
    }

    /// The controller of the hand-off `k` is back as `id`: offered again while its offer runs,
    /// else P1's.
    fn resume(&mut self, k: usize, id: Id, rules: &Rules) {
        // Another hand-off's, gone for good if its id is this one's now.
        let mut hand_off = self.given_up.remove(k);
        self.given_up.retain(|h| h.id != id);
        hand_off.id = id;
        let offer = hand_off.away.take().and_then(|away| away.offer);
        self.given_up.push(hand_off);
        match offer {
            Some(since) if self.may_offer(rules) => self.put(id, Hold::Offered(since)),
            _ => self.end_offer(id),
        }
    }

    /// The hand-off whose controller, away, may be `id`: by its id, or where that says too little,
    /// by its model.
    fn returning(&self, id: Id, uuid: [u8; 16], identity: Identity) -> Option<usize> {
        let by_model = |away: &Away<Id>| {
            identity != Identity::Id
                && uuid != [0; 16]
                && away.uuid == uuid
                && away.arrived.contains(&id)
        };
        let away = |by: &dyn Fn(&HandOff<Id>, &Away<Id>) -> bool| {
            let matches = |h: &HandOff<Id>| h.away.as_ref().is_some_and(|away| by(h, away));
            self.given_up.iter().position(matches)
        };
        away(&|h, _| h.id == id).or_else(|| away(&|_, away| by_model(away)))
    }

    /// This kind of controller is known, and one player or hand-off waits for one.
    fn one_away(&self, uuid: [u8; 16]) -> bool {
        let given_up = self.given_up.iter().filter_map(|h| h.away.as_ref());
        let away = given_up.filter(|a| a.uuid == uuid).count();
        let lost = self.lost.iter().filter(|l| l.uuid == uuid).count();
        uuid != [0; 16] && away + lost == 1
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
        owners.back(C, model(A), &mac);
        owners.press(C, model(A), South, &mac);
        assert_eq!(offers(&owners), [C], "its press only says it is back");
        owners.press(C, model(A), East, &mac);
        assert_eq!(owners.pad_of(P1), Some(C));
    }

    /// Its first press there, whatever the button, is not an answer for P2, who waits for another
    /// model.
    #[test]
    fn a_hand_off_back_answers_no_waiting_guest() {
        let mac = Rules {
            identity: Identity::Model,
            ..rules()
        };
        for button in [East, South] {
            let mut owners = guests(&[B]);
            owners.seat(A, P1);
            owners.give_up(mac.now);
            gone(&mut owners, A, &mac);
            gone(&mut owners, B, &mac);
            owners.effects();
            owners.back(C, model(A), &mac);
            owners.press(C, model(A), button, &mac);
            assert_eq!(offers(&owners), [C], "{button:?}");
            assert!(owners.waits(2), "{button:?}");
            assert_eq!(owners.effects(), [], "{button:?}");
        }
        // Both of one model: nothing says whose it is, so P2's question is answered.
        let mut owners = guests(&[B]);
        owners.seat(A, P1);
        owners.give_up(mac.now);
        owners.gone(A, [7; 16], false, &mac);
        owners.gone(B, [7; 16], false, &mac);
        owners.back(C, [7; 16], &mac);
        owners.press(C, [7; 16], South, &mac);
        assert_eq!(owners.pad_of(2), Some(C));
    }

    /// Another of its model that was connected all along is not it.
    #[test]
    fn a_hand_off_is_not_taken_by_one_connected_all_along() {
        let mac = Rules {
            identity: Identity::Model,
            ..rules()
        };
        let mut owners = p1_with(A);
        owners.give_up(mac.now);
        owners.gone(A, [7; 16], false, &mac);
        let late = later(mac);
        owners.expire(&late);
        owners.press(C, [7; 16], West, &late);
        assert_eq!((owners.pad_of(P1), offers(&owners)), (None, vec![C]));
        owners.press(C, [7; 16], East, &late);
        assert_eq!(owners.pad_of(P1), None, "P1 never had it");
    }

    /// Linux: an unused one of its model left a lower slot free, which it comes back in.
    #[test]
    fn a_hand_off_back_in_another_slot_is_known_by_a_press() {
        let linux = Rules {
            identity: Identity::Slot,
            ..rules()
        };
        let same = [7; 16];
        let replugged = || {
            let mut owners = p1_with(B);
            owners.gone(A, same, false, &linux);
            owners.give_up(linux.now);
            owners.gone(B, same, false, &linux);
            owners
        };
        let mut owners = replugged();
        owners.back(A, same, &linux);
        assert_eq!(offers(&owners), NONE, "A's slot: not for sure P1's");
        owners.press(A, same, South, &linux);
        assert_eq!(offers(&owners), [A], "its press says it is");
        owners.press(A, same, East, &linux);
        assert_eq!(owners.pad_of(P1), Some(A));

        let mut owners = replugged();
        let late = later(Rules { ..rules() });
        let late = Rules {
            identity: Identity::Slot,
            ..late
        };
        owners.expire(&late);
        owners.back(A, same, &late);
        owners.press(A, same, West, &late);
        assert_eq!(owners.pad_of(P1), Some(A), "its offer ran out: P1's");
    }

    /// Linux: an unused one of its model away could come back with P2's id.
    #[test]
    fn an_unused_controller_away_makes_its_model_ambiguous_on_linux() {
        let linux = Rules {
            identity: Identity::Slot,
            ..rules()
        };
        let same = [7; 16];
        let mut owners = guests(&[A]);
        owners.gone(A, same, false, &linux);
        owners.gone(B, same, false, &linux);
        owners.back(A, same, &linux);
        assert_eq!(owners.pad_of(2), None);
        owners.press(A, same, South, &linux);
        assert_eq!(owners.pad_of(2), Some(A));
        // One only, once the other is back.
        owners.back(B, same, &linux);
        owners.gone(A, same, false, &linux);
        owners.back(A, same, &linux);
        assert_eq!(owners.pad_of(2), Some(A));
    }

    /// P1 gives up A, whose player is choosing a profile, then another, B, which goes away.
    #[test]
    fn a_second_hand_off_leaves_the_first_its_way_back() {
        let rules = rules();
        let mut owners = p1_with(A);
        owners.give_up(rules.now);
        press(&mut owners, A, South, &rules);
        owners.seat(A, 2);
        press(&mut owners, B, West, &rules);
        press(&mut owners, B, North, &rules);
        owners.give_up(rules.now);
        gone(&mut owners, B, &rules);
        owners.left(2);
        assert_eq!(owners.pad_of(P1), Some(A), "A's player cancelled");
        owners.back(B, model(B), &rules);
        press(&mut owners, B, East, &rules);
        assert_eq!(owners.pad_of(P1), Some(A), "and B is nobody's");
        assert_eq!(owners.hold(B), None);
    }

    /// The same through P1's controllers being off, with the first away.
    #[test]
    fn a_hand_off_away_keeps_its_way_back_while_another_is_offered() {
        let rules = rules();
        let off = Rules {
            p1_controllers: false,
            now: rules.now,
            ..self::rules()
        };
        let mut owners = p1_with(A);
        owners.give_up(rules.now);
        gone(&mut owners, A, &rules);
        press(&mut owners, B, West, &rules);
        press(&mut owners, B, North, &rules);
        press(&mut owners, B, West, &off);
        assert_eq!(offers(&owners), [B]);
        owners.back(A, model(A), &off);
        assert_eq!(offers(&owners), [B, A], "A's offer still runs");
        press(&mut owners, A, East, &off);
        assert_eq!(owners.pad_of(P1), Some(A));
    }

    #[test]
    fn replugging_gives_an_offer_no_more_time() {
        let rules = rules();
        let mut owners = p1_with(A);
        owners.give_up(rules.now);
        gone(&mut owners, A, &rules);
        let soon = Rules {
            now: rules.now + OFFER - Duration::from_secs(1),
            ..self::rules()
        };
        owners.expire(&soon);
        owners.back(A, model(A), &soon);
        assert_eq!(offers(&owners), [A]);
        owners.expire(&later(rules));
        assert_eq!((owners.pad_of(P1), offers(&owners)), (Some(A), vec![]));
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
