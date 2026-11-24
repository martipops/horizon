//! What a station's player is sent after a recipe pick and after a click: the slots that
//! changed, not the whole menu (`AbstractContainerMenu.broadcastChanges`).
use super::*;
use crate::game::native_entity_refs::tests::with_game;
use crate::game::native_stations_tests::{sent, stick_guest};

/// `StonecutterMenu.INPUT_SLOT` and `RESULT_SLOT`.
const INPUT: i16 = 0;
const RESULT: i16 = 1;
/// The guest's hotbar slot holding the fixture's stick, and its slot in a stonecutter's menu:
/// the menu's 2 slots and the inventory's 27 come first.
const STICK_HOTBAR: usize = 5;
const STICK_SLOT: i16 = 2 + 27 + STICK_HOTBAR as i16;

/// A stonecutter open for the fixture's guest with four stone in it; its window.
fn stonecutter_with_stone(game: &mut Game<'_>) -> i32 {
    game.lan_open_station_at(
        1,
        StationKind::Stonecutter,
        [6, 200, 10],
        vec![Some(ItemStack::new("minecraft:stone", 4))],
    )
    .expect("guests use stonecutters")
    .expect("the window opened")
}

/// The slots of `window` the packets set one by one, and whether one of them is the whole menu.
fn slots_set(packets: &[C], window: i32) -> (Vec<i16>, bool) {
    let slots = packets
        .iter()
        .filter_map(|packet| match packet {
            C::ContainerSetSlot(p) if p.container == window => Some(p.slot),
            _ => None,
        })
        .collect();
    let whole = packets
        .iter()
        .any(|packet| matches!(packet, C::ContainerSetContent(p) if p.container == window));
    (slots, whole)
}

/// The state id of the last packet about `window`, which the client's next click carries.
fn last_state(packets: &[C], window: i32) -> i32 {
    packets
        .iter()
        .rev()
        .find_map(|packet| match packet {
            C::ContainerSetSlot(p) if p.container == window => Some(p.state),
            C::ContainerSetContent(p) if p.container == window => Some(p.state),
            _ => None,
        })
        .expect("a packet about the window")
}

/// `handleContainerButtonClick`: `clickMenuButton` sets the result, and `broadcastChanges` sends
/// that slot and the selected recipe. The input slot is not set again, which on the client
/// would run `StonecutterScreen.containerChanged` and scroll the recipe list back to the top.
#[test]
#[ignore = "daily"]
fn a_recipe_pick_sends_the_result_and_leaves_the_input_alone() {
    with_game(|game| {
        let rx = stick_guest(game);
        let window = stonecutter_with_stone(game);
        let opened = sent(&rx);
        assert!(
            slots_set(&opened, window).1,
            "the open sends the whole menu"
        );

        game.lan_station_button(1, window, 0);
        let packets = sent(&rx);
        assert_eq!(slots_set(&packets, window), (vec![RESULT], false));
        let selected: Vec<(i16, i16)> = packets
            .iter()
            .filter_map(|packet| match packet {
                C::ContainerSetData(p) if p.container == window => Some((p.id, p.value)),
                _ => None,
            })
            .collect();
        assert_eq!(selected, [(0, 0)], "selectedRecipeIndex follows");
    });
}

/// The stack's `HashedStack`, as the client sends it for a slot it predicts.
fn claim(game: &mut Game<'_>, item: &str, count: u32) -> Option<horizon_protocol::item::Hashed> {
    let hub = &mut game.lan.as_mut().unwrap().hub;
    let stack = hub.stack(&Some(ItemStack::new(item, count))).unwrap();
    Some(horizon_protocol::item::Hashed::of(&stack))
}

/// The counts the packets put on the cursor.
fn cursors(packets: &[C]) -> Vec<Option<i32>> {
    packets
        .iter()
        .filter_map(|packet| match packet {
            C::SetCursorItem(p) => Some(p.item.as_ref().map(|stack| stack.count)),
            _ => None,
        })
        .collect()
}

/// `handleContainerClick`: a click the client predicted right is answered with nothing, one it
/// predicted wrong with the slots it has wrong and the carried stack, and one on an older state
/// with the whole menu and all its data.
#[test]
#[ignore = "daily"]
fn a_click_in_the_inventory_sends_no_station_slot() {
    with_game(|game| {
        let rx = stick_guest(game);
        let window = stonecutter_with_stone(game);
        let opened = sent(&rx);
        let stick = claim(game, "minecraft:stick", 1);

        let click = play::ContainerClick {
            container: window,
            state: last_state(&opened, window),
            slot: STICK_SLOT,
            button: 0,
            input: 0,
            changed: vec![(STICK_SLOT, None)],
            carried: stick,
        };
        assert!(game.lan_station_click(1, &click));
        let packets = sent(&rx);
        assert_eq!(slots_set(&packets, window), (vec![], false));
        assert!(cursors(&packets).is_empty(), "the cursor was predicted");

        // A click the server changes nothing for (the result slot takes no stick), which the
        // client predicted to clear the input and the cursor.
        let refused = play::ContainerClick {
            slot: RESULT,
            changed: vec![(INPUT, None), (RESULT, None)],
            carried: None,
            ..click
        };
        assert!(game.lan_station_click(1, &refused));
        let packets = sent(&rx);
        assert_eq!(slots_set(&packets, window), (vec![INPUT, RESULT], false));
        assert_eq!(cursors(&packets), [Some(1)], "the stick is still held");

        // `fullResyncNeeded`: that answer moved the state on, so a click still carrying the
        // older one gets the whole menu, its unchanged selected recipe too.
        assert!(game.lan_station_click(1, &refused));
        let packets = sent(&rx);
        assert_eq!(slots_set(&packets, window), (Vec::new(), true));
        let data: Vec<(i16, i16)> = packets
            .iter()
            .filter_map(|packet| match packet {
                C::ContainerSetData(p) if p.container == window => Some((p.id, p.value)),
                _ => None,
            })
            .collect();
        assert_eq!(data, [(0, -1)]);
    });
}

/// Taking a result changes the input. The client predicted that (and scrolled its list to the
/// top then), so the answer must not set the input again: it would scroll the list once more,
/// under a player who has gone on scrolling while the answer was on its way.
#[test]
#[ignore = "daily"]
fn a_rightly_predicted_take_does_not_set_the_input_again() {
    with_game(|game| {
        let rx = stick_guest(game);
        let window = stonecutter_with_stone(game);
        game.lan_station_button(1, window, 0);
        let picked = sent(&rx);
        let result = picked
            .iter()
            .find_map(|packet| match packet {
                C::ContainerSetSlot(p) if p.container == window && p.slot == RESULT => {
                    p.item.clone()
                }
                _ => None,
            })
            .expect("the picked recipe's result");
        let take = play::ContainerClick {
            container: window,
            state: last_state(&picked, window),
            slot: RESULT,
            button: 0,
            input: 0,
            changed: vec![(INPUT, claim(game, "minecraft:stone", 3))],
            carried: Some(horizon_protocol::item::Hashed::of(&result)),
        };
        assert!(game.lan_station_click(1, &take));
        let packets = sent(&rx);
        assert_eq!(slots_set(&packets, window), (vec![], false));
        assert!(cursors(&packets).is_empty(), "the cursor was predicted");

        // The next take leaves two stone: the client's three are corrected.
        let mut both = result.clone();
        both.count *= 2;
        let wrong = play::ContainerClick {
            carried: Some(horizon_protocol::item::Hashed::of(&both)),
            ..take
        };
        assert!(game.lan_station_click(1, &wrong));
        let packets = sent(&rx);
        assert_eq!(slots_set(&packets, window), (vec![INPUT, RESULT], false));
        assert!(cursors(&packets).is_empty(), "the cursor was predicted");
    });
}

/// A claim is compared with its components, not by item and count alone.
#[test]
#[ignore = "daily"]
fn a_claim_with_other_components_does_not_match() {
    with_game(|game| {
        let rx = stick_guest(game);
        drop(rx);
        let hub = &mut game.lan.as_mut().unwrap().hub;
        let mut sword = ItemStack::new("minecraft:diamond_sword", 1);
        let plain = hub.stack(&Some(sword.clone())).unwrap();
        sword.damage = 5;
        let worn = hub.stack(&Some(sword.clone())).unwrap();
        let hash = |stack| Some(horizon_protocol::item::Hashed::of(stack));
        let sword = Some(sword);
        assert!(hub.claim_matches(&hash(&worn), &sword));
        assert!(!hub.claim_matches(&hash(&plain), &sword));
    });
}
