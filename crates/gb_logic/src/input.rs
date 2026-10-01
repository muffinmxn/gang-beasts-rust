//! InputState: the game's named digital/analogue actions, sampled once per FixedUpdate.
use std::collections::HashMap;

pub const JUMP: &str = "Jump";
pub const DUCK: &str = "Duck";
pub const LIFT: &str = "Lift";
pub const KICK: &str = "Kick";
pub const GRAB_LEFT: &str = "LeftGrab";
pub const GRAB_RIGHT: &str = "RightGrab";
pub const HORIZONTAL: &str = "HoriMove";
pub const VERTICAL: &str = "VertMove";

pub const DIGITAL: [&str; 6] = [JUMP, DUCK, LIFT, KICK, GRAB_LEFT, GRAB_RIGHT];

#[derive(Default, Clone, Copy)]
struct DigitalState {
    down: bool,
    previous: bool,
}

#[derive(Default, Clone)]
pub struct InputState {
    digital: HashMap<&'static str, DigitalState>,
    analogue: HashMap<&'static str, f32>,
}

impl InputState {
    /// Latch this tick's raw device state (call once per fixed step, before the actor runs).
    pub fn set(&mut self, digital: &[(&'static str, bool)], horizontal: f32, vertical: f32) {
        for (name, down) in digital {
            let s = self.digital.entry(name).or_default();
            s.previous = s.down;
            s.down = *down;
        }
        // InputState.MIN_MOVEMENT_DELTA
        let dead = |v: f32| {
            if v.abs() < 0.001 {
                0.0
            } else {
                v.clamp(-1.0, 1.0)
            }
        };
        self.analogue.insert(HORIZONTAL, dead(horizontal));
        self.analogue.insert(VERTICAL, dead(vertical));
    }

    pub fn digital(&self, name: &str) -> bool {
        self.digital.get(name).is_some_and(|s| s.down)
    }
    pub fn just_down(&self, name: &str) -> bool {
        self.digital
            .get(name)
            .is_some_and(|s| s.down && !s.previous)
    }
    pub fn just_up(&self, name: &str) -> bool {
        self.digital
            .get(name)
            .is_some_and(|s| !s.down && s.previous)
    }
    pub fn analogue(&self, name: &str) -> f32 {
        self.analogue.get(name).copied().unwrap_or(0.0)
    }
    /// True while any action is held (IdleCheck scans both dictionaries).
    pub fn any(&self) -> bool {
        self.digital.values().any(|s| s.down) || self.analogue.values().any(|v| *v != 0.0)
    }
}
