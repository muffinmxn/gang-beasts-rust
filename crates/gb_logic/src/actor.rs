//! Femur.Actor + ControlHandeler_Human + MovementHandeler_HumanoidMediumEctomorph + ground
//! CollisionHandelers for one beast, ported from GameAssembly.dll (RVAs next to each function).
//!
//! Frame order matches Unity: `fixed_update` (Actor.FixedUpdate), then `World::step`, then
//! `after_step` (OnCollisionEnter/Stay/Exit).
use crate::beast::{Beast, Part};
use crate::input::{self, InputState};
use crate::movement::align_to_vector;
use gb_phys::{ActorRef, ContactKind, World};
use glam::Vec3;

/// Actor.ActorState
pub mod state {
    pub const DEAD: u32 = 1;
    pub const UNCONSCIOUS: u32 = 2;
    pub const STAND: u32 = 4;
    pub const RUN: u32 = 8;
    pub const JUMP: u32 = 16;
    pub const FALL: u32 = 32;
    pub const CLIMB: u32 = 64;
    pub const SWIM: u32 = 128;
    pub const DRIVE: u32 = 256;
    pub const IDLE: u32 = 512;
}
use state::*;

/// MovementHandeler_HumanoidMediumEctomorph statics (.cctor).
const RUN_VECTOR_FORCE_10: Vec3 = Vec3::new(0.0, 16.0, 0.0);
const RUN_VECTOR_FORCE_5: Vec3 = Vec3::new(0.0, 8.0, 0.0);
const RUN_VECTOR_FORCE_2: Vec3 = Vec3::new(0.0, 4.0, 0.0);
const COUNTER_FORCE: Vec3 = Vec3::new(0.0, 8.0, 0.0);

const VELOCITY_CHANGE: u32 = 2;
const IMPULSE: u32 = 1;

/// Scale for the lift/throw magnitudes (`GB_LIFT_SCALE`). The source values are 1:1 (asm:
/// `ArmActionGrabbing` applies `f = 80` as `ForceMode.Impulse` to a grabbed ragdoll part, and the
/// release applies `v * 80` clamped to 40 as `ForceMode.VelocityChange`), so this defaults to 1.0
/// and exists only so the feel can be dialled without a rebuild while the joint-drive limit
/// interpretation (`gb_phys::World::set_drive_limits_are_forces`, F9) is settled.
/// The game layer sets it from the in-game dev panel (F2).
pub fn lift_scale() -> f32 {
    scale(SET_LIFT_SCALE.load(std::sync::atomic::Ordering::Relaxed))
}

/// Punch strength scale (`GB_PUNCH_SCALE`), same bridge as [`lift_scale`].
pub fn punch_scale() -> f32 {
    scale(SET_PUNCH_SCALE.load(std::sync::atomic::Ordering::Relaxed))
}

/// Sets both scales (called by the dev panel each frame); `f32` bits in an atomic keeps gb_logic
/// free of any dependency on the game crate.
pub fn set_scale_knobs(punch: f32, lift: f32) {
    SET_PUNCH_SCALE.store(punch.to_bits(), std::sync::atomic::Ordering::Relaxed);
    SET_LIFT_SCALE.store(lift.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

static SET_PUNCH_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000);
static SET_LIFT_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000);

/// Targeting strength knobs. `gb_logic` keeps the source values (FOV 90 deg, 2 m range, 1x pull)
/// so headless tests are unaffected; the game layer pushes softer values so a beast does not yank
/// its arm to every nearby collider.
pub fn target_fov() -> f32 {
    f32::from_bits(TARGET_FOV.load(std::sync::atomic::Ordering::Relaxed))
}

/// Outer candidate radius in metres (`sqrt` of `TargetingHandeler`'s 4.0 distance-squared filter).
pub fn target_range() -> f32 {
    f32::from_bits(TARGET_RANGE.load(std::sync::atomic::Ordering::Relaxed))
}

/// Scales the `ArmActionGrabbing` reach forces (10/15/30).
pub fn target_pull() -> f32 {
    f32::from_bits(TARGET_PULL.load(std::sync::atomic::Ordering::Relaxed))
}

pub fn set_target_knobs(fov: f32, range: f32, pull: f32) {
    TARGET_FOV.store(fov.clamp(10.0, 180.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
    TARGET_RANGE.store(range.clamp(0.5, 5.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
    TARGET_PULL.store(pull.clamp(0.0, 3.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

static TARGET_FOV: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x42b4_0000); // 90.0
static TARGET_RANGE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x4000_0000); // 2.0
static TARGET_PULL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000); // 1.0

fn scale(bits: u32) -> f32 {
    f32::from_bits(bits).clamp(0.0, 3.0)
}
const STABILITY: f32 = 0.1;

/// Deterministic stand-in for UnityEngine.Random (xorshift; seeded per actor).
#[derive(Clone)]
pub struct Random(u32);
impl Random {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// Random.Range(int min, int max): max exclusive
    pub fn range_i(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        min + (self.next() % (max - min) as u32) as i32
    }
    /// Random.Range(float min, float max): inclusive
    pub fn range_f(&mut self, min: f32, max: f32) -> f32 {
        min + (self.next() as f32 / u32::MAX as f32) * (max - min)
    }
}

/// Femur.ControlHandeler (+ _Human) fields.
#[derive(Clone, Debug)]
pub struct Control {
    pub direction: Vec3,
    pub raw_direction: Vec3,
    pub on_ground: bool,
    pub ground_check_delay: f32,
    pub on_moving_platform: bool,
    pub left_can_climb: bool,
    pub right_can_climb: bool,
    pub in_water: bool,
    pub swim_check_delay: f32,
    pub look_direction: Vec3,
    pub left_arm_override: bool,
    pub right_arm_override: bool,
    pub left_leg_override: bool,
    pub right_leg_override: bool,
    pub jump_delay: f32,
    pub lift: bool,
    pub lift_self: bool,
    pub jump: bool,
    pub grab_jump: bool,
    pub grab_jump_counter: f32,
    pub duck: bool,
    pub left_kick: bool,
    pub right_kick: bool,
    pub kick_duck: bool,
    pub back_flip: bool,
    pub revive_delay: f32,
    pub idle_timer: f32,
    pub run: bool,
    pub run_timer: f32,
    pub jump_timer: f32,
    pub fall_timer: f32,
    pub slide_direction: Vec3,
    pub can_dive: bool,
    pub headbutt: bool,
    pub headbutt_timer: f32,
    pub burning: bool,
    pub driving: bool,
    // ControlHandeler_Human
    pub duck_timer: f32,
    pub headbutt_delay: f32,
    pub valid_input: bool,
    pub idle_time_mod: f32,
    pub sit_delay: f32,
    pub action_delay: f32,
    pub grab_delay: f32,
    pub arm_action_delay: f32,
    pub leg_action_delay: f32,
    pub lift_timer: f32,
    pub leg_action_timer: f32,
    pub left_kick_timer: f32,
    pub right_kick_timer: f32,
    /// Per side (0 left, 1 right): ControlHandeler left/right Grab/Punch + _Human timers, and the
    /// BodyHandeler grab joint/rigidbody for that hand.
    pub hands: [Hand; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Hand {
    pub grab: bool,
    pub punch: bool,
    pub punch_timer: f32,
    pub arm_action_timer: f32,
    /// World runtime-joint handle (BodyHandeler.left/rightGrabJoint).
    pub joint: Option<usize>,
    /// BodyHandeler.left/rightGrabRigidbody (None while holding static geometry).
    pub rigidbody: Option<usize>,
    /// InteractableObjectData.partOfRagdoll of what is held.
    pub part_of_ragdoll: bool,
    pub grab_modifier: i32,
}

impl Default for Control {
    /// ControlHandeler_Human..ctor
    fn default() -> Self {
        Self {
            direction: Vec3::ZERO,
            raw_direction: Vec3::ZERO,
            on_ground: false,
            ground_check_delay: 0.0,
            on_moving_platform: false,
            left_can_climb: false,
            right_can_climb: false,
            in_water: false,
            swim_check_delay: 0.0,
            look_direction: Vec3::ZERO,
            left_arm_override: false,
            right_arm_override: false,
            left_leg_override: false,
            right_leg_override: false,
            jump_delay: 0.0,
            lift: false,
            lift_self: false,
            jump: false,
            grab_jump: false,
            grab_jump_counter: 0.0,
            duck: false,
            left_kick: false,
            right_kick: false,
            kick_duck: false,
            back_flip: false,
            revive_delay: 0.0,
            idle_timer: 0.0,
            run: false,
            run_timer: 0.0,
            jump_timer: 0.0,
            fall_timer: 0.0,
            slide_direction: Vec3::ZERO,
            can_dive: true,
            headbutt: false,
            headbutt_timer: 0.0,
            burning: false,
            driving: false,
            duck_timer: 0.0,
            headbutt_delay: 0.0,
            valid_input: false,
            idle_time_mod: 0.0,
            sit_delay: 0.5,
            action_delay: 0.0,
            grab_delay: 0.0,
            arm_action_delay: 0.0,
            hands: [Hand::default(); 2],
            leg_action_delay: 0.0,
            lift_timer: 0.0,
            leg_action_timer: 0.0,
            left_kick_timer: 0.0,
            right_kick_timer: 0.0,
        }
    }
}

/// MovementHandeler_HumanoidMediumEctomorph fields.
#[derive(Clone, Debug)]
pub struct Movement {
    pub state_change: bool,
    pub air_speed: f32,
    pub cycle_modifier: f32,
    pub cycle_timer: f32,
    pub cycle_speed: f32,
    pub left_leg_pose: i32,
    pub right_leg_pose: i32,
    pub left_arm_pose: i32,
    pub right_arm_pose: i32,
    pub run_force: f32,
}

impl Default for Movement {
    /// ..ctor
    fn default() -> Self {
        Self {
            state_change: true,
            air_speed: 4.0,
            cycle_modifier: 1.0,
            cycle_timer: 0.0,
            cycle_speed: 0.23,
            left_leg_pose: 2,
            right_leg_pose: 0,
            left_arm_pose: 0,
            right_arm_pose: 2,
            run_force: 0.0,
        }
    }
}

/// StatusHandeler (only what movement reads so far).
#[derive(Clone, Debug)]
pub struct Status {
    pub health: f32,
    pub stamina: f32,
    pub health_damage: f32,
    pub stamina_damage: f32,
    pub unconscious_time: f32,
    pub max_health: f32,
    pub max_stamina: f32,
    pub health_regeneration: f32,
    pub pain_threshold: f32,
    pub knockout_threshold: f32,
    /// Ramps 0 -> 1 after a knockout (damage taken is scaled by it).
    pub dmg_modifier: f32,
    pub max_unconscious_time: f32,
    pub min_unconscious_time: f32,
    pub always_drain: bool,
}

impl Default for Status {
    /// StatusHandeler prefab values / .ctor
    fn default() -> Self {
        Self {
            health: 100.0,
            stamina: 100.0,
            health_damage: 0.0,
            stamina_damage: 0.0,
            unconscious_time: 0.0,
            max_health: 100.0,
            max_stamina: 100.0,
            health_regeneration: 5.0,
            pain_threshold: 5.0,
            knockout_threshold: 40.0,
            dmg_modifier: 0.0,
            max_unconscious_time: 0.0,
            min_unconscious_time: 0.0,
            always_drain: false,
        }
    }
}

/// InteractableObjectData of whatever a collision/grab touched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Interactable {
    /// 0: can't be grabbed, 1: slips out, 2: 5/s stamina, 3: 20/s, other: 50/s.
    pub grab_modifier: i32,
    /// The kind of hit it deals (1 = none special).
    pub damage_modifier: i32,
    pub part_of_ragdoll: bool,
    pub always_drain: bool,
}

/// InteractableObjectData.priorityModifier of the InteractableObject on `node` or its nearest
/// ancestor (0 = Ignore / none: not in InteractableObjectManager.TargetsForActors).
pub fn priority_in(src: &gb_phys::Sidecar, mut node: usize) -> i32 {
    loop {
        let n = &src.nodes[node];
        if let Some(c) = n
            .components
            .iter()
            .find(|c| c.script.as_deref() == Some("InteractableObject"))
        {
            return c.data["interactableObjectData"]["priorityModifier"]
                .as_i64()
                .unwrap_or(0) as i32;
        }
        let Some(p) = n.parent else { return 0 };
        node = p;
    }
}

/// InteractableObject on `node` or its nearest ancestor (InteractableObjectManager lookup).
pub fn interactable_in(src: &gb_phys::Sidecar, mut node: usize) -> Option<Interactable> {
    loop {
        let n = &src.nodes[node];
        if let Some(c) = n
            .components
            .iter()
            .find(|c| c.script.as_deref() == Some("InteractableObject"))
        {
            let d = &c.data["interactableObjectData"];
            let i = |k: &str| d[k].as_i64().unwrap_or(0) as i32;
            let b = |k: &str| d[k].as_i64().unwrap_or(0) != 0 || d[k].as_bool().unwrap_or(false);
            return Some(Interactable {
                grab_modifier: i("grabModifier"),
                damage_modifier: i("damageModifier"),
                part_of_ragdoll: b("partOfRagdoll"),
                always_drain: b("alwaysDrain"),
            });
        }
        node = n.parent?;
    }
}

/// CollisionHandeler ground flags per part.
#[derive(Clone, Copy, Debug, Default)]
pub struct PartCollision {
    pub part_on_ground: bool,
    pub ground_check: bool,
    pub ground_handler: bool,
}

/// The current upper-body target selected by TargetingHandeler.FindTargets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PunchTarget {
    pub center: Vec3,
    pub axis: Vec3,
    pub half_segment: f32,
    pub radius: f32,
}

impl PunchTarget {
    /// Collider.ClosestPoint for the chest capsule used by the shipped beast prefab.
    pub fn closest_point(self, point: Vec3) -> Vec3 {
        let axis = self.axis.normalize_or_zero();
        let along = (point - self.center)
            .dot(axis)
            .clamp(-self.half_segment, self.half_segment);
        let axis_point = self.center + axis * along;
        let delta = point - axis_point;
        if delta.length_squared() <= self.radius * self.radius {
            point
        } else {
            axis_point + delta.normalize_or_zero() * self.radius
        }
    }
}

pub struct Actor {
    pub beast: Beast,
    pub state: u32,
    pub last_state: u32,
    pub applyed_force: f32,
    pub input_spam_force_modifier: f32,
    /// AIProfile `_punchForceModifer` (1 for humans / NormalAI; TinyAI 0.3, BigAI 10).
    pub punch_modifier: f32,
    pub control: Control,
    pub movement: Movement,
    pub status: Status,
    pub parts: [PartCollision; 21],
    pub rng: Random,
    /// InteractableObjectData.damageModifier per part: the kind of hit this part deals.
    pub interact: [i32; 21],
    /// Largest raw DamageCheck value seen and hit count (diagnostics).
    pub debug_max_hit: f32,
    pub debug_hits: u32,
    /// TargetingHandeler upperIntrest / lowerIntrest (World collider indices).
    pub upper_interest: Option<usize>,
    pub lower_interest: Option<usize>,
    /// Actor.timeSinceTarget: FindTargets runs every 0.5 s.
    time_since_target: f32,
    dt: f32,
}

impl Actor {
    pub fn new(beast: Beast, world: &World, seed: u32) -> Self {
        let mut parts = [PartCollision::default(); 21];
        for (k, p) in Part::ALL.iter().enumerate() {
            // From the prefab: CollisionHandeler.groundCheck and GroundCollisionHandler presence.
            let ground = matches!(p, Part::LeftLeg | Part::RightLeg | Part::Waist | Part::Ball);
            parts[k] = PartCollision {
                part_on_ground: false,
                ground_check: ground,
                ground_handler: ground,
            };
        }
        Self {
            beast,
            state: STAND,
            last_state: 0,
            applyed_force: 1.0,
            input_spam_force_modifier: 1.0,
            punch_modifier: 1.0,
            control: Control::default(),
            movement: Movement::default(),
            status: Status::default(),
            interact: [1; 21],
            debug_max_hit: 0.0,
            debug_hits: 0,
            upper_interest: None,
            lower_interest: None,
            time_since_target: 0.0,
            parts,
            rng: Random(seed.max(1)),
            dt: world.settings.fixed_timestep,
        }
    }

    fn body(&self, p: Part) -> usize {
        self.beast.body(p)
    }
    fn forward(&self, w: &World, p: Part) -> Vec3 {
        w.pose(self.body(p)).rotation * Vec3::Z
    }
    fn up(&self, w: &World, p: Part) -> Vec3 {
        w.pose(self.body(p)).rotation * Vec3::Y
    }
    fn align(&self, w: &mut World, p: Part, alignment: Vec3, target: Vec3, speed: f32) {
        align_to_vector(w, self.body(p), alignment, target, STABILITY, speed);
    }
    fn force(&self, w: &mut World, p: Part, f: Vec3) {
        w.add_force(self.body(p), f, VELOCITY_CHANGE);
    }
    fn part_index(p: Part) -> usize {
        Part::ALL.iter().position(|x| *x == p).unwrap()
    }

    // ---------------------------------------------------------------- Actor

    /// Actor.Update targeting tick (every 0.5 s while alive) -> TargetingHandeler.FindTargets
    /// (0x46E730, TargetingType 1 = TargetsForActors). `candidates`: (collider, priority) for every
    /// collider of an interactable whose priority isn't Ignore.
    pub fn update_targets(&mut self, w: &World, candidates: &[(usize, i32)]) {
        if self.state == DEAD || self.state == UNCONSCIOUS {
            return;
        }
        self.time_since_target += self.dt;
        if self.time_since_target < 0.5 {
            return;
        }
        self.time_since_target = 0.0;
        self.find_targets(w, candidates);
    }

    fn part_collider(&self, w: &World, p: Part) -> Option<usize> {
        let b = self.body(p);
        w.colliders
            .iter()
            .position(|c| c.body == Some(b) && !c.trigger)
    }

    /// TargetingHandeler.FindTargets. tUpper/cUpper = Head, tLower/cLower = Hips
    /// (Actor.SetupTargetingSphere), fov 90.
    pub fn find_targets(&mut self, w: &World, candidates: &[(usize, i32)]) {
        let fov = target_fov();
        let range_sq = target_range() * target_range();
        self.upper_interest = None;
        self.lower_interest = None;
        let (Some(cu), Some(cl)) = (
            self.part_collider(w, Part::Head),
            self.part_collider(w, Part::Hips),
        ) else {
            return;
        };
        let head = w.pose(self.body(Part::Head));
        let hips = w.pose(self.body(Part::Hips));
        let raw = self.control.raw_direction;
        let upper_center = w.collider_bounds(cu).0;
        let lower_center = w.collider_bounds(cl).0;
        let near_u = upper_center + raw * 0.5;
        let near_l = lower_center + raw * 0.5;
        // Candidates: closest point on bounds (to tUpper) within 2 m of cUpper (+ stick * 0.5).
        let own = self.beast.instance;
        let list: Vec<(usize, i32)> = candidates
            .iter()
            .copied()
            .filter(|&(c, _)| w.colliders[c].instance != own)
            .filter(|&(c, _)| {
                (w.closest_point_on_bounds(c, head.position) - near_u).length_squared() <= range_sq
            })
            .collect();
        // Vector3.Angle in degrees (0 for degenerate vectors).
        let angle = |a: Vec3, b: Vec3| {
            let d = (a.length_squared() * b.length_squared()).sqrt();
            if d < 1e-15 {
                0.0
            } else {
                (a.dot(b) / d).clamp(-1.0, 1.0).acos().to_degrees()
            }
        };
        let (up, right) = (head.rotation * Vec3::Y, head.rotation * Vec3::X);
        let (lg, rg) = (self.control.hands[0].grab, self.control.hands[1].grab);
        let reference = if self.state == CLIMB {
            head.position
        } else if lg == rg {
            head.position + up * 0.5
        } else if lg {
            head.position + up * 0.5 + right * 0.5
        } else {
            head.position + up * 0.5 - right * 0.5
        };
        let (mut best_u, mut best_l) = (f32::INFINITY, f32::INFINITY);
        for (c, priority) in list {
            let weight = match priority {
                2 => 0.25,
                3 => 0.5,
                4 => 1.0,
                5 => 1.5,
                6 => 10.0,
                _ => 0.0,
            };
            let p = w.closest_point(c, reference);
            let du = (p - near_u).length_squared();
            let dl = (p - near_l).length_squared();
            // The game measures against forward + position (sic).
            let ang_u = angle(p - upper_center, head.rotation * Vec3::Z + head.position);
            let ang_l = angle(p - lower_center, hips.rotation * Vec3::Z + hips.position);
            if ((du < 3.0 && ang_u <= fov) || (du < 1.5 && ang_u > fov))
                && head.position.y - 1.0 < p.y
                && p.y < head.position.y + 3.0
            {
                let score = du - weight + if ang_u > fov { 0.1 } else { 0.0 };
                if score < best_u {
                    best_u = score;
                    self.upper_interest = Some(c);
                }
            }
            if dl < 2.0 && hips.position.y - 1.0 < p.y && p.y < hips.position.y + 1.0 {
                let score = dl - weight + if ang_l > fov { 0.2 } else { 0.0 };
                if score < best_l {
                    best_l = score;
                    self.lower_interest = Some(c);
                }
            }
        }
    }

    /// Actor.FixedUpdate (0x4274F0)
    pub fn fixed_update(&mut self, w: &mut World, input: &InputState) {
        self.status_update(w);
        // Unity destroys a joint when it breaks; scripts then see the grab as gone.
        for side in 0..2 {
            if let Some(j) = self.control.hands[side].joint {
                if w.joint_broken(j) {
                    self.reset_grab(w, side);
                }
            }
        }
        self.get_input(w, input);
        self.update_state(w);
        // movementHandeler.Emote(): emotes are not ported yet.
    }

    /// Actor.UpdateState (0x427830)
    fn update_state(&mut self, w: &mut World) {
        self.movement.state_change = self.state != self.last_state;
        match self.state {
            STAND => self.stand(w),
            RUN => self.run(w),
            IDLE => self.idle(w),
            JUMP => self.jump(w),
            FALL => self.fall(w),
            // Not ported yet: the ragdoll goes limp in these states.
            UNCONSCIOUS => self.unconscious(),
            CLIMB => self.climb(w),
            DEAD | SWIM | DRIVE => {}
            _ => {}
        }
        self.last_state = self.state;
        let s = self.state;
        if s == DEAD || s == UNCONSCIOUS {
            self.applyed_force = 0.1;
            self.input_spam_force_modifier = 1.0;
        }
        if s == JUMP || s == FALL {
            self.applyed_force = 0.5;
            return;
        }
        self.applyed_force = (self.applyed_force + self.dt * 0.5).clamp(0.01, 1.0);
        self.input_spam_force_modifier =
            (self.input_spam_force_modifier + self.dt * 0.5).clamp(0.01, 1.0);
    }

    // ---------------------------------------------------------------- ControlHandeler

    /// ControlHandeler.GetInput (0x45B2E0), local-player path.
    fn get_input(&mut self, w: &mut World, i: &InputState) {
        if self.state == DEAD || self.state == UNCONSCIOUS {
            self.reset_variables();
            self.reset_grabs(w);
            self.revive_check(i);
            return;
        }
        self.direction_check(i);
        self.ground_check();
        // SwimCheck, LiftCheck: not ported yet.
        self.punch_grab_check(w, i);
        self.kick_stomp_check(w, i);
        self.duck_check(w, i);
        self.lift_check(w, i);
        self.fall_check();
        if self.control.driving {
            return;
        }
        if !self.control.on_ground {
            if self.control.left_can_climb || self.control.right_can_climb {
                self.climb_check(i);
                return;
            }
            if self.control.in_water {
                return;
            }
        } else {
            // vtable slots 16/9: IdleCheck then RunCheck, so moving overrides sitting.
            self.idle_check(i);
            self.run_check();
        }
        self.jump_run_check(i);
    }

    /// ControlHandeler_Human.DirectionCheck (0x460150); target interest (lookDirection toward a
    /// targeted collider) is not ported yet.
    fn direction_check(&mut self, i: &InputState) {
        let c = &mut self.control;
        c.raw_direction = Vec3::ZERO;
        let h = i.analogue(input::HORIZONTAL);
        let v = i.analogue(input::VERTICAL);
        if h != 0.0 || v != 0.0 || c.burning {
            let d = (Vec3::X * h + Vec3::Z * v).clamp_length_max(1.0);
            c.raw_direction = Vec3::new(d.x, 0.0, d.z);
            if c.raw_direction.length_squared() >= 9.9999994e-11 {
                let m = c.raw_direction.length();
                c.direction = if m > 1e-5 {
                    c.raw_direction / m
                } else {
                    Vec3::ZERO
                };
            }
        }
        if self.state == IDLE {
            return;
        }
        c.look_direction = c.direction + Vec3::new(0.0, 0.2, 0.0);
    }

    /// ControlHandeler_Human.GroundCheck (0x461CF0)
    fn ground_check(&mut self) {
        let on = [Part::LeftLeg, Part::RightLeg, Part::Ball, Part::Waist]
            .iter()
            .filter(|p| self.parts[Self::part_index(**p)].part_on_ground)
            .count();
        let dt = self.dt;
        let c = &mut self.control;
        if on == 0 {
            c.on_ground = false;
            return;
        }
        if c.ground_check_delay <= 0.0 {
            c.on_ground = true;
            c.back_flip = false;
            c.swim_check_delay = 0.2;
            if c.grab_jump_counter > 0.0 {
                c.grab_jump_counter -= dt + dt;
            }
        } else {
            c.on_ground = false;
            c.ground_check_delay -= dt;
            if c.grab_jump_counter > 0.0 {
                c.grab_jump_counter -= dt * 0.5;
            }
        }
    }

    /// ControlHandeler_Human.FallCheck (0x461BE0)
    fn fall_check(&mut self) {
        let c = &mut self.control;
        if c.on_ground {
            c.fall_timer = 0.0;
            return;
        }
        if c.fall_timer < 100.0 {
            c.fall_timer += self.dt;
        }
        if c.left_can_climb || c.right_can_climb {
            c.fall_timer = 0.0;
        }
        if self.state != FALL {
            if 0.1 < c.fall_timer && c.fall_timer < 0.6 {
                self.state = JUMP;
            }
            if 0.6 <= c.fall_timer {
                self.state = FALL;
            }
        }
    }

    /// ControlHandeler_Human.RunCheck (0x465770)
    fn run_check(&mut self) {
        let target = if self.state == JUMP {
            FALL
        } else if self.applyed_force <= 0.5
            || (self.control.raw_direction.x == 0.0 && self.control.raw_direction.z == 0.0)
        {
            if self.state == IDLE {
                return;
            }
            STAND
        } else {
            RUN
        };
        self.state = target;
    }

    fn grabbing(&self) -> (bool, bool) {
        (
            self.control.hands[0].joint.is_some(),
            self.control.hands[1].joint.is_some(),
        )
    }

    /// BodyHandeler.ResetLeftGrab / ResetRightGrab (0x4306E0)
    fn reset_grab(&mut self, w: &mut World, side: usize) {
        let h = &mut self.control.hands[side];
        if let Some(j) = h.joint.take() {
            w.remove_joint(j);
        }
        h.rigidbody = None;
        h.part_of_ragdoll = false;
    }

    /// Climb (0x4431A0): hang from a grab; stick swings the body, Jump (liftSelf) pulls up.
    fn climb(&mut self, w: &mut World) {
        let c = self.control.clone();
        let dir = c.direction;
        let air = self.movement.air_speed;
        let k = if c.lift_self {
            if c.duck {
                0.7
            } else {
                1.5
            }
        } else {
            0.6
        };
        let held = |side: usize| c.hands[side].joint.and(c.hands[side].rigidbody);
        if c.raw_direction.x != 0.0 || c.raw_direction.z != 0.0 {
            if let Some(rb) = held(0).or(held(1)) {
                w.add_force(rb, dir * (air * 4.0), IMPULSE);
            }
            self.force(w, Part::Chest, dir * 0.5 * air);
            self.force(w, Part::Waist, -dir * air);
            self.force(w, Part::Hips, dir * 0.5 * air);
            for p in [Part::Hips, Part::Waist, Part::Chest] {
                align_to_vector(w, self.body(p), -self.up(w, p), dir, 0.1, 6.0);
            }
        }
        let pull = if c.duck { Part::Hips } else { Part::Head };
        self.force(w, pull, Vec3::Y * (k * 6.0));
        if c.lift_self {
            // Left hand holding: right leg kicks up the wall; right hand: left leg.
            for (side, foot, thigh) in [
                (0, Part::RightFoot, Part::RightThigh),
                (1, Part::LeftFoot, Part::LeftThigh),
            ] {
                if c.hands[side].joint.is_none() {
                    continue;
                }
                self.force(w, foot, dir);
                self.force(w, thigh, -dir);
                if 5.0 < self.status.stamina {
                    let (part, up, down) = if c.duck {
                        (Part::Hips, 1.5, -32.0)
                    } else {
                        (Part::Head, 3.0, -24.0)
                    };
                    self.force(w, part, Vec3::Y * (k * up));
                    if let Some(rb) = c.hands[side].rigidbody {
                        w.add_force(rb, Vec3::Y * (k * down), IMPULSE);
                    }
                }
                let chest_fwd = self.forward(w, Part::Chest);
                align_to_vector(w, self.body(thigh), -self.up(w, thigh), chest_fwd, 0.1, 4.0);
            }
        }
        // Hanging from something kinematic (moving): lean into the stick direction.
        let kinematic = (0..2).any(|s| held(s).is_some_and(|rb| w.bodies[rb].kinematic));
        if kinematic && (c.raw_direction.x != 0.0 || c.raw_direction.z != 0.0) && !c.duck {
            self.force(w, Part::Chest, -dir);
            self.force(w, Part::Hips, dir);
            for p in [Part::Chest, Part::Hips] {
                align_to_vector(w, self.body(p), self.forward(w, p), dir, 0.1, 10.0);
            }
        }
    }

    /// ControlHandeler_Human.ClimbCheck (0x45FE90); DoubleJump (grab-jump off a ledge) not ported.
    fn climb_check(&mut self, i: &InputState) {
        let (lg, rg) = self.grabbing();
        let c = &mut self.control;
        if !lg && c.left_can_climb {
            c.left_can_climb = false;
        }
        if !rg && c.right_can_climb {
            c.right_can_climb = false;
        }
        if (lg && c.left_can_climb) || (rg && c.right_can_climb) {
            c.lift_self = i.digital(input::JUMP);
            self.state = CLIMB;
        }
    }

    /// ControlHandeler_Human.PunchGrabCheck (0x4640C0)
    fn punch_grab_check(&mut self, w: &mut World, i: &InputState) {
        if self.control.driving {
            return;
        }
        let dt = self.dt;
        let c = &mut self.control;
        if 0.0 < c.arm_action_delay {
            c.arm_action_delay -= dt;
        }
        if 0.0 < c.action_delay {
            c.action_delay -= dt;
        }
        if 0.0 < c.grab_delay {
            c.grab_delay -= dt;
        }
        self.punch_grab(w, i, 0);
        self.punch_grab(w, i, 1);
    }

    /// ControlHandeler_Human.PunchGrab (0x464220)
    fn punch_grab(&mut self, w: &mut World, i: &InputState, side: usize) {
        let dt = self.dt;
        let name = if side == 0 {
            input::GRAB_LEFT
        } else {
            input::GRAB_RIGHT
        };
        if i.just_down(name) {
            let h = &mut self.control.hands[side];
            h.arm_action_timer = 0.0;
            h.grab = false;
            if h.joint.is_some() {
                self.reset_grab(w, side);
            }
        } else if i.digital(name) {
            let h = &mut self.control.hands[side];
            if h.arm_action_timer < 1.0 {
                h.arm_action_timer += dt;
            }
            if h.arm_action_timer <= 0.2 || 0.0 < self.control.grab_delay {
                self.arm_action_readying(w, side);
            } else {
                self.control.hands[side].grab = true;
                self.arm_action_grabbing(w, side);
            }
        } else if i.just_up(name) {
            let c = &mut self.control;
            if c.hands[side].arm_action_timer <= 0.2 {
                if !c.hands[side].punch && c.arm_action_delay <= 0.0 && c.action_delay <= 0.0 {
                    c.hands[side].punch = true;
                    c.arm_action_delay = 0.4;
                    c.action_delay = 0.5;
                }
                c.hands[side].arm_action_timer = 0.0;
            } else {
                c.hands[side].grab = false;
                if c.hands[side].joint.is_some() {
                    if c.on_ground {
                        if let Some(rb) = c.hands[side].rigidbody {
                            // Throw what was held.
                            let v = w.linear_velocity(rb);
                            let f = if !c.hands[side].part_of_ragdoll {
                                (v * 0.5).clamp_length_max(35.0)
                            } else {
                                (v * 80.0).clamp_length_max(40.0)
                            } * lift_scale();
                            w.add_force(rb, f, VELOCITY_CHANGE);
                        }
                    }
                    self.control.grab_delay = 0.2;
                    self.reset_grab(w, side);
                }
            }
        }
        if self.control.hands[side].punch {
            let h = &mut self.control.hands[side];
            h.punch_timer += dt;
            let t = h.punch_timer;
            if t < 0.1 {
                self.arm_action_readying(w, side);
            }
            // TargetingHandeler.isBehindUpper (elbow) needs targeting: always punch for now.
            if 0.2 <= t && t < 0.3 {
                self.arm_action_punching(w, side);
            }
            if 0.3 <= t && t < 0.5 {
                self.arm_action_punch_resetting(w, side);
            }
            if 0.5 <= t {
                let h = &mut self.control.hands[side];
                h.punch = false;
                h.punch_timer = 0.0;
                self.control.grab_delay = 0.2;
            }
        }
        let h = self.control.hands[side];
        let over = h.punch || h.grab;
        if side == 0 {
            self.control.left_arm_override = over;
        } else {
            self.control.right_arm_override = over;
        }
    }

    fn arm_parts(side: usize) -> (Part, Part, Part) {
        if side == 0 {
            (Part::LeftArm, Part::LeftForarm, Part::LeftHand)
        } else {
            (Part::RightArm, Part::RightForarm, Part::RightHand)
        }
    }

    fn right(&self, w: &World, p: Part) -> Vec3 {
        w.pose(self.body(p)).rotation * Vec3::X
    }

    /// ArmActionReadying (0x43E7D0)
    fn arm_action_readying(&mut self, w: &mut World, side: usize) {
        let (arm, forearm, _) = Self::arm_parts(side);
        let chest_right = self.right(w, Part::Chest);
        let target = if side == 0 { chest_right } else { -chest_right };
        align_to_vector(w, self.body(arm), self.up(w, arm), target, 0.01, 20.0);
        let back = -self.forward(w, Part::Chest);
        align_to_vector(w, self.body(forearm), self.up(w, forearm), back, 0.01, 20.0);
    }

    /// ArmActionGrabbing (0x440540): reach while empty-handed. Target interest and Lift are not
    /// ported yet.
    fn arm_action_grabbing(&mut self, w: &mut World, side: usize) {
        let (arm, forearm, hand) = Self::arm_parts(side);
        let h = self.control.hands[side];
        if h.joint.is_some() || h.rigidbody.is_some() {
            // Lift: raise what's held (0x440540 tail).
            let (Some(_), Some(rb)) = (h.joint, h.rigidbody) else {
                return;
            };
            if !self.control.lift {
                return;
            }
            let mut f = ((w.mass(rb) / 800.0).sin() * 200.0).clamp(0.0, 25.0);
            if h.part_of_ragdoll {
                f = 80.0;
            }
            f *= lift_scale();
            let target = Vec3::Y + self.forward(w, Part::Chest) * 0.8;
            self.align(w, arm, -self.up(w, arm), target, 4.0);
            self.align(w, forearm, -self.up(w, forearm), target, 4.0);
            if !self.control.on_ground {
                return;
            }
            let f2 = f / 100.0 + f;
            w.add_force(self.body(Part::Ball), Vec3::NEG_Y * f2, 1);
            w.add_force(self.body(Part::Chest), Vec3::Y * f2, 1);
            w.add_force(rb, Vec3::Y * f, 1);
            w.add_force(self.body(arm), Vec3::NEG_Y * f, 1);
            return;
        }
        let ism = self.input_spam_force_modifier;
        if let Some(interest) = self.upper_interest {
            // Reach for the closest point of TargetingHandeler.upperIntrest.
            let hand_pos = w.pose(self.body(hand)).position;
            let mut v = (w.closest_point(interest, hand_pos) - hand_pos).normalize_or_zero();
            if self.state == CLIMB {
                v += Vec3::Y;
            }
            let v = v + self.control.raw_direction * 0.5;
            let d = w
                .collider_bounds(interest)
                .0
                .distance(w.pose(self.body(arm)).position);
            // 0x440c2c: the strong close-range pull also requires StatusHandeler.m_stamina > 10
            // (`comiss 10, [status + 0x54]; jae far`): an exhausted beast only gets the weak pull.
            // `target_pull` (dev panel) scales the whole reach so the arm locks on less hard.
            let pull = target_pull();
            let hand_f = (if d > 1.0 || self.status.stamina <= 10.0 {
                10.0
            } else if d > 0.8 {
                15.0
            } else {
                30.0
            }) * pull;
            self.force(w, arm, -(v * 10.0) * ism * pull);
            self.force(w, hand, v * hand_f * ism);
            return;
        }
        let dir = self.control.direction;
        if 45.0 <= dir.angle_between(self.forward(w, Part::Chest)).to_degrees() {
            return;
        }
        let f = if self.state == CLIMB {
            (dir + Vec3::Y * 0.5) * 5.0
        } else {
            dir * 5.0
        } * ism;
        self.force(w, arm, -f);
        self.force(w, hand, f);
    }

    /// ArmActionPunching (0x43EB10), no-target path.
    fn arm_action_punching(&mut self, w: &mut World, side: usize) {
        let (arm, forearm, hand) = Self::arm_parts(side);
        self.set_interact(hand, 3);
        // Source: the punching hand and forearm switch to ContinuousDynamic collision detection
        // (mode 2) for the punch, and back to Discrete (0) in ArmActionPunchResetting. Without
        // CCD the hand tunnels at punch speed and PhysX's depenetration throws the target.
        w.set_collision_detection(self.body(hand), 2);
        w.set_collision_detection(self.body(forearm), 2);
        let ism = self.input_spam_force_modifier;
        // Strike strength scale (`GB_PUNCH_SCALE`, in-game panel). The 30/40/50 forces below are
        // the ported source values; 1.0 = authored.
        let strength: f32 = punch_scale() * self.punch_modifier;
        let hand_position = w.pose(self.body(hand)).position;
        let (direction, hand_force) = if let Some(target) = self.upper_interest {
            let point = w.closest_point(target, hand_position);
            let arm_position = w.pose(self.body(arm)).position;
            let force = if w.collider_bounds(target).0.distance(arm_position) > 1.2 {
                40.0
            } else {
                50.0
            };
            ((point - hand_position).normalize_or_zero(), force)
        } else {
            let chest = w.pose(self.body(Part::Chest));
            let point = chest.position + chest.rotation * Vec3::Z + chest.rotation * Vec3::Y * 0.5;
            ((point - hand_position).normalize_or_zero(), 30.0)
        };
        self.force(w, arm, -(direction * 20.0) * ism * strength);
        self.force(w, hand, direction * hand_force * ism * strength);
    }

    /// ArmActionPunchResetting (0x441E80)
    fn arm_action_punch_resetting(&mut self, w: &mut World, side: usize) {
        let (arm, forearm, hand) = Self::arm_parts(side);
        // Source: back to Discrete collision detection once the punch resets.
        w.set_collision_detection(self.body(hand), 0);
        w.set_collision_detection(self.body(forearm), 0);
        for p in [arm, forearm, hand] {
            self.set_interact(p, 1);
        }
        let chest_right = self.right(w, Part::Chest) * 0.5;
        let s = if side == 0 { chest_right } else { -chest_right };
        let fwd = self.forward(w, Part::Chest);
        align_to_vector(w, self.body(arm), self.up(w, arm), s + fwd, 0.01, 2.0);
        align_to_vector(
            w,
            self.body(forearm),
            self.up(w, forearm),
            s - fwd,
            0.01,
            2.0,
        );
    }

    /// CollisionHandeler.GrabCheck + LeftGrab/RightGrab (0x43B2A0/0x43B860): a hand touching
    /// something while grabbing gets a fully locked ConfigurableJoint to it.
    fn grab_check(
        &mut self,
        w: &mut World,
        side: usize,
        other: Option<usize>,
        io: Option<Interactable>,
    ) {
        let h = self.control.hands[side];
        if !h.grab || 0.0 < self.control.grab_delay || h.joint.is_some() {
            return;
        }
        // Grabbable only with an interactable whose grabModifier isn't 0 (UpdateGrab drops a grab
        // with no interactable at once).
        let Some(io) = io else { return };
        if io.grab_modifier == 0 {
            return;
        }
        let (_, _, hand) = Self::arm_parts(side);
        // GameplayModifiers.grabStrengthMul (1) * 15000; CollisionHandeler projection distance 0.1.
        let data = serde_json::json!({
            "m_AngularXMotion": 0, "m_AngularYMotion": 0, "m_AngularZMotion": 0,
            "m_EnablePreprocessing": false, "m_EnableCollision": false,
            "m_ProjectionMode": 1, "m_ProjectionDistance": 0.1, "m_ProjectionAngle": 180.0,
            "m_BreakForce": 15000.0, "m_BreakTorque": 15000.0,
            "m_AutoConfigureConnectedAnchor": true, "m_RotationDriveMode": 0,
            "m_MassScale": 1.0, "m_ConnectedMassScale": 1.0,
        });
        let j = w.add_joint(self.body(hand), other, &data);
        let h = &mut self.control.hands[side];
        h.joint = Some(j);
        h.rigidbody = other;
        h.part_of_ragdoll = io.part_of_ragdoll;
        h.grab_modifier = io.grab_modifier;
        if side == 0 {
            self.control.left_can_climb = true;
        } else {
            self.control.right_can_climb = true;
        }
    }

    /// ControlHandeler_Human.JumpRunCheck (0x4624C0)
    fn jump_run_check(&mut self, i: &InputState) {
        let dt = self.dt;
        let (lg, rg) = self.grabbing();
        let c = &mut self.control;
        if c.jump_delay > 0.0 {
            c.jump_delay -= dt;
        }
        if i.just_down(input::JUMP) {
            c.jump_timer = 0.0;
        }
        if self.status.stamina <= 10.0
            || (self.last_state == STAND && c.raw_direction.length_squared() < 9.9999994e-11)
        {
            c.run = false;
        }
        let crouched = c.duck || c.kick_duck;
        let jump_held = (!crouched || (lg && rg)) && i.digital(input::JUMP);
        if jump_held {
            let t = c.jump_timer;
            if 0.4 < t && 10.0 <= self.status.stamina {
                c.slide_direction = c.direction;
                c.run = true;
                c.run_timer = 1.0;
            }
            c.jump_timer = t + dt;
        } else if 0.0 <= c.run_timer {
            c.run_timer -= dt;
        } else {
            c.run_timer = 0.0;
            c.run = false;
        }
        let airborne = self.state == JUMP || self.state == FALL;
        let mut to_bd1 = false;
        if !crouched || lg || rg {
            if i.just_up(input::JUMP) {
                c.back_flip = false;
                if !(0.8 < c.jump_timer || c.duck) {
                    if !c.kick_duck {
                        c.jump = true;
                        if c.jump_delay <= 0.0 && !airborne {
                            c.jump_delay = 0.8;
                            c.ground_check_delay = 0.1;
                            c.fall_timer -= 0.4;
                            self.state = JUMP;
                        }
                    } else {
                        to_bd1 = true;
                    }
                }
            } else {
                c.jump = false;
            }
        } else {
            c.jump = false;
        }
        if to_bd1 || c.kick_duck || c.duck {
            if self.state == JUMP || self.state == FALL {
                c.jump_delay = 0.8;
            }
        }
        let duck_or_kick = i.digital(input::DUCK) || i.digital(input::KICK);
        if duck_or_kick && self.state != JUMP && self.state != FALL {
            if i.just_up(input::JUMP) && c.jump_timer <= 0.4 {
                c.jump = true;
                c.run_timer = 10.0;
                if c.jump_delay <= 0.0 {
                    c.jump_delay = 0.4;
                    c.ground_check_delay = 0.1;
                    c.can_dive = false;
                    self.state = JUMP;
                }
            }
            if !c.kick_duck && !c.duck && !c.run && i.digital(input::JUMP) {
                c.jump = true;
                c.run_timer = 10.0;
                if c.jump_delay <= 0.0 {
                    c.jump_delay = 0.4;
                    c.ground_check_delay = 0.1;
                    c.can_dive = false;
                    self.state = JUMP;
                    if i.digital(input::DUCK) {
                        c.duck = true;
                    }
                    if i.digital(input::KICK) {
                        c.kick_duck = true;
                    }
                }
            }
        }
        if !duck_or_kick {
            c.can_dive = true;
        }
    }

    /// ControlHandeler_Human.IdleCheck (0x461E70)
    fn idle_check(&mut self, i: &InputState) {
        let dt = self.dt;
        if self.state == JUMP || self.state == FALL {
            return;
        }
        let c = &self.control;
        if c.left_kick || c.right_kick || c.kick_duck || c.headbutt || c.duck {
            return;
        }
        if !i.just_up(input::JUMP) && !i.any() {
            let c = &mut self.control;
            if c.idle_timer < 30.0 {
                c.idle_timer = (c.idle_timer + dt).clamp(-60.0, 30.0);
            }
            if 10.0 <= c.idle_timer && c.idle_timer < 20.0 && self.rng.range_i(1, 400) == 1 {
                self.random_look();
            }
            if self.control.idle_timer < 20.0 {
                return;
            }
            self.state = IDLE;
            if self.rng.range_i(1, 400) == 1 {
                self.random_look();
            }
            return;
        }
        let c = &mut self.control;
        if !i.digital(input::JUMP) {
            if self.state == IDLE {
                self.state = STAND;
                c.sit_delay = 0.5;
                c.idle_time_mod = self.rng.range_f(0.0, 3.0) + 1.0;
                c.idle_timer = c.idle_time_mod;
                return;
            }
        } else {
            c.sit_delay -= dt;
            if c.sit_delay <= 0.0 {
                self.state = IDLE;
            }
        }
        self.control.idle_timer = self.control.idle_time_mod;
    }

    fn random_look(&mut self) {
        let x = self.rng.range_f(-1.0, 1.0);
        let y = self.rng.range_f(-0.2, 1.0);
        let z = self.rng.range_f(-1.0, 1.0);
        self.control.look_direction = Vec3::new(x, y, z);
    }

    /// ControlHandeler_Human.DuckCheck (0x461500); head/hip damage modifiers are not ported yet.
    fn duck_check(&mut self, _w: &mut World, i: &InputState) {
        let dt = self.dt;
        let c = &mut self.control;
        if i.just_down(input::DUCK) {
            c.valid_input = true;
            c.duck_timer = 0.0;
        }
        if i.digital(input::DUCK) && c.valid_input {
            c.duck_timer += dt;
            if 0.2 <= c.duck_timer {
                c.duck = true;
            }
        }
        if i.just_up(input::DUCK) && c.valid_input {
            c.duck = false;
            if c.headbutt_delay <= 0.0 && c.duck_timer < 0.5 && c.action_delay <= 0.0 {
                c.headbutt = true;
                c.headbutt_delay = 1.0;
                c.action_delay = 0.5;
            }
            c.duck_timer = 0.0;
            c.valid_input = false;
        }
        if 0.0 <= c.headbutt_delay {
            c.headbutt_delay -= dt;
        }
        if c.headbutt {
            c.headbutt_timer += dt;
            let t = c.headbutt_timer;
            if t <= 0.2 {
                self.head_action(_w, false);
            } else if t <= 0.3 {
                self.head_action(_w, true);
            } else if 0.5 < t {
                self.head_action(_w, true);
                self.control.headbutt = false;
                self.control.headbutt_timer = 0.0;
            }
        }
        // Hit types of the head (duck = diving headbutt) and hips/thighs (kick-duck = drop kick).
        let airborne = self.state == JUMP || self.state == FALL;
        if !self.control.duck {
            if !self.control.headbutt {
                self.set_interact(Part::Head, 1);
            }
        } else if airborne {
            self.set_interact(Part::Head, 7);
        }
        let low = [Part::Hips, Part::LeftThigh, Part::RightThigh];
        for p in low {
            if !self.control.kick_duck {
                self.set_interact(p, 1);
            } else if airborne {
                self.set_interact(p, 5);
            }
            if !airborne && matches!(self.interact[Self::part_index(p)], 5 | 7) {
                self.set_interact(p, 1);
            }
        }
    }

    /// ControlHandeler_Human.LiftCheck (0x463C50)
    fn lift_check(&mut self, w: &mut World, i: &InputState) {
        let c = &mut self.control;
        if !i.digital(input::LIFT) {
            c.lift = false;
            if 0.0 < c.lift_timer {
                c.lift_timer -= self.dt;
            }
        } else {
            c.lift = true;
            c.lift_timer = 1.0;
        }
        if c.hands[0].joint.is_some() || c.hands[1].joint.is_some() {
            c.lift_timer = 0.0;
        }
        let c = &self.control;
        let busy = c.hands.iter().any(|h| h.punch || h.grab);
        if busy || !c.lift || c.lift_timer <= 0.0 {
            return;
        }
        self.control.left_arm_override = true;
        self.control.right_arm_override = true;
        self.arm_action_cheering(w);
    }

    /// ArmActionCheering (0x442AA0): both arms straight up over the head.
    fn arm_action_cheering(&mut self, w: &mut World) {
        let c = &self.control;
        let (lh, rh) = (c.hands[0], c.hands[1]);
        let dir = c.direction;
        if !lh.grab && !lh.punch {
            let t = -self.up(w, Part::Waist) + self.right(w, Part::Chest) * 0.5 - dir * 0.125;
            align_to_vector(
                w,
                self.body(Part::LeftArm),
                self.up(w, Part::LeftArm),
                t,
                0.01,
                8.0,
            );
            let t = -self.up(w, Part::Waist);
            align_to_vector(
                w,
                self.body(Part::LeftForarm),
                self.up(w, Part::LeftForarm),
                t,
                0.01,
                8.0,
            );
        }
        if rh.grab || rh.punch {
            return;
        }
        let t = -self.up(w, Part::Waist) - self.right(w, Part::Chest) * 0.5 - dir * 0.125;
        align_to_vector(
            w,
            self.body(Part::RightArm),
            self.up(w, Part::RightArm),
            t,
            0.01,
            8.0,
        );
        let t = -self.up(w, Part::Waist);
        align_to_vector(
            w,
            self.body(Part::RightForarm),
            self.up(w, Part::RightForarm),
            t,
            0.01,
            8.0,
        );
    }

    /// ControlHandeler_Human.ResetVariables (0x465390)
    fn reset_variables(&mut self) {
        let c = &mut self.control;
        c.left_arm_override = false;
        c.right_arm_override = false;
        c.left_leg_override = false;
        c.right_leg_override = false;
        c.jump_delay = 1.0;
        c.lift = false;
        c.lift_self = false;
        c.grab_jump = false;
        c.duck = false;
        c.kick_duck = false;
    }

    /// Entering a Kill Volume trigger (layer "Kill Volume"): StatusHandeler.Kill -> Dead; the
    /// group camera stops tracking dead beasts.
    pub fn kill(&mut self, w: &mut World) {
        if self.state == DEAD {
            return;
        }
        self.state = DEAD;
        self.status.health = 0.0;
        self.reset_grabs(w);
    }

    /// Respawn: alive again with full health (the game spawns a fresh beast).
    pub fn revive(&mut self) {
        self.state = FALL;
        self.last_state = FALL;
        self.status = Status::default();
    }

    fn reset_grabs(&mut self, w: &mut World) {
        for side in 0..2 {
            if self.control.hands[side].joint.is_some() {
                self.reset_grab(w, side);
            }
        }
    }

    /// ControlHandeler_Human.ReviveCheck (0x4654C0)
    fn revive_check(&mut self, i: &InputState) {
        let c = &mut self.control;
        c.revive_delay = (c.revive_delay - self.dt).clamp(0.0, 1.0);
        if 0.0 < c.revive_delay {
            return;
        }
        if input::DIGITAL.iter().any(|n| i.just_up(n)) {
            self.status.health_damage -= 5.0;
            self.status.unconscious_time -= 0.05;
            self.control.revive_delay = 0.5;
        }
    }

    // ---------------------------------------------------------------- MovementHandeler

    /// Stand (0x455F30)
    fn stand(&mut self, w: &mut World) {
        let af = self.applyed_force;
        let c = self.control.clone();
        if !c.duck {
            if c.kick_duck {
                for p in [Part::Chest, Part::Waist, Part::Hips] {
                    self.align(w, p, self.forward(w, p), Vec3::Y, af * 5.0);
                }
            } else {
                self.align(
                    w,
                    Part::Head,
                    self.forward(w, Part::Head),
                    c.look_direction,
                    af * 2.5,
                );
                self.align(w, Part::Head, self.up(w, Part::Head), Vec3::Y, af * 2.5);
                for p in [Part::Chest, Part::Waist] {
                    self.align(w, p, self.forward(w, p), c.direction, af * 4.0);
                    self.align(w, p, self.up(w, p), Vec3::Y, af * 4.0);
                }
                self.align(w, Part::Hips, self.up(w, Part::Hips), Vec3::Y, af * 3.0);
                if !c.left_kick {
                    self.align(
                        w,
                        Part::LeftThigh,
                        self.up(w, Part::LeftThigh),
                        Vec3::Y,
                        af * 3.0,
                    );
                    self.align(
                        w,
                        Part::LeftLeg,
                        self.up(w, Part::LeftLeg),
                        Vec3::Y,
                        af * 3.0,
                    );
                    self.force(w, Part::LeftFoot, -COUNTER_FORCE * af);
                }
                if !c.right_kick {
                    self.align(
                        w,
                        Part::RightThigh,
                        self.up(w, Part::RightThigh),
                        Vec3::Y,
                        af * 3.0,
                    );
                    self.align(
                        w,
                        Part::RightLeg,
                        self.up(w, Part::RightLeg),
                        Vec3::Y,
                        af * 3.0,
                    );
                    self.force(w, Part::RightFoot, -COUNTER_FORCE * af);
                }
                self.force(w, Part::Chest, COUNTER_FORCE * af);
            }
        } else {
            if !c.left_kick && !c.right_kick {
                self.force(w, Part::Hips, COUNTER_FORCE * af);
                self.force(w, Part::LeftFoot, -COUNTER_FORCE * af);
                self.force(w, Part::RightFoot, -COUNTER_FORCE * af);
            }
            for p in [Part::Chest, Part::Waist, Part::Hips] {
                self.align(w, p, self.forward(w, p), Vec3::NEG_Y, af * 5.0);
            }
        }
        w.set_angular_velocity(self.body(Part::Ball), Vec3::ZERO);
    }

    /// Run (0x44E720)
    fn run(&mut self, w: &mut World) {
        let m = &mut self.movement;
        m.cycle_speed = if self.control.run { 0.15 } else { 0.2 };
        if m.state_change {
            if self.rng.range_i(0, 2) == 1 {
                (
                    m.left_leg_pose,
                    m.right_leg_pose,
                    m.left_arm_pose,
                    m.right_arm_pose,
                ) = (0, 2, 2, 0);
            } else {
                (
                    m.left_leg_pose,
                    m.right_leg_pose,
                    m.left_arm_pose,
                    m.right_arm_pose,
                ) = (2, 0, 0, 2);
            }
            m.state_change = false;
        }
        if m.cycle_timer < m.cycle_speed {
            m.cycle_timer += self.dt;
        } else {
            let next = |p: i32| if p + 1 < 4 { p + 1 } else { 0 };
            m.left_arm_pose = next(m.left_arm_pose);
            m.right_arm_pose = next(m.right_arm_pose);
            m.left_leg_pose = next(m.left_leg_pose);
            m.right_leg_pose = next(m.right_leg_pose);
            m.cycle_timer = 0.0;
        }
        self.run_cycle_rotate_ball(w);
        self.run_cycle_pose_body(w);
        let c = self.control.clone();
        let m = self.movement.clone();
        if !c.duck && !c.kick_duck {
            if !c.left_arm_override {
                self.run_cycle_pose_arm(w, 0, m.left_arm_pose);
            }
            if !c.right_arm_override {
                self.run_cycle_pose_arm(w, 1, m.right_arm_pose);
            }
        }
        if !c.left_leg_override {
            self.run_cycle_pose_leg(w, 0, m.left_leg_pose);
        }
        if !c.right_leg_override {
            self.run_cycle_pose_leg(w, 1, m.right_leg_pose);
        }
    }

    /// RunCycleRotateBall (0x455550); the moving-platform push is not ported yet.
    fn run_cycle_rotate_ball(&mut self, w: &mut World) {
        let c = &self.control;
        let dir = c.direction;
        let hips_forward = self.forward(w, Part::Hips);
        let mut angle = hips_forward.angle_between(dir).to_degrees();
        if !(angle >= 0.01) {
            angle = 0.01;
        } else if 180.0 < angle {
            angle = 180.0;
        }
        let a = angle / 40.0;
        let axis = Vec3::new(dir.z / a, 0.0 / a, -dir.x / a);
        let crouched = c.duck || c.kick_duck;
        let (threshold, k) = match (c.run, crouched) {
            (false, false) => (5.0, 15.0),
            (true, false) => (5.0, 20.0),
            (_, true) => (2.5, 2.5),
        };
        let ball = self.body(Part::Ball);
        let modifier = self.movement.cycle_modifier;
        if w.linear_velocity(ball).length() < self.applyed_force * threshold * modifier {
            w.set_max_angular_velocity(ball, modifier * k);
            w.add_torque(ball, axis * k * modifier, VELOCITY_CHANGE);
        }
    }

    /// RunCyclePoseBody (0x44EAA0): upright path; the crouch/dive paths are not ported yet.
    fn run_cycle_pose_body(&mut self, w: &mut World) {
        let dt = self.dt;
        let m = &mut self.movement;
        m.run_force = if self.control.run {
            m.run_force + dt
        } else {
            m.run_force - dt
        }
        .clamp(0.0, 1.0);
        let c = self.control.clone();
        if c.duck {
            let dive = c.run && c.can_dive && 0.8 <= c.run_timer;
            if dive {
                // Diving (run + duck): Jump state with dive forces, not ported yet.
                return;
            }
            // Crouch walk ("scoot").
            self.applyed_force = 0.5;
            let af = self.applyed_force;
            let hu = self.up(w, Part::Hips);
            self.force(w, Part::Hips, (hu + RUN_VECTOR_FORCE_5) * af);
            self.force(w, Part::Ball, (-hu - RUN_VECTOR_FORCE_5) * af);
            self.force(w, Part::Chest, c.direction * 2.0);
            self.force(w, Part::Waist, -c.direction * 2.0);
            self.force(w, Part::Hips, (c.direction + Vec3::Y) * 4.0);
            self.force(w, Part::Ball, (Vec3::NEG_Y - c.direction) * 4.0);
            self.align(
                w,
                Part::Head,
                self.forward(w, Part::Head),
                c.look_direction,
                af * 4.0,
            );
            self.align(w, Part::Head, self.up(w, Part::Head), Vec3::Y, af * 4.0);
            return;
        }
        if c.kick_duck {
            if c.run && c.can_dive && 0.8 <= c.run_timer {
                return; // kick-duck dive: not ported yet
            }
            // Lying down (Kick held).
            self.applyed_force = 0.5;
            let af = self.applyed_force;
            for (leg, foot) in [
                (Part::LeftLeg, Part::LeftFoot),
                (Part::RightLeg, Part::RightFoot),
            ] {
                self.force(w, leg, Vec3::Y);
                self.force(w, foot, Vec3::NEG_Y);
            }
            self.force(w, Part::Chest, -c.direction * 2.0);
            self.force(w, Part::Waist, c.direction * 2.0);
            self.force(w, Part::Hips, (Vec3::Y * 0.5 - c.direction) * 4.0);
            self.force(w, Part::Ball, (Vec3::NEG_Y * 0.5 + c.direction) * 4.0);
            self.align(
                w,
                Part::Head,
                self.forward(w, Part::Head),
                c.look_direction,
                af * 4.0,
            );
            self.align(w, Part::Head, self.up(w, Part::Head), Vec3::Y, af * 4.0);
            return;
        }
        let af = self.applyed_force;
        let rf = self.movement.run_force;
        self.force(
            w,
            Part::Chest,
            (c.direction * rf + RUN_VECTOR_FORCE_10) * af,
        );
        self.force(
            w,
            Part::Hips,
            (-(c.direction * rf) - RUN_VECTOR_FORCE_5) * af,
        );
        self.force(w, Part::Ball, -RUN_VECTOR_FORCE_5 * af);
        self.align(
            w,
            Part::Head,
            self.forward(w, Part::Head),
            c.look_direction,
            af * 2.5,
        );
        self.align(w, Part::Head, self.up(w, Part::Head), Vec3::Y, af * 2.5);
        let lean = c.direction * 0.25 - Vec3::Y;
        self.align(w, Part::Chest, self.forward(w, Part::Chest), lean, af * 4.0);
        self.align(w, Part::Chest, self.up(w, Part::Chest), Vec3::Y, af * 8.0);
        self.align(w, Part::Waist, self.forward(w, Part::Waist), lean, af * 4.0);
        self.align(w, Part::Waist, self.up(w, Part::Chest), Vec3::Y, af * 8.0);
        self.align(
            w,
            Part::Hips,
            self.forward(w, Part::Hips),
            c.direction,
            af * 8.0,
        );
        self.align(w, Part::Hips, self.up(w, Part::Hips), Vec3::Y, af * 8.0);
    }

    /// Jump (0x4481E0): take-off impulse on entry, air control, tucked posture. The duck (dive)
    /// and backflip branches and target-aware grab delay are not ported yet.
    fn jump(&mut self, w: &mut World) {
        const JUMP_FORCE_MUL: f32 = 1.0; // GameplayModifiers.jumpForceMul
        let ism = self.input_spam_force_modifier;
        let s = ism * 90.0;
        self.applyed_force = 0.2;
        let mut f = ism * 0.6;
        let torso = [Part::Chest, Part::Waist, Part::Hips];
        if self.movement.state_change {
            self.movement.state_change = false;
            self.control.grab_delay = 0.25;
            f = ism * if self.control.run { 0.8 } else { 0.6 };
            if self.control.jump && 0.0 <= self.status.stamina {
                let dir = self.control.direction;
                let grab_jump = self.control.grab_jump;
                if grab_jump {
                    self.reset_grabs(w);
                }
                let push = if grab_jump { 2.0 } else { 1.0 };
                for p in torso {
                    self.force(w, p, dir * f * push);
                }
                if w.linear_velocity(self.body(Part::Hips)).y < 2.0 {
                    let upright = w.pose(self.body(Part::Hips)).position.y + 0.1
                        <= w.pose(self.body(Part::Chest)).position.y;
                    let k = match (grab_jump, upright) {
                        (false, true) => 1.0,
                        (false, false) => 0.5,
                        (true, true) => 1.5,
                        (true, false) => 1.0,
                    };
                    let up = Vec3::Y * (s * k / 3.0) * JUMP_FORCE_MUL;
                    for p in torso {
                        self.force(w, p, up);
                    }
                }
            }
            self.control.jump = false;
            self.control.grab_jump = false;
        }
        let c = self.control.clone();
        if c.raw_direction.x != 0.0 || c.raw_direction.z != 0.0 {
            for p in torso {
                self.force(w, p, c.direction * f);
            }
        }
        self.align(
            w,
            Part::Head,
            self.forward(w, Part::Head),
            c.look_direction,
            4.0,
        );
        self.align(w, Part::Head, self.up(w, Part::Head), Vec3::Y, 4.0);
        if c.duck {
            return; // dive: not ported yet
        }
        if c.kick_duck || c.back_flip {
            // Drop kick (Kick held in the air) / backflip (long Kick release in the air).
            if !c.run {
                for p in [Part::Chest, Part::Waist, Part::Hips] {
                    w.add_relative_torque(self.body(p), Vec3::NEG_X * 2.0, VELOCITY_CHANGE);
                }
            } else {
                self.force(w, Part::Chest, -c.slide_direction * 0.5);
                self.force(w, Part::Waist, c.slide_direction * 0.5);
                self.force(w, Part::Hips, c.slide_direction);
            }
            for p in [Part::Hips, Part::Waist, Part::Chest] {
                self.align(w, p, self.forward(w, p), Vec3::Y, 4.0);
            }
            let back = -self.forward(w, Part::Chest);
            for p in [
                Part::LeftThigh,
                Part::LeftLeg,
                Part::RightThigh,
                Part::RightLeg,
            ] {
                self.align(w, p, self.up(w, p), back, 4.0);
            }
            return;
        }
        self.force(w, Part::Chest, Vec3::Y * 2.0);
        self.force(w, Part::Hips, Vec3::NEG_Y * 2.0);
        self.align(
            w,
            Part::Chest,
            self.forward(w, Part::Chest),
            c.direction + Vec3::NEG_Y,
            10.0,
        );
        self.align(
            w,
            Part::Waist,
            self.forward(w, Part::Waist),
            c.direction,
            10.0,
        );
        self.align(
            w,
            Part::Hips,
            self.forward(w, Part::Hips),
            c.direction + Vec3::Y,
            10.0,
        );
        let (right, fwd) = (self.right(w, Part::Chest), self.forward(w, Part::Chest));
        if !c.left_arm_override {
            self.align(
                w,
                Part::LeftArm,
                self.up(w, Part::LeftArm),
                (right + fwd) * 0.25,
                4.0,
            );
            self.align(
                w,
                Part::LeftForarm,
                self.up(w, Part::LeftForarm),
                (right - fwd) * 0.25,
                4.0,
            );
        }
        if !c.right_arm_override {
            self.align(
                w,
                Part::RightArm,
                self.up(w, Part::RightArm),
                (-right + fwd) * 0.25,
                4.0,
            );
            self.align(
                w,
                Part::RightForarm,
                self.up(w, Part::RightForarm),
                (-right - fwd) * 0.25,
                4.0,
            );
        }
        let (hu, hf) = (self.up(w, Part::Hips), self.forward(w, Part::Hips));
        self.align(
            w,
            Part::LeftThigh,
            self.up(w, Part::LeftThigh),
            hu - hf,
            4.0,
        );
        self.align(w, Part::LeftLeg, self.up(w, Part::LeftLeg), hu + hf, 4.0);
        self.align(
            w,
            Part::RightThigh,
            self.up(w, Part::RightThigh),
            hu - hf,
            4.0,
        );
        self.align(w, Part::RightLeg, self.up(w, Part::RightLeg), hu + hf, 4.0);
    }

    /// Fall (0x446150): the dive/flail posture; the duck/kick-duck variants are not ported yet.
    fn fall(&mut self, w: &mut World) {
        let c = self.control.clone();
        self.align(w, Part::Head, self.forward(w, Part::Head), c.direction, 2.0);
        if !c.duck && !c.kick_duck {
            self.align(w, Part::Chest, self.up(w, Part::Chest), c.direction, 4.0);
            self.align(w, Part::Chest, -self.forward(w, Part::Chest), Vec3::Y, 4.0);
        }
        for p in [Part::Chest, Part::Waist, Part::Hips] {
            self.align(w, p, self.up(w, p), c.direction, 2.0);
            self.align(w, p, -self.forward(w, p), Vec3::Y, 2.0);
        }
        let (hu, hf) = (self.up(w, Part::Hips), self.forward(w, Part::Hips));
        self.align(w, Part::LeftThigh, self.up(w, Part::LeftThigh), hu, 2.0);
        self.align(w, Part::RightThigh, self.up(w, Part::RightThigh), hu, 2.0);
        self.align(w, Part::LeftLeg, self.up(w, Part::LeftLeg), hf, 2.0);
        self.align(w, Part::RightLeg, self.up(w, Part::RightLeg), hf, 2.0);
        if c.raw_direction.x != 0.0 || c.raw_direction.z != 0.0 {
            self.force(w, Part::Chest, c.direction * 0.2);
        }
    }

    fn leg_parts(side: usize) -> (Part, Part, Part) {
        if side == 0 {
            (Part::LeftThigh, Part::LeftLeg, Part::LeftFoot)
        } else {
            (Part::RightThigh, Part::RightLeg, Part::RightFoot)
        }
    }

    /// RunCyclePoseLeg (0x453770). Poses: 0 Bent, 1 Forward, 2 Straight, 3 Behind.
    fn run_cycle_pose_leg(&mut self, w: &mut World, side: usize, pose: i32) {
        let (thigh, leg, foot) = Self::leg_parts(side);
        let c = self.control.clone();
        let af = self.applyed_force;
        let dir = c.direction;
        let hu = self.up(w, Part::Hips);
        let (tu, lu) = (self.up(w, thigh), self.up(w, leg));
        let k = if c.run { 3.0 } else { 2.0 };
        match pose {
            0 => {
                self.align(w, thigh, -tu, dir, af * k);
                self.align(w, leg, lu, dir, af * k);
            }
            1 => {
                if !c.run {
                    self.align(w, thigh, -tu, dir - hu * 0.5, af * 4.0);
                    self.align(w, leg, -lu, dir - hu * 0.5, af * 4.0);
                    if !c.duck {
                        self.force(w, thigh, -dir * 0.5);
                        self.force(w, foot, dir * 0.5);
                    }
                } else {
                    self.align(w, thigh, -tu, dir, af * 3.0);
                    self.align(w, leg, -lu, dir, af * 3.0);
                    if !c.duck {
                        w.add_force(self.body(thigh), -dir * 2.0 * af, 0);
                        w.add_force(self.body(foot), dir * 2.0 * af, 0);
                    }
                }
            }
            2 => {
                self.align(w, thigh, tu, Vec3::Y, af * 2.0);
                self.align(w, leg, lu, Vec3::Y, af * 2.0);
                if !c.duck {
                    w.add_force(self.body(thigh), hu * af * 2.0, 0);
                    w.add_force(self.body(foot), -hu * 2.0 * af, 0);
                    self.force(w, foot, -RUN_VECTOR_FORCE_2);
                }
            }
            3 => {
                self.align(w, thigh, tu, dir * 2.0, af * k);
                self.align(w, leg, -lu, -dir * 2.0, af * k);
                if c.run {
                    self.force(w, Part::Hips, RUN_VECTOR_FORCE_2 * af);
                    self.force(w, Part::Ball, -RUN_VECTOR_FORCE_2 * af);
                    let back = if !c.duck {
                        -self.forward(w, Part::Hips)
                    } else {
                        -RUN_VECTOR_FORCE_2
                    };
                    self.force(w, foot, back);
                } else if c.duck {
                    self.force(w, Part::Hips, RUN_VECTOR_FORCE_2);
                    self.force(w, Part::Ball, -RUN_VECTOR_FORCE_2);
                    self.force(w, foot, -RUN_VECTOR_FORCE_2);
                }
            }
            _ => {}
        }
    }

    /// RunCyclePoseArm (0x452120)
    fn run_cycle_pose_arm(&mut self, w: &mut World, side: usize, pose: i32) {
        let (arm, fore, hand) = Self::arm_parts(side);
        let c = self.control.clone();
        let af = self.applyed_force;
        let dir = c.direction;
        let r = self.right(w, Part::Chest);
        let s = if side == 0 { r } else { -r };
        let cu = self.up(w, Part::Chest);
        let (au, fu) = (self.up(w, arm), self.up(w, fore));
        let pumping = c.duck || c.kick_duck || c.run;
        match pose {
            0 => {
                self.align(w, arm, au, s + cu, af * 4.0);
                if pumping {
                    self.align(w, fore, fu, -dir, af * 4.0);
                    self.force(w, arm, -dir);
                    self.force(w, hand, dir);
                } else {
                    self.align(w, fore, fu, -dir * 0.25, af * 4.0);
                }
            }
            1 => {
                self.align(w, arm, au, dir - s, af * 4.0);
                if pumping {
                    self.align(w, fore, fu, dir - s - cu, af * 4.0);
                    self.force(w, arm, Vec3::NEG_Y);
                    self.force(w, hand, Vec3::Y);
                } else {
                    self.align(w, fore, fu, -s - cu + dir * 0.25, af * 4.0);
                }
            }
            2 => {
                self.align(w, arm, au, s + cu, af * 4.0);
                self.align(w, fore, fu, cu, af * 4.0);
                if pumping {
                    self.force(w, arm, Vec3::Y);
                    self.force(w, fore, Vec3::NEG_Y);
                }
            }
            3 => {
                self.align(w, arm, au, dir, af * 4.0);
                self.align(w, fore, fu, cu, af * 4.0);
                if pumping {
                    self.force(w, arm, Vec3::NEG_Y);
                    self.force(w, hand, Vec3::Y);
                }
            }
            _ => {}
        }
    }

    /// ControlHandeler_Human.KickStompCheck (0x462E30); kick targets (lowerIntrest) not ported,
    /// so the kicking leg is random like the untargeted original.
    fn kick_stomp_check(&mut self, w: &mut World, i: &InputState) {
        let dt = self.dt;
        let airborne = self.state == JUMP || self.state == FALL;
        let upright = w.pose(self.body(Part::Hips)).position.y - 0.5
            < w.pose(self.body(Part::Head)).position.y;
        let c = &mut self.control;
        if 0.0 < c.leg_action_delay {
            c.leg_action_delay -= dt;
        }
        if i.digital(input::KICK) {
            c.leg_action_timer += dt;
            let threshold = if airborne || self.state == CLIMB {
                0.2
            } else {
                0.5
            };
            if threshold <= c.leg_action_timer {
                c.kick_duck = true;
            }
        }
        if i.just_up(input::KICK) {
            if 0.5 <= c.leg_action_timer || 0.0 < c.action_delay {
                if airborne && upright {
                    c.back_flip = true;
                }
            } else if upright && c.leg_action_delay <= 0.0 {
                c.action_delay = 0.5;
                c.leg_action_delay = 0.6;
                if airborne {
                    c.left_kick = true;
                    c.right_kick = true;
                } else if self.rng.range_f(0.0, 1.0) < self.rng.range_f(0.0, 1.0) {
                    self.control.left_kick = true;
                } else {
                    self.control.right_kick = true;
                }
            }
            self.control.kick_duck = false;
            self.control.leg_action_timer = 0.0;
        }
        for side in 0..2 {
            let active = if side == 0 {
                self.control.left_kick
            } else {
                self.control.right_kick
            };
            if !active {
                continue;
            }
            let t = if side == 0 {
                &mut self.control.left_kick_timer
            } else {
                &mut self.control.right_kick_timer
            };
            *t += dt;
            let t = *t;
            if t < 0.2 {
                self.leg_action_readying(w, side);
            }
            if 0.2 <= t && t < 0.3 {
                self.leg_action_kicking(w, side);
            }
            if 0.3 <= t && t < 0.5 {
                // LegActionResetting (0x44E640)
                let (thigh, leg, _) = Self::leg_parts(side);
                self.set_interact(leg, 1);
                self.set_interact(thigh, 1);
            }
            if 0.5 <= t {
                if side == 0 {
                    self.control.left_kick = false;
                    self.control.left_kick_timer = 0.0;
                } else {
                    self.control.right_kick = false;
                    self.control.right_kick_timer = 0.0;
                }
            }
        }
        self.control.left_leg_override = self.control.left_kick;
        self.control.right_leg_override = self.control.right_kick;
    }

    /// LegActionReadying (0x44CD90)
    fn leg_action_readying(&mut self, w: &mut World, side: usize) {
        let (thigh, leg, _) = Self::leg_parts(side);
        let back = -self.forward(w, Part::Hips);
        align_to_vector(w, self.body(thigh), self.up(w, thigh), back, 0.01, 10.0);
        align_to_vector(w, self.body(leg), self.up(w, leg), back, 0.01, 10.0);
    }

    /// LegActionKicking (0x44D020), untargeted path. The original's two AlignToVector calls
    /// target -Vector3.zero (no torque), so only the forces act.
    fn leg_action_kicking(&mut self, w: &mut World, side: usize) {
        let (thigh, leg, foot) = Self::leg_parts(side);
        self.set_interact(leg, 4);
        self.set_interact(thigh, 4);
        let fwd = self.forward(w, Part::Chest);
        let ism = self.input_spam_force_modifier;
        let both = self.control.left_kick && self.control.right_kick;
        self.force(w, thigh, -(fwd * 20.0) * ism);
        self.force(w, foot, fwd * if both { 25.0 } else { 30.0 } * ism);
    }

    /// HeadActionReadying (0x43CF80) / HeadActionHeadbutting (0x43DBC0), untargeted path: aim at a
    /// point ahead of the chest; wind up (chest in, head back), then strike (chest back, head in).
    fn head_action(&mut self, w: &mut World, strike: bool) {
        let chest = w.pose(self.body(Part::Chest));
        let target = chest.position + chest.rotation * Vec3::Z + chest.rotation * Vec3::Y * 0.5;
        let d = (target - w.pose(self.body(Part::Head)).position).normalize_or_zero();
        let ism = self.input_spam_force_modifier;
        if strike {
            self.set_interact(Part::Head, 6);
            self.force(w, Part::Chest, -(d * 20.0) * ism);
            self.force(w, Part::Head, d * 30.0 * ism);
        } else {
            self.force(w, Part::Chest, d * 8.0 * ism);
            self.force(w, Part::Head, -(d * 10.0) * ism);
        }
    }

    fn set_interact(&mut self, p: Part, v: i32) {
        self.interact[Self::part_index(p)] = v;
    }

    /// Unconscious (0x4593D0): limp; every part stops dealing special hits.
    fn unconscious(&mut self) {
        if self.movement.state_change {
            self.interact = [1; 21];
        }
    }

    /// StatusHandeler.Update (0x46B500) with UpdateGrab / UpdateHealth / UpdateStamina.
    fn status_update(&mut self, w: &mut World) {
        let dt = self.dt;
        if self.state == DEAD {
            return;
        }
        let st = &mut self.status;
        st.max_unconscious_time = (st.max_unconscious_time - dt / 20.0)
            .max(st.min_unconscious_time)
            .min(20.0);
        st.dmg_modifier = (st.dmg_modifier + dt * 0.25).clamp(0.0, 1.0);
        // UpdateGrab (0x46CA10): holding on drains stamina by grabModifier; empty -> let go.
        let (lg, rg) = self.grabbing();
        self.status.always_drain = false;
        let mut rate = 0.0f32;
        for side in 0..2 {
            let h = self.control.hands[side];
            if h.joint.is_none() {
                continue;
            }
            if h.grab_modifier == 1 {
                self.reset_grab(w, side); // InvokeDelayed ResetGrab 0.05 s
                continue;
            }
            self.status.always_drain = true;
            rate = rate.max(match h.grab_modifier {
                2 => 5.0,
                3 => 20.0,
                _ => 50.0,
            });
            if self.status.stamina <= 0.0 {
                self.status.stamina = -20.0;
                self.reset_grab(w, side);
                self.control.grab_delay = self.rng.range_f(0.0, 0.1);
            }
        }
        if lg || rg {
            self.status.stamina_damage += dt * rate;
        }
        self.update_health(w);
        // UpdateStamina
        let st = &mut self.status;
        let mut s = st.stamina;
        if !st.always_drain && s < st.max_stamina {
            s += dt * 80.0;
        }
        if 0.0 < s {
            s -= st.stamina_damage * 0.8;
        }
        st.stamina = s.clamp(-100.0, st.max_stamina);
        st.stamina_damage = 0.0;
        // Unconscious countdown
        if self.state != UNCONSCIOUS && 0.0 < self.status.unconscious_time {
            self.status.unconscious_time =
                (self.status.unconscious_time - dt).clamp(0.0, self.status.max_unconscious_time);
        }
        if self.state == UNCONSCIOUS {
            let st = &mut self.status;
            st.unconscious_time = (st.unconscious_time - dt).clamp(0.0, st.max_unconscious_time);
            if st.unconscious_time <= 0.0 {
                self.state = FALL;
                st.unconscious_time = 0.0;
                st.health = st.max_health;
                st.dmg_modifier = 0.0;
            }
        }
    }

    /// StatusHandeler.UpdateHealth (0x46BA40); death (canDie) is off in the default rules.
    fn update_health(&mut self, w: &mut World) {
        let dt = self.dt;
        let knocked = self.state == UNCONSCIOUS;
        let st = &mut self.status;
        let mut h = st.health;
        if h < st.max_health {
            h += dt * st.health_regeneration;
        }
        if 0.0 < h {
            h -= if knocked {
                st.health_damage * 0.25
            } else {
                st.health_damage * st.dmg_modifier
            };
        }
        let before = st.health;
        let mut ko = false;
        if !knocked && st.pain_threshold <= before - h && st.knockout_threshold <= before - h {
            ko = true;
        }
        if h <= 0.0 {
            ko = !knocked && self.last_state != UNCONSCIOUS && self.last_state != DEAD;
            h = 0.0;
        }
        if ko {
            let st = &mut self.status;
            st.max_unconscious_time = (st.max_unconscious_time + 1.5)
                .max(st.min_unconscious_time)
                .min(20.0);
            st.unconscious_time = st.max_unconscious_time;
            self.state = UNCONSCIOUS;
            self.reset_grabs(w);
        }
        let st = &mut self.status;
        st.health = h.min(st.max_health);
        st.health_damage = 0.0;
    }

    /// CollisionHandeler.DamageCheck (0x437F80) for one OnCollisionEnter on `part`.
    /// `other_mod`: the hitter's InteractableObjectData.damageModifier; `other_mass`: its
    /// Rigidbody mass (None = static, counted as 40).
    fn damage_check(
        &mut self,
        w: &mut World,
        part: usize,
        other_mod: i32,
        other_mass: Option<f32>,
        other_body: Option<usize>,
        points: &[gb_phys::ContactPoint],
        rel: Vec3,
    ) {
        let settings = self.beast.settings[part];
        let body = self.beast.body(Part::ALL[part]);
        let holding = self
            .control
            .hands
            .iter()
            .any(|h| h.rigidbody.is_some() && h.rigidbody == other_body);
        for p in points {
            let n = p.normal;
            let m = other_mass.unwrap_or(40.0).clamp(0.0, 40.0);
            let mut dmg = (m * rel.dot(n) / 1100.0).abs() * settings.damage_modifier;
            let airborne = self.state == JUMP || self.state == FALL;
            if airborne && self.control.kick_duck && self.control.run {
                dmg *= 4.0;
            }
            if airborne && self.control.duck && self.control.run {
                dmg += dmg;
            }
            if self.state == FALL {
                dmg *= 4.0;
            } else if self.state == JUMP {
                dmg += dmg;
            }
            if !holding {
                // Per-hit-type damage scale (only in the disassembly; Ghidra drops the SSE math).
                // GameplayModifiers.punchStrengthMul and Actor._punchDamageModifer are 1.
                dmg *= match other_mod {
                    0 => 0.0,
                    2 => 20.0,
                    3 => 70.0,
                    4 | 5 => 140.0,
                    6 => 80.0,
                    7 => 200.0,
                    8 => 1.0e6,
                    9 | 10 | 12 => 2.0,
                    13 => 40.0,
                    _ => 1.0,
                };
                let up = Vec3::Y;
                let kick = match other_mod {
                    0 | 1 => None,
                    2 => Some(n * 5.0),
                    3 => Some(up * 10.0 + n * 25.0),
                    4 => Some(n * 20.0 + up * 10.0),
                    5 => {
                        self.applyed_force = 0.5;
                        Some(n * 25.0 + up * 10.0)
                    }
                    6 => {
                        self.applyed_force = 0.5;
                        Some(n * 20.0 + up * 10.0)
                    }
                    7 => {
                        self.applyed_force = 0.5;
                        Some(n * 30.0 + up * 10.0)
                    }
                    9 => {
                        self.applyed_force = 0.5;
                        Some((Vec3::X * 40.0 + n).clamp_length_max(20.0))
                    }
                    10 => {
                        self.applyed_force = 0.5;
                        Some((-Vec3::Z * 20.0 + n * 20.0).clamp_length_max(20.0))
                    }
                    13 => Some(up * 10.0 + n * 30.0),
                    _ => Some(n * 10.0),
                };
                if let Some(v) = kick {
                    w.add_force(body, v, VELOCITY_CHANGE);
                }
            }
            self.debug_max_hit = self.debug_max_hit.max(dmg);
            self.debug_hits += 1;
            let amount = dmg.round();
            if 0.0 < amount && settings.damage_minimum_velocity < rel.length() {
                self.add_damage(amount);
            }
        }
    }

    /// StatusHandeler.AddDamage (0x46B390): _damageModifer 1, GameplayModifiers.damageTakenMul 1.
    fn add_damage(&mut self, amount: f32) {
        if self.state == DEAD || self.state == UNCONSCIOUS {
            return;
        }
        self.status.health_damage += amount;
    }

    /// Idle (0x446F40)
    fn idle(&mut self, w: &mut World) {
        let (lg, rg) = self.grabbing();
        if !lg && !rg {
            let c = self.control.clone();
            let af = self.applyed_force;
            if c.idle_timer <= 25.0 {
                self.align(
                    w,
                    Part::Head,
                    self.forward(w, Part::Head),
                    c.look_direction,
                    af * 2.5,
                );
                self.align(w, Part::Head, self.up(w, Part::Head), Vec3::Y, af * 2.5);
            }
            if c.idle_timer < 30.0 {
                self.align(
                    w,
                    Part::Chest,
                    self.forward(w, Part::Chest),
                    c.direction + Vec3::NEG_Y,
                    5.0,
                );
                self.align(
                    w,
                    Part::Waist,
                    self.forward(w, Part::Waist),
                    c.direction,
                    5.0,
                );
                self.align(
                    w,
                    Part::Hips,
                    self.forward(w, Part::Hips),
                    c.direction + Vec3::Y,
                    5.0,
                );
                let tuck = self.up(w, Part::Hips) - self.forward(w, Part::Hips);
                if !c.left_kick {
                    self.align(w, Part::LeftThigh, self.up(w, Part::LeftThigh), tuck, 8.0);
                    self.align(w, Part::LeftLeg, self.up(w, Part::LeftLeg), tuck, 8.0);
                }
                if !c.right_kick {
                    self.align(w, Part::RightThigh, self.up(w, Part::RightThigh), tuck, 8.0);
                    self.align(w, Part::RightLeg, self.up(w, Part::RightLeg), tuck, 8.0);
                }
                self.force(w, Part::Chest, Vec3::Y * 2.0);
                self.force(w, Part::Hips, Vec3::NEG_Y * 2.0);
            } else {
                if self.rng.range_i(1, 1000) == 1 {
                    let r = Vec3::new(
                        self.rng.range_i(-4, 4) as f32,
                        self.rng.range_i(-4, 4) as f32,
                        self.rng.range_i(-4, 4) as f32,
                    );
                    self.force(w, Part::Chest, r);
                }
                let waist_up = self.up(w, Part::Waist);
                self.align(w, Part::Chest, self.up(w, Part::Chest), waist_up, 4.0);
                self.align(w, Part::Hips, waist_up, self.up(w, Part::Hips), 4.0);
                self.align(w, Part::Hips, self.up(w, Part::Hips), waist_up, 4.0);
            }
        }
        w.set_angular_velocity(self.body(Part::Ball), Vec3::ZERO);
    }

    // ---------------------------------------------------------------- collisions

    /// CollisionHandeler / GroundCollisionHandler OnCollisionEnter/Stay/Exit for this beast's parts.
    /// `beasts`: physics instances that are beasts (for InteractableObjectData.partOfRagdoll).
    /// `interact`: InteractableObjectData.damageModifier of every world body (by body index;
    /// statics count as 1). `beasts`: physics instances that are beasts.
    pub fn after_step(&mut self, w: &mut World, lookup: &dyn Fn(ActorRef) -> Option<Interactable>) {
        // CollisionHandeler.OnCollisionEnter -> DamageCheck on damage-checked parts.
        let mut hits = vec![];
        for contact in &w.contacts {
            let this = w.actors[contact.this];
            let other = w.actors[contact.other];
            if contact.kind != ContactKind::Enter
                || this.instance != self.beast.instance
                || other.instance == self.beast.instance
            {
                continue;
            }
            let Some(body) = this.body else { continue };
            let Some(k) = Part::ALL.iter().position(|p| self.beast.body(*p) == body) else {
                continue;
            };
            if !self.beast.settings[k].damage_check {
                continue;
            }
            // OnCollisionEnter returns early when the other object isn't interactable.
            let Some(io) = lookup(other) else { continue };
            let other_mod = io.damage_modifier;
            let mass = other.body.map(|b| w.mass(b));
            hits.push((
                k,
                other_mod,
                mass,
                other.body,
                contact.points.clone(),
                contact.relative_velocity,
            ));
        }
        for (k, m, mass, ob, pts, rel) in hits {
            self.damage_check(w, k, m, mass, ob, &pts, rel);
        }
        let hands = [self.body(Part::LeftHand), self.body(Part::RightHand)];
        let mut grabs = vec![];
        for contact in &w.contacts {
            let this = w.actors[contact.this];
            let other = w.actors[contact.other];
            if this.instance != self.beast.instance || other.instance == self.beast.instance {
                continue;
            }
            if contact.kind == ContactKind::Exit {
                continue;
            }
            if let Some(side) = hands.iter().position(|h| Some(*h) == this.body) {
                grabs.push((side, other.body, lookup(other)));
            }
        }
        for (side, other, io) in grabs {
            self.grab_check(w, side, other, io);
        }
        self.ground_contacts(w);
    }

    fn ground_contacts(&mut self, w: &World) {
        for contact in &w.contacts {
            let this = w.actors[contact.this];
            let other = w.actors[contact.other];
            if this.instance != self.beast.instance || other.instance == self.beast.instance {
                continue;
            }
            let Some(body) = this.body else { continue };
            let Some(k) = Part::ALL.iter().position(|p| self.beast.body(*p) == body) else {
                continue;
            };
            let pc = &mut self.parts[k];
            let grounded = || {
                // CollisionHandeler.GroundCheck: the actor-layer mask quirk leaves the 50° limit.
                contact
                    .points
                    .iter()
                    .any(|p| p.normal.angle_between(Vec3::Y).to_degrees() <= 50.0)
            };
            match contact.kind {
                ContactKind::Enter => {
                    if pc.ground_check && grounded() {
                        pc.part_on_ground = true;
                    }
                }
                ContactKind::Stay => {
                    if pc.ground_handler && grounded() {
                        pc.part_on_ground = true;
                    }
                }
                ContactKind::Exit => {
                    if pc.ground_check || pc.ground_handler {
                        pc.part_on_ground = false;
                    }
                }
            }
        }
    }
}
