//! LocalPlayer sprint decisions around KeyboardInput.tick. Conditions are the
//! current player queries; setting sprint dispatches to the live attribute state.
use crate::input;

#[derive(Clone, Copy, Debug, Default)]
pub struct Keys {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub shift: bool,
    pub sprint: bool,
    /// A movement-input listener explicitly holds sprint off this local tick.
    /// Server prediction instead replays the client's resulting sprint state.
    pub sprint_suppressed: bool,
    /// A controller stick's move vector (`[left, forward]`, Horizon's own),
    /// used instead of the keys' direction while it is pushed.
    pub analog: Option<[f32; 2]>,
    /// The movement keys a stock controller stick holds this tick, with their
    /// margin (`gamepad::eight_direction_keys_held`): the flags, and sprint's
    /// forward. `None` for keys, and for axes a mod supplied.
    pub stick: Option<[bool; 4]>,
}
impl Keys {
    pub fn direction(self) -> [f32; 2] {
        match self.analog {
            Some(v) => v,
            None => input::keyboard_direction(self.forward, self.backward, self.left, self.right),
        }
    }
    /// Add a stick's move vector and the keys it holds (`held`, for a stock
    /// stick; a mod's vector gets `gamepad::eight_direction_keys`): the
    /// direction flags code reading keys (sprint, vehicles, the server's input
    /// packet) see are those keys, as `movement_key_held` reads them.
    pub fn with_stick(mut self, stick: Option<[f32; 2]>, held: Option<[bool; 4]>) -> Self {
        if let Some(v) = stick {
            let keys = held.unwrap_or_else(|| crate::gamepad::eight_direction_keys(v));
            let [forward, backward, left, right] = keys;
            self.stick = held;
            self.forward |= forward;
            self.backward |= backward;
            self.left |= left;
            self.right |= right;
            self.analog = Some(v);
        }
        self
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct SprintState {
    pub trigger_time: i32,
    pub previous_forward: bool,
    pub previous_shift: bool,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Conditions {
    pub sprinting: bool,
    pub mobility_restricted: bool,
    pub passenger: bool,
    pub vehicle_can_sprint: bool,
    pub vehicle_local: bool,
    pub enough_food: bool,
    pub may_fly: bool,
    pub flying: bool,
    pub shallow_water: bool,
    pub slow_item: bool,
    pub fall_flying: bool,
    pub under_water: bool,
    pub moving_slowly: bool,
    pub swimming: bool,
    pub in_water: bool,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub minor_horizontal_collision: bool,
}
impl Conditions {
    fn possible(self, allow_shallow_water: bool) -> bool {
        !self.mobility_restricted
            && if self.passenger {
                self.vehicle_can_sprint && self.vehicle_local
            } else {
                self.enough_food || self.may_fly
            }
            && (allow_shallow_water || !self.shallow_water)
    }
    fn can_start(self, forward: bool) -> bool {
        !self.sprinting
            && forward
            && self.possible(self.flying)
            && !self.slow_item
            && (!self.fall_flying || self.under_water)
            && (!self.moving_slowly || self.under_water)
    }
    fn should_stop(self, forward: bool, shift: bool) -> bool {
        if self.swimming {
            !self.possible(true) || !self.in_water || (!forward && !self.on_ground && !shift)
        } else {
            !self.possible(self.flying)
                || !forward
                || (self.horizontal_collision && !self.minor_horizontal_collision)
        }
    }
}
impl SprintState {
    /// `LocalPlayer.aiStep`'s sprint phase, with a mod's optional tick suppression.
    /// The default vanilla window is seven ticks; options can select 0..10.
    /// The setter runs for every native request, including repeated true requests.
    pub fn tick<E>(
        &mut self,
        keys: Keys,
        conditions: Conditions,
        window: i32,
        mut set_sprinting: impl FnMut(bool) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.trigger_time > 0 {
            self.trigger_time -= 1;
        }
        let old_forward = self.previous_forward;
        let old_shift = self.previous_shift;
        // A stock stick counts as forward where it holds the forward key, with its margin
        // (`Keys::stick`): not any forward part, which would sprint sideways with the stick hard
        // left, nor a flag that flickers at an edge into a double-tap. Keys and a mod's axes keep
        // vanilla's `hasForwardImpulse`.
        let forward = match keys.stick {
            Some([ahead, ..]) => ahead && !keys.backward,
            None => keys.direction()[1] > 1.0e-5,
        };
        self.previous_forward = forward;
        self.previous_shift = keys.shift;
        if old_shift || (conditions.slow_item && !conditions.passenger) || keys.backward {
            self.trigger_time = 0;
        }
        if keys.sprint_suppressed {
            self.trigger_time = 0;
            if conditions.sprinting {
                set_sprinting(false)?;
            }
            return Ok(());
        }
        let mut sprinting = conditions.sprinting;
        if conditions.can_start(forward) {
            if !old_forward {
                if self.trigger_time > 0 {
                    set_sprinting(true)?;
                    sprinting = true;
                } else {
                    self.trigger_time = window;
                }
            }
            if keys.sprint {
                set_sprinting(true)?;
                sprinting = true;
            }
        }
        if sprinting && conditions.should_stop(forward, keys.shift) {
            set_sprinting(false)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing() -> Conditions {
        Conditions {
            enough_food: true,
            on_ground: true,
            ..Conditions::default()
        }
    }

    fn step(state: &mut SprintState, keys: Keys, sprinting: &mut bool) {
        let current = *sprinting;
        let mut next = current;
        state
            .tick(
                keys,
                Conditions {
                    sprinting: current,
                    ..standing()
                },
                7,
                |value| {
                    next = value;
                    Ok::<(), ()>(())
                },
            )
            .unwrap();
        *sprinting = next;
    }

    /// A stock stick at `v` that held `previous` last tick.
    fn stick(v: [f32; 2], previous: [bool; 4]) -> Keys {
        let held = crate::gamepad::eight_direction_keys_held(v, previous);
        Keys::default().with_stick(Some(v), Some(held))
    }

    /// The stick hard left with a little forward is a strafe: the sprint key doesn't sprint
    /// sideways, as only the left key is held.
    #[test]
    fn a_stick_strafing_left_does_not_sprint() {
        let mut state = SprintState::default();
        let mut sprinting = false;
        let mut keys = stick([0.98, 0.17], [false; 4]);
        keys.sprint = true;
        assert!(!keys.forward);
        step(&mut state, keys, &mut sprinting);
        assert!(!sprinting);
    }

    /// A stick resting on the tilt that starts walking, or swinging by an edge of the forward
    /// sectors, never double-taps forward into a sprint; a real double tap still sprints.
    #[test]
    fn only_a_real_double_tap_of_the_stick_sprints() {
        let at = |degrees: f32, tilt: f32| {
            let (s, c) = degrees.to_radians().sin_cos();
            [tilt * s, tilt * c]
        };
        let run = |samples: &[[f32; 2]]| {
            let (mut state, mut sprinting, mut held) = (SprintState::default(), false, [false; 4]);
            for &v in samples {
                let keys = stick(v, held);
                held = keys.stick.unwrap_or_default();
                step(&mut state, keys, &mut sprinting);
            }
            sprinting
        };
        let resting: Vec<_> = (0..12).map(|i| at(0.0, [0.31, 0.29][i % 2])).collect();
        assert!(!run(&resting));
        assert!(!run(&[at(20.0, 0.31), at(27.0, 0.29), at(27.0, 0.31)]));
        assert!(!run(&[at(60.0, 0.6), at(71.0, 0.6), at(67.0, 0.6)]));
        assert!(run(&[at(0.0, 1.0), [0.0, 0.0], at(0.0, 1.0)]));
    }

    /// A mod's own axes keep vanilla's forward impulse: a light forward with Sprint sprints.
    #[test]
    fn a_mods_light_forward_axis_still_sprints() {
        let mut state = SprintState::default();
        let mut sprinting = false;
        let keys = Keys {
            forward: true,
            sprint: true,
            analog: Some([0.0, 0.2]),
            ..Keys::default()
        };
        step(&mut state, keys, &mut sprinting);
        assert!(sprinting);
    }

    /// Vanilla sprint is a state. Releasing the sprint key, including on the
    /// jump tick, keeps it while forward is held; releasing forward stops it.
    #[test]
    fn sprint_survives_releasing_the_key_through_a_jump() {
        let mut state = SprintState::default();
        let mut sprinting = false;
        step(
            &mut state,
            Keys {
                forward: true,
                sprint: true,
                ..Keys::default()
            },
            &mut sprinting,
        );
        assert!(sprinting);
        step(
            &mut state,
            Keys {
                forward: true,
                jump: true,
                ..Keys::default()
            },
            &mut sprinting,
        );
        assert!(
            sprinting,
            "a jump after the sprint key comes up must stay a sprint"
        );
        step(
            &mut state,
            Keys {
                jump: true,
                ..Keys::default()
            },
            &mut sprinting,
        );
        assert!(!sprinting);
    }
}
