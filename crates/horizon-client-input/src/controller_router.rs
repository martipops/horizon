//! One controller reader for independently driven local players.
//! Poll events once, assign each PadId to a player, then read that pad's state for its game.
//! Device IDs belong to this reader; discard assignments when the reader is replaced.
use crate::gamepad::{Button, GlyphStyle, PadState};

/// A button press or connection change since the previous poll.
#[derive(Debug)]
pub enum PadEvent {
    Press(PadId, Button),
    Gone(PadId),
    Back(PadId),
}

fn gilrs_button(button: Button) -> gilrs::Button {
    use gilrs::Button as G;
    match button {
        Button::South => G::South,
        Button::East => G::East,
        Button::West => G::West,
        Button::North => G::North,
        Button::LeftBumper => G::LeftTrigger,
        Button::RightBumper => G::RightTrigger,
        Button::LeftTrigger => G::LeftTrigger2,
        Button::RightTrigger => G::RightTrigger2,
        Button::Select => G::Select,
        Button::Start => G::Start,
        Button::LeftStick => G::LeftThumb,
        Button::RightStick => G::RightThumb,
        Button::DPadUp => G::DPadUp,
        Button::DPadDown => G::DPadDown,
        Button::DPadLeft => G::DPadLeft,
        Button::DPadRight => G::DPadRight,
    }
}

/// A pad's buttons, sticks and triggers now.
fn state_of(pad: &gilrs::Gamepad) -> PadState {
    state_from(
        |b| {
            (
                pad.is_pressed(b),
                pad.button_data(b).map_or(0.0, |d| d.value()),
            )
        },
        |a| pad.value(a),
    )
}

fn state_from(
    button_data: impl Fn(gilrs::Button) -> (bool, f32),
    axis: impl Fn(gilrs::Axis) -> f32,
) -> PadState {
    let mut state = PadState::default();
    for button in Button::ALL {
        if !matches!(button, Button::LeftTrigger | Button::RightTrigger) {
            state.set(button, button_data(gilrs_button(button)).0);
        }
    }
    use gilrs::Axis as A;
    state.left = [axis(A::LeftStickX), axis(A::LeftStickY)];
    state.right = [axis(A::RightStickX), axis(A::RightStickY)];
    // Triggers arrive as analog buttons on most mappings, as plain buttons on some.
    let trigger = |b| {
        let (pressed, value) = button_data(b);
        if pressed { value.max(1.0) } else { value }
    };
    state.triggers = [
        trigger(gilrs::Button::LeftTrigger2),
        trigger(gilrs::Button::RightTrigger2),
    ];
    state
}

/// A controller, as splitscreen tells them apart.
pub type PadId = gilrs::GamepadId;

/// Splitscreen: one gilrs for every player's game in this process. The split decides which
/// player each pad drives; this reads them.
#[derive(Default)]
pub struct PadRouter {
    gilrs: Option<gilrs::Gilrs>,
    started: bool,
}

impl PadRouter {
    /// The buttons pressed and the controllers gone or back since the last call, in order.
    pub fn events(&mut self) -> Vec<PadEvent> {
        if !self.started {
            self.started = true;
            self.gilrs = gilrs::Gilrs::new().ok();
        }
        let Some(gilrs) = self.gilrs.as_mut() else {
            return Vec::new();
        };
        let mut events = Vec::new();
        while let Some(gilrs::Event { id, event, .. }) = gilrs.next_event() {
            match event {
                gilrs::EventType::ButtonPressed(pressed, _) => {
                    if let Some(button) = Button::ALL
                        .into_iter()
                        .find(|&b| gilrs_button(b) == pressed)
                    {
                        events.push(PadEvent::Press(id, button));
                    }
                }
                gilrs::EventType::Disconnected => {
                    events.push(PadEvent::Gone(id));
                }
                gilrs::EventType::Connected => events.push(PadEvent::Back(id)),
                _ => {}
            }
        }
        events
    }

    pub fn connected(&self, id: PadId) -> bool {
        self.gilrs
            .as_ref()
            .is_some_and(|g| g.connected_gamepad(id).is_some())
    }

    /// The kind of controller (its maker, model and bus; two of one model share it), to know it
    /// back when it reconnects under a new id; zero when the system doesn't say.
    pub fn uuid(&self, id: PadId) -> [u8; 16] {
        self.gilrs
            .as_ref()
            .map_or([0; 16], |g| g.gamepad(id).uuid())
    }

    /// The pad's state now (released when it is gone).
    pub fn state(&self, id: PadId) -> PadState {
        self.gilrs
            .as_ref()
            .and_then(|g| g.connected_gamepad(id))
            .map_or_else(PadState::default, |pad| state_of(&pad))
    }

    /// Its glyphs (Xbox, PlayStation, Nintendo), as `read` finds them for the active pad.
    pub fn style(&self, id: PadId) -> GlyphStyle {
        self.gilrs
            .as_ref()
            .and_then(|g| g.connected_gamepad(id))
            .map_or(GlyphStyle::Auto, |pad| {
                GlyphStyle::of_vendor(pad.vendor_id())
            })
    }

    pub fn name(&self, id: PadId) -> String {
        self.gilrs
            .as_ref()
            .and_then(|g| g.connected_gamepad(id))
            .map_or_else(|| "A controller".into(), |pad| pad.name().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analog_and_digital_triggers_keep_their_own_states() {
        let state = state_from(
            |button| match button {
                gilrs::Button::South => (true, 1.0),
                gilrs::Button::LeftTrigger2 => (false, 0.4),
                gilrs::Button::RightTrigger2 => (true, 0.0),
                _ => (false, 0.0),
            },
            |axis| match axis {
                gilrs::Axis::LeftStickX => 0.5,
                gilrs::Axis::RightStickY => -0.75,
                _ => 0.0,
            },
        );
        assert!(state.held(Button::South));
        assert!(!state.held(Button::East));
        assert_eq!(state.left, [0.5, 0.0]);
        assert_eq!(state.right, [0.0, -0.75]);
        assert_eq!(state.triggers, [0.4, 1.0]);
        let released = state_from(|_| (false, 0.0), |_| 0.0);
        assert_eq!(released, PadState::default());
        assert_eq!(
            state.triggers,
            [0.4, 1.0],
            "reading another pad must not alter this snapshot"
        );
    }
}
