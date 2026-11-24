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
}
impl Keys {
    pub fn direction(self) -> [f32; 2] {
        match self.analog {
            Some(v) => v,
            None => input::keyboard_direction(self.forward, self.backward, self.left, self.right),
        }
    }
    /// Add a stick's move vector: the direction flags code reading keys
    /// (sprint, vehicles, the server's input packet) see are the keys of the
    /// one of 8 headings it is nearest, past 0.3 of its tilt
    /// (`gamepad::eight_direction_keys`), as `movement_key_held` reads them.
    pub fn with_stick(mut self, stick: Option<[f32; 2]>) -> Self {
        if let Some(v) = stick {
            let [forward, backward, left, right] = crate::gamepad::eight_direction_keys(v);
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
        // A stick counts as forward where it holds the forward key, with a margin once it does
        // (`gamepad::forward_key_held`): not any forward part, which would sprint sideways with
        // the stick hard left, nor a flag that flickers at an edge into a double-tap.
        let forward = match keys.analog {
            Some(v) => crate::gamepad::forward_key_held(v, old_forward) && !keys.backward,
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

    fn stick(v: [f32; 2]) -> Keys {
        Keys::default().with_stick(Some(v))
    }

    /// The stick hard left with a little forward is a strafe: the sprint key doesn't sprint
    /// sideways, as only the left key is held.
    #[test]
    fn a_stick_strafing_left_does_not_sprint() {
        let mut state = SprintState::default();
        let mut sprinting = false;
        let mut keys = stick([0.98, 0.17]);
        keys.sprint = true;
        assert!(!keys.forward);
        step(&mut state, keys, &mut sprinting);
        assert!(!sprinting);
    }

    /// A stick resting on the tilt that starts walking, as a thumb does, never double-taps
    /// forward into a sprint.
    #[test]
    fn a_stick_resting_on_the_key_tilt_does_not_start_a_sprint() {
        let mut state = SprintState::default();
        let mut sprinting = false;
        for i in 0..12 {
            let tilt = if i % 2 == 0 { 0.31 } else { 0.29 };
            step(&mut state, stick([0.0, tilt]), &mut sprinting);
            assert!(!sprinting, "tick {i}");
        }
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
