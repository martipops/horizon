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
        assert!(slots_set(&opened, window).1, "the open sends the whole menu");

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

/// `handleContainerClick`: a click in the player's inventory changes no slot of the
/// stonecutter's own, so none of them is sent. The slot the click listed and the carried stack
/// are.
#[test]
#[ignore = "daily"]
fn a_click_in_the_inventory_sends_no_station_slot() {
    with_game(|game| {
        let rx = stick_guest(game);
        let window = stonecutter_with_stone(game);
        sent(&rx);

        let click = play::ContainerClick {
            container: window,
            state: 0,
            slot: STICK_SLOT,
            button: 0,
            input: 0,
            changed: vec![(STICK_SLOT, None)],
            carried: None,
        };
        assert!(game.lan_station_click(1, &click));
        let packets = sent(&rx);
        assert_eq!(slots_set(&packets, window), (vec![STICK_SLOT], false));
        let carried: Vec<Option<i32>> = packets
            .iter()
            .filter_map(|packet| match packet {
                C::SetCursorItem(p) => Some(p.item.as_ref().map(|stack| stack.count)),
                _ => None,
            })
            .collect();
        assert_eq!(carried, [Some(1)], "the picked-up stick is on the cursor");

        // A click the server changes nothing for (the result slot takes no stick): the slots
        // the client listed are sent again as the server has them, so a wrong prediction there
        // is corrected.
        let refused = play::ContainerClick {
            slot: RESULT,
            changed: vec![(INPUT, None), (RESULT, None)],
            ..click
        };
        assert!(game.lan_station_click(1, &refused));
        assert_eq!(
            slots_set(&sent(&rx), window),
            (vec![INPUT, RESULT], false)
        );
    });
}
