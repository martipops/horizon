//! Taps shorter than a poll, on a real `Game` without a window (`gamepad::Pulses` has the rest).

use super::parity_tests::{NoEventLoop, playing};
use crate::game::LoadingOverlayPreview;
use horizon_client::gamepad::{Button, PadState};
use std::time::Duration;

#[test]
#[ignore = "daily"]
fn b_and_start_close_the_loading_overlay_preview() {
    playing(|game| {
        game.focused = true;
        game.pad.feed = Some(PadState::default());
        for button in [Button::East, Button::Start] {
            let mut down = PadState::default();
            down.set(button, true);
            let rest = PadState::default();
            // Held at the poll, and tapped between two polls behind another button's tap.
            let mut other = PadState::default();
            other.set(Button::South, true);
            for (live, moments) in [(down, vec![down]), (rest, vec![other, rest, down, rest])] {
                game.pad.feed = Some(rest);
                game.poll_pads(&NoEventLoop);
                game.loading_overlay_preview
                    .set(Some(LoadingOverlayPreview::Queued(Duration::from_secs(60))));
                game.poll_pads(&NoEventLoop);
                assert!(!closing(game), "nothing pressed yet");
                game.pad.feed = Some(live);
                for moment in &moments {
                    game.pad.between.push(moment);
                }
                game.poll_pads(&NoEventLoop);
                assert!(closing(game), "{button:?} {live:?}");
                assert!(!game.pad.pulses.waiting(), "nothing left for the screen below");
                // The press that closed it is none of that screen's.
                game.loading_overlay_preview.set(None);
                game.poll_pads(&NoEventLoop);
                assert!(!game.pad.edges.is_held(button));
            }
        }
    });
}

fn closing(game: &crate::game::Game) -> bool {
    matches!(
        game.loading_overlay_preview.get(),
        Some(LoadingOverlayPreview::Closing)
    )
}
