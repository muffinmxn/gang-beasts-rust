//! Stage events driven by scene scripts that the exporter only records as data.
//!
//! * Inactive GameObjects: Unity never draws a node that is inactive (nor its children). The
//!   exporter keeps their active children's meshes, so every pooled prop used to sit visible in the
//!   level (the subway's trains were parked in the middle of the map). They start hidden here.
//! * Subway trains (`PoolSpawner` + `SyncPool` + `Train` + `TrainPush` + `PoolSpawnTrigger` +
//!   `TrainDespawnZone`): decompiled in `re/decomp/Train*.c` and `CoreNet.Pooling.*`.
//!   - `PoolSpawner` activates a pooled item after a random `DelayMin..DelayMax` and
//!     `PoolSpawnTrigger` volumes (an Actor standing inside, re-checked every `RepeatTime`) spawn
//!     one as soon as `DelayMin` has passed.
//!   - `Train.FixedUpdate` (0x5506A0): a kinematic body `MovePosition(pos + moveAmount * dt)`.
//!   - `TrainPush.OnTriggerStay` (0x7915D0): every rigidbody in the trigger gets
//!     `AddForce(direction * force)`.
//!   - `TrainDespawnZone.OnTriggerEnter` (0x550A30): a Train entering repools itself.
use super::*;
use gb_phys::source::{mirror_position, mirror_rotation, vec3};

struct Train {
    node: usize,
    entity: Entity,
    /// Train.moveAmount, Unity space, metres per second.
    velocity: Vec3,
    /// Root BoxCollider in Unity space, relative to the root: centre and half extents.
    box_center: Vec3,
    box_half: Vec3,
    layer: u32,
    body: Option<usize>,
    live: bool,
    /// (entity, direction * 1, force, centre, half) of the TrainPush triggers under this train.
    pushes: Vec<Push>,
    spawner: usize,
}

struct Push {
    entity: Entity,
    direction: Vec3,
    force: f32,
    center: Vec3,
    half: Vec3,
}

struct Spawner {
    entity: Entity,
    pool: Vec<usize>,
    delay_min: f32,
    delay_max: f32,
    since: f32,
    next: f32,
}

struct Trigger {
    entity: Entity,
    spawner: usize,
    repeat: f32,
    timer: f32,
    center: Vec3,
    half: Vec3,
}

struct Zone {
    entity: Entity,
    center: Vec3,
    half: Vec3,
}

/// `WheelRotator` (0x7B8D00 / 0x7B8DA0): the Ferris wheel axle is a kinematic body spun about its
/// local Z; every 5-20 s it re-rolls a `wheelState` (0..100) that picks a target speed.
struct Wheel {
    body: usize,
    pose: Iso,
    speed: f32,
    state: f32,
    timer: f32,
    max_speed: f32,
}

/// Train stage escalation (`TrackPool.startEscalationRange` 30-120 s, `escalationDurationRange` 120 s): a landslide
/// rains boulders from `landSlideBoulderSpawns` onto the cliff track.
struct Boulders {
    spawns: Vec<Entity>,
    items: Vec<PropItem>,
    start: f32,
    end: f32,
    next: f32,
}

/// `SharkActor` states (decomp: FixedUpdate switch on `currentState`).
#[derive(Clone, Copy, PartialEq, Debug)]
enum SharkState {
    Sleeping,
    Searching,
    Attacking,
    Breaching,
    Carrying,
    Retreating,
    Diving,
}

/// `SharkActor` (ported state machine, rigid-body motion): sleeps `startDelay` + `sleepTime`, then loops
/// Searching (swim the `searchNodes`) -> Attacking (chase a beast in the water) or Breaching (leap from `breachDepth`
/// to `breachHeight` at a beast on land) -> Carrying (mouth-held beast taken to the `retreatNodes`) -> Diving
/// (`diveNodes`) -> Retreating -> Searching. The 12-rigidbody ragdoll is driven as one kinematic body.
struct Shark {
    bodies: Vec<(usize, Iso)>,
    centre: Vec3,
    fwd0: Vec3,
    mouth: f32,
    search: Vec<Vec3>,
    retreat: Vec<Vec3>,
    dive: Vec<Vec3>,
    sleep: (f32, f32),
    searching: (f32, f32),
    attacking: (f32, f32),
    breach_depth: f32,
    breach_height: f32,
    speed: f32,
    state: SharkState,
    timer: f32,
    state_len: f32,
    pos: Vec3,
    dir: Vec3,
    node: usize,
    target: Option<usize>,
    held: Vec<(usize, Vec3)>,
    arc: Option<Vec3>,
    started: bool,
    start_at: f32,
    health: f32,
    max_health: f32,
    /// The jaw rigidbody and its hinge (the jaw node's pivot) for the open / close animation.
    jaw: Option<(usize, Vec3)>,
    jaw_open: f32,
}

impl Shark {
    fn placeholder() -> Shark {
        Shark {
            bodies: vec![],
            centre: Vec3::ZERO,
            fwd0: Vec3::Z,
            mouth: 0.0,
            search: vec![],
            retreat: vec![],
            dive: vec![],
            sleep: (0.0, 0.0),
            searching: (0.0, 0.0),
            attacking: (0.0, 0.0),
            breach_depth: 0.0,
            breach_height: 0.0,
            speed: 0.0,
            state: SharkState::Sleeping,
            timer: 0.0,
            state_len: 0.0,
            pos: Vec3::ZERO,
            dir: Vec3::Z,
            node: 0,
            target: None,
            held: vec![],
            arc: None,
            started: false,
            start_at: 0.0,
            health: 100.0,
            max_health: 100.0,
            jaw: None,
            jaw_open: 0.0,
        }
    }
}

/// Damage of one collision on a breakable stage object (Gondola_Cable / SharkActor `OnCollisionEnter`): the hitter's mass
/// (0..40) times the impact along the contact normal, /1000, scaled by the hit type of the thing that struck it.
fn hit_damage(sim: &Sim, c: &gb_phys::Contact, beast_hit: &HashMap<usize, i32>) -> f32 {
    let other = sim.world.actors[c.other];
    let Some(ob) = other.body else { return 0.0 };
    let mass = sim.world.mass(ob).clamp(0.0, 40.0);
    let n = c.points.first().map_or(Vec3::ZERO, |p| p.normal);
    let dmg = (mass * n.dot(c.relative_velocity) / 1000.0).abs();
    let kind = beast_hit.get(&ob).copied().or_else(|| {
        sim.scenes
            .iter()
            .position(|s| s.0 == other.instance)
            .and_then(|si| sim.stage_interact.get(&(si, other.node)).copied().flatten())
            .map(|i| i.damage_modifier)
    });
    dmg * match kind {
        Some(0) | None => 0.0,
        Some(2) => 10.0,
        Some(3) => 50.0,
        Some(4) | Some(5) => 150.0,
        Some(6) => 100.0,
        Some(7) => 200.0,
        Some(8) => 1.0e6,
        _ => 1.0,
    }
}

fn swim(pos: &mut Vec3, dir: &mut Vec3, to: Vec3, speed: f32, dt: f32) {
    let want = (to - *pos).normalize_or_zero();
    if want != Vec3::ZERO {
        *dir = dir.lerp(want, 1.0 - (-3.0 * dt).exp()).normalize_or(want);
    }
    *pos += *dir * speed * dt;
}

fn closest(points: &[Vec3], p: Vec3) -> usize {
    points
        .iter()
        .enumerate()
        .min_by(|a, b| (*a.1 - p).length().total_cmp(&(*b.1 - p).length()))
        .map_or(0, |(i, _)| i)
}

fn shark_go(sh: &mut Shark, next: SharkState, len: f32) {
    info!("shark {:?} -> {:?}", sh.state, next);
    sh.state = next;
    sh.timer = 0.0;
    sh.state_len = len;
}

fn shark_step(sh: &mut Shark, sim: &mut Sim, level: f32, dt: f32, edt: f32, now: f32, r: [f32; 3]) {
    const G: f32 = 20.0;
    if !sh.started {
        if now < sh.start_at {
            return;
        }
        sh.started = true;
        sh.state_len = sh.sleep.0 + r[0] * (sh.sleep.1 - sh.sleep.0);
    }
    sh.timer += edt;
    {
        let target = jaw_target(sh, sim);
        let k = (6.0 * dt).min(1.0);
        sh.jaw_open += (target - sh.jaw_open) * k;
    }
    // CheckDamage / CheckHealth: beasts that hit the shark hurt it; it regains 1 health/s and, at 0, is knocked out
    // (drops its catch and goes back to sleep for up to `maxUnconsciousTime`).
    {
        let beast_hit: HashMap<usize, i32> = sim
            .actors
            .iter()
            .flat_map(|a| Part::ALL.iter().enumerate().map(move |(k, p)| (a.beast.body(*p), a.interact[k])))
            .collect();
        let mut dmg = 0.0;
        for c in &sim.world.contacts {
            if c.kind == gb_phys::ContactKind::Enter && sh.bodies.iter().any(|(b, _)| sim.world.actors[c.this].body == Some(*b)) {
                dmg += hit_damage(sim, c, &beast_hit);
            }
        }
        sh.health = (sh.health - dmg + edt).clamp(0.0, sh.max_health);
        if sh.health <= 0.0 && sh.state != SharkState::Sleeping {
            sh.held.clear();
            sh.health = 0.1;
            let len = 1.0 + r[2] * 0.0;
            shark_go(sh, SharkState::Sleeping, len);
        }
    }
    // Living beasts and their hips.
    let alive: Vec<(usize, Vec3)> = sim
        .actors
        .iter()
        .enumerate()
        .filter(|(k, a)| !sim.parked.get(*k).copied().unwrap_or(false) && crate::round::alive(a.state))
        .map(|(k, a)| (k, sim.world.pose(a.beast.body(Part::Hips)).position))
        .collect();
    let nearest = |p: Vec3| alive.iter().min_by(|a, b| (a.1 - p).length().total_cmp(&(b.1 - p).length())).copied();
    let mouth_pos = sh.pos + sh.dir * sh.mouth;
    match sh.state {
        SharkState::Sleeping => {
            if sh.timer >= sh.state_len {
                sh.node = closest(&sh.search, sh.pos);
                let len = sh.searching.0 + r[1] * (sh.searching.1 - sh.searching.0);
                shark_go(sh, SharkState::Searching, len);
            }
        }
        SharkState::Searching => {
            if let Some(&node) = sh.search.get(sh.node) {
                if (node - sh.pos).length() < 2.0 {
                    sh.node = (sh.node + 1) % sh.search.len();
                }
                swim(&mut sh.pos, &mut sh.dir, node, sh.speed, dt);
            }
            if sh.timer >= sh.state_len {
                match nearest(sh.pos) {
                    None => shark_go(sh, SharkState::Retreating, 0.0),
                    Some((k, p)) => {
                        sh.target = Some(k);
                        let len = sh.attacking.0 + r[1] * (sh.attacking.1 - sh.attacking.0);
                        // A beast out of the water is taken by a leap from below; one in the water is chased.
                        if p.y > level + 1.5 {
                            sh.arc = None;
                            shark_go(sh, SharkState::Breaching, len);
                        } else {
                            shark_go(sh, SharkState::Attacking, len);
                        }
                    }
                }
            }
        }
        SharkState::Attacking => {
            let t = sh.target.and_then(|k| alive.iter().find(|a| a.0 == k).copied()).or_else(|| nearest(sh.pos));
            match t {
                Some((k, p)) => {
                    sh.target = Some(k);
                    let aim = Vec3::new(p.x, p.y.min(level - 0.3), p.z);
                    swim(&mut sh.pos, &mut sh.dir, aim, sh.speed, dt);
                    if (mouth_pos - p).length() < 1.8 && p.y < level + 1.2 {
                        grab(sh, sim, k);
                        shark_go(sh, SharkState::Carrying, 0.0);
                    } else if p.y > level + 1.5 {
                        sh.arc = None;
                        let len = sh.state_len;
                        shark_go(sh, SharkState::Breaching, len);
                    }
                }
                None => shark_go(sh, SharkState::Retreating, 0.0),
            }
            if sh.state == SharkState::Attacking && sh.timer >= sh.state_len {
                shark_go(sh, SharkState::Retreating, 0.0);
            }
        }
        SharkState::Breaching => {
            let t = sh.target.and_then(|k| alive.iter().find(|a| a.0 == k).copied());
            if let Some(vel) = sh.arc {
                // Ballistic leap.
                let v = vel - Vec3::Y * G * dt;
                sh.arc = Some(v);
                sh.pos += v * dt;
                sh.dir = v.normalize_or(sh.dir);
                if let Some((k, p)) = t {
                    if (sh.pos + sh.dir * sh.mouth - p).length() < 2.0 {
                        grab(sh, sim, k);
                        sh.arc = None;
                        shark_go(sh, SharkState::Carrying, 0.0);
                        return apply_shark(sh, sim);
                    }
                }
                if v.y < 0.0 && sh.pos.y < level - 2.0 {
                    sh.arc = None;
                    shark_go(sh, SharkState::Attacking, 6.0);
                }
            } else if let Some((_, p)) = t {
                // Get under the target at `breachDepth`, then leap so the apex is over it.
                let below = Vec3::new(p.x, level + sh.breach_depth, p.z);
                swim(&mut sh.pos, &mut sh.dir, below, sh.speed, dt);
                if (below - sh.pos).length() < 2.0 {
                    let rise = (sh.breach_height - sh.breach_depth).max(1.0);
                    let vy = (2.0 * G * rise).sqrt();
                    let t_apex = vy / G;
                    let h = Vec3::new(p.x - sh.pos.x, 0.0, p.z - sh.pos.z) / t_apex;
                    sh.arc = Some(Vec3::new(h.x, vy, h.z));
                    sim.sound_queue.push(("GB SFX WATER SPLASHESnew".into(), 0.9, Some(sh.pos)));
                    sim.sound_queue.push(("GB SFX SHARK JAW OPEN".into(), 0.8, Some(sh.pos)));
                }
                if sh.timer >= sh.state_len {
                    shark_go(sh, SharkState::Retreating, 0.0);
                }
            } else {
                shark_go(sh, SharkState::Retreating, 0.0);
            }
        }
        SharkState::Carrying => {
            let node = sh.retreat.get(closest(&sh.retreat, sh.pos)).copied().unwrap_or(sh.pos - Vec3::Y * 20.0);
            swim(&mut sh.pos, &mut sh.dir, node, sh.speed, dt);
            let mouth_pos = sh.pos + sh.dir * sh.mouth;
            for (b, off) in &sh.held {
                sim.world.teleport(*b, Iso::new(mouth_pos + *off, Quat::IDENTITY));
                sim.world.set_linear_velocity(*b, Vec3::ZERO);
                sim.world.set_angular_velocity(*b, Vec3::ZERO);
            }
            if (node - sh.pos).length() < 2.5 || sh.timer > 12.0 || sh.pos.y < level - 6.0 {
                sh.held.clear();
                shark_go(sh, SharkState::Diving, 0.0);
            }
        }
        SharkState::Diving => {
            let node = sh.dive.get(closest(&sh.dive, sh.pos)).copied().unwrap_or(sh.pos - Vec3::Y * 20.0);
            swim(&mut sh.pos, &mut sh.dir, node, sh.speed, dt);
            if (node - sh.pos).length() < 2.5 || sh.timer > 15.0 {
                shark_go(sh, SharkState::Retreating, 0.0);
            }
        }
        SharkState::Retreating => {
            let node = sh.retreat.get(closest(&sh.retreat, sh.pos)).copied().unwrap_or(sh.pos);
            swim(&mut sh.pos, &mut sh.dir, node, sh.speed, dt);
            if (node - sh.pos).length() < 2.5 || sh.timer > 15.0 {
                sh.node = closest(&sh.search, sh.pos);
                let len = sh.searching.0 + r[2] * (sh.searching.1 - sh.searching.0);
                shark_go(sh, SharkState::Searching, len);
            }
        }
    }
    apply_shark(sh, sim);
}

/// `FixedJoint biteJoint` stand-in: the beast's body parts are carried at the mouth.
fn grab(sh: &mut Shark, sim: &mut Sim, actor: usize) {
    sim.sound_queue.push(("GB SFX SHARK JAW CLOSE".into(), 1.0, Some(sh.pos + sh.dir * sh.mouth)));
    let hips = sim.world.pose(sim.actors[actor].beast.body(Part::Hips)).position;
    sh.held = Part::ALL
        .iter()
        .map(|p| {
            let b = sim.actors[actor].beast.body(*p);
            (b, sim.world.pose(b).position - hips)
        })
        .collect();
    info!("shark bites beast {actor}");
}

fn apply_shark(sh: &Shark, sim: &mut Sim) {
    let rot = Quat::from_rotation_arc(sh.fwd0, sh.dir.normalize_or(sh.fwd0));
    // The jaw swings down about its hinge (axis = up x forward turns the mouth tip toward -Y).
    let fwd = rot * sh.fwd0;
    let jaw_rot = Quat::from_axis_angle(Vec3::Y.cross(fwd).normalize_or(Vec3::X), sh.jaw_open);
    for (b, rest) in &sh.bodies {
        let mut p = sh.pos + rot * (rest.position - sh.centre);
        let mut q = rot * rest.rotation;
        if let Some((jaw, hinge)) = sh.jaw.filter(|(jaw, _)| jaw == b) {
            let _ = jaw;
            let pivot = sh.pos + rot * (hinge - sh.centre);
            p = pivot + jaw_rot * (p - pivot);
            q = jaw_rot * q;
        }
        sim.world.move_kinematic(*b, Iso::new(p, q));
    }
}

/// Mouth angle target by state: open while chasing / leaping / just before biting, shut while carrying.
fn jaw_target(sh: &Shark, sim: &Sim) -> f32 {
    let near = sh.target.is_some_and(|a| {
        let hips = sim.world.pose(sim.actors[a].beast.body(Part::Hips)).position;
        (hips - (sh.pos + sh.dir * sh.mouth)).length() < 7.0
    });
    match sh.state {
        SharkState::Attacking | SharkState::Breaching if near => 0.7,
        SharkState::Attacking | SharkState::Breaching => 0.25,
        SharkState::Carrying => 0.05,
        _ => 0.0,
    }
}

/// `Trawler_Mechanics.sinking` / `sinkDelay` (150 s): the hull rolls over and goes down.
struct Capsize {
    hull: usize,
    pose: Iso,
    start: f32,
}

/// `FanController` + `PushVolume` (Vents' turbine): the fan cycles off (`FanDisabledTime`) -> wind-up -> spin
/// (`FanSpinTime`); while it spins everything inside its push volume is thrown upward.
struct Fan {
    trigger: Entity,
    center: Vec3,
    half: Vec3,
    thrust: f32,
    spin: f32,
    windup: f32,
    off: f32,
    clock: f32,
}

/// `Gondola_Cable` (decomp OnCollisionEnter): each cable segment has `health` (20); a collision removes
/// `clamp(otherMass, 0, 40) * |normal . relativeVelocity| / 1000` scaled by the other object's hit type
/// (none 0, x10, x50, x150, x100, x200, instant), and at 0 the cable's joints are destroyed.
struct Cable {
    body: usize,
    health: f32,
}

/// `Crane_RandomPointMover` (simplified base movement): offset along the gantry axis, retargeted every
/// `stateChangeDelayMin..Max` seconds within `maxDistanceBaseCanMove`, at `baseMovementSpeed`.
struct CraneMove {
    bodies: Vec<(usize, Iso)>,
    axis: Vec3,
    offset: f32,
    target: f32,
    max: f32,
    speed: f32,
    delay: (f32, f32),
    next: f32,
}

/// `Elevators_Car` (simplified): the car shuttles between its `floors` (metres above the first one), pausing at
/// each, as a kinematic body. The malfunction / cable-snap / fall sequence of `Elevators_Logic` is not ported.
struct Lift {
    body: usize,
    pose: Iso,
    start_y: f32,
    floors: Vec<f32>,
    index: usize,
    going_up: bool,
    wait: f32,
    /// The car's rope segments (`Elevators_Cable` rigidbodies): held still while the car works, freed when it snaps.
    cables: Vec<usize>,
}

/// The Train stage's endless track (`TrackPool` 12 m/s, `TrackMover`, pieces `trackSectionOffset` 100 m long): the
/// train stays at the origin while pieces scroll underneath and recycle at the front. Each new piece is a straight, left
/// or right section drawn from the three pools (left / right sections jog the track 20 m sideways). The game steers the
/// train along the `TrackNode` chain with `NodeFollower` forces; here the train stays put and the whole track is shifted
/// sideways so the rails under it stay centred. Boulder landslides are separate.
struct Track {
    pieces: Vec<TrackPiece>,
    active: Vec<ActivePiece>,
    speed: f32,
    length: f32,
    /// World z of the point under the train.
    ref_z: f32,
}

struct TrackPiece {
    entity: Entity,
    bodies: Vec<(usize, Iso)>,
    /// 0 straight, 1 left, 2 right.
    kind: u8,
    /// TrackNode chain (local x, z), sorted by z.
    poly: Vec<(f32, f32)>,
    /// Lateral jog from the piece start to the next piece's start.
    dx: f32,
}

struct ActivePiece {
    idx: usize,
    /// Start offset of the piece (lateral, along the track).
    sx: f32,
    sz: f32,
}

/// Pooled physics props (`CoreNet.Pooling.PoolSpawner` + `Pool`/`SyncPool`, 0x488FD0): Chute's meat paste, Incinerator's
/// boxes and barrels, Trucks' sign holders. The scene ships each pool as inactive `(Clone)` bodies; they are parked
/// at load and, every `DelayMin..DelayMax` seconds, one free item is dropped at the spawner (+ `RandomSpawnOffset`).
struct PropSpawner {
    entity: Entity,
    items: Vec<PropItem>,
    delay_min: f32,
    delay_max: f32,
    offset: Vec3,
    since: f32,
    next: f32,
}

struct PropItem {
    entity: Entity,
    /// Every rigid body under the pooled root with its authored pose, and the root's authored position.
    bodies: Vec<(usize, Iso)>,
    root_pos: Vec3,
    live: bool,
    age: f32,
}

/// `OnTriggerStayApplyForce` (0x751570): a door/shutter body is pushed every step with `openForce` while the
/// door is open and `closeForce` otherwise (VelocityChange; Linear -> AddForce, Torque -> AddTorque). A beast
/// inside the trigger opens it after `_waitOpenTime`; leaving closes it after `_waitClosedTime`.
struct Door {
    trigger: Entity,
    center: Vec3,
    half: Vec3,
    target: usize,
    torque: bool,
    open_force: Vec3,
    close_force: Vec3,
    wait_open: f32,
    wait_close: f32,
    inside_time: f32,
    outside_time: f32,
    open: bool,
}

/// `SimpleBuoyancy.FixedUpdateImpl` (0x7DEE70), simplified: below the liquid surface the body gets an
/// upward ACCELERATION (AddForceAtPosition mode 5) of `(force * FORCE_MULTIPLIER(2)) * depth / forceFalloff`.
struct Floater {
    body: usize,
    force: f32,
    falloff: f32,
    /// Authored (rest) height: the scene is saved with every floating body already at its waterline.
    rest_y: f32,
    /// Integral term: carries jointed fragments / riders so the body settles exactly at `rest_y`.
    bias: f32,
}

/// `Road` (0x7487E0): a kinematic road tile that MoveTowards(end) at `speed` and wraps to `start`.
struct Road {
    body: usize,
    pose: Iso,
    start: Vec3,
    end: Vec3,
    speed: f32,
}

/// `TruckBase` (0x7A8560): keeps the truck inside `start + [min, max]` (x/z) with a gentle force
/// and wanders along a random `direction` re-rolled every 0..10 s.
struct Truck {
    body: usize,
    start: Vec3,
    min: Vec3,
    max: Vec3,
    force: f32,
    out_of_bounds: bool,
    direction: Vec3,
    next_direction: f32,
}

#[derive(Default)]
pub struct StageEvents {
    in_water: HashSet<usize>,
    track_shift: f32,
    last_splash: f32,
    debug_road_bucket: u64,
    key: (usize, usize, u32),
    ready: bool,
    trains: Vec<Train>,
    spawners: Vec<Spawner>,
    triggers: Vec<Trigger>,
    zones: Vec<Zone>,
    wheels: Vec<Wheel>,
    roads: Vec<Road>,
    trucks: Vec<Truck>,
    /// `RotateOverTime` (0x736400): (entity, degrees per second about each Unity axis).
    spinners: Vec<(Entity, Vec3)>,
    floaters: Vec<Floater>,
    doors: Vec<Door>,
    props: Vec<PropSpawner>,
    track: Option<Track>,
    lifts: Vec<Lift>,
    cables: Vec<Cable>,
    cranes: Vec<CraneMove>,
    /// `Elevators_Logic` malfunction: (time it happens, which car).
    lift_failure: Option<(f32, usize)>,
    fans: Vec<Fan>,
    /// (material name prefix, colour) chosen by ContainerTinter, applied by `apply_container_tints`.
    pub container_tints: Vec<(String, Color)>,
    tints_applied: bool,
    time: f32,
    boulders: Option<Boulders>,
    sharks: Vec<Shark>,
    capsize: Option<Capsize>,
    /// `Liquid` surface heights (Unity y) of the stage.
    water_level: Option<f32>,
    rng: u32,
}

impl StageEvents {
    fn rand01(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }
}

fn collider_box(node: &gb_phys::source::Node) -> Option<(Vec3, Vec3)> {
    let c = node.components.iter().find(|c| c.kind == "BoxCollider")?;
    let center = vec3(&c.data["m_Center"]);
    let size = vec3(&c.data["m_Size"]);
    Some((center, size * 0.5))
}

/// Local-space containment test for a Unity-space collider box on a Bevy entity.
fn inside(global: &GlobalTransform, center: Vec3, half: Vec3, point_bevy: Vec3) -> bool {
    let local = global.affine().inverse().transform_point3(point_bevy);
    (local - mirror_position(center)).abs().cmple(half).all()
}

fn actor_points(sim: &Sim) -> Vec<(usize, Vec3)> {
    let mut out = Vec::new();
    for (k, actor) in sim.actors.iter().enumerate() {
        if sim.parked.get(k).copied().unwrap_or(false) {
            continue;
        }
        for part in Part::ALL {
            let body = actor.beast.body(part);
            out.push((body, sim.world.pose(body).position));
        }
    }
    out
}

static DEBUG_RELOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn stage_events(
    mut sim: NonSendMut<Sim>,
    map: Res<NodeEntities>,
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    mut state: Local<StageEvents>,
    mut transforms: Query<&mut Transform>,
    globals: Query<&GlobalTransform>,
    mut visibility: Query<&mut Visibility>,
    mat_query: Query<(&MeshMaterial3d<StandardMaterial>, &bevy::gltf::GltfMaterialName)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    vinyl_query: Query<(&MeshMaterial3d<crate::vinyl::VinylMaterial>, &bevy::gltf::GltfMaterialName)>,
    mut vinyl: ResMut<Assets<crate::vinyl::VinylMaterial>>,
    sea: Option<Res<crate::water::SeaWaves>>,
) {
    if sim.lobby {
        return;
    }
    let Some((_, stage, _)) = sim.scenes.first() else { return };
    let key = (stage.nodes.len(), sim.world.instances.len(), sim.reloads);
    if state.key != key {
        *state = StageEvents { key, rng: 0x9E3779B9 ^ key.0 as u32, ..Default::default() };
    }
    if !state.ready {
        let n = sim.scenes[0].1.nodes.len();
        if !scene_mapping_complete(map.0.keys().copied(), 0, n) {
            return;
        }
        state.ready = true;
        init(&mut sim, &map, &mut state, &mut visibility, &mut transforms);
    }
    let named_materials: Vec<(Handle<StandardMaterial>, String)> = if !state.tints_applied && !state.container_tints.is_empty() && vinyl_query.iter().next().is_some() {
        mat_query.iter().map(|(h, n)| (h.0.clone(), n.0.clone())).collect()
    } else {
        Vec::new()
    };
    let dt = time.delta_secs().min(0.1);
    let alpha = fixed.overstep_fraction() * fixed.timestep().as_secs_f32();
    let points = actor_points(&sim);

    // Wheel axles.
    for i in 0..state.wheels.len() {
        let (timer, max) = (state.wheels[i].timer - dt, state.wheels[i].max_speed);
        state.wheels[i].timer = timer;
        if timer <= 0.0 {
            let st = state.rand01() * 100.0;
            let span = if st > 50.0 && st <= 80.0 { 10.0 } else if st > 90.0 { 15.0 } else { 20.0 };
            let t = 5.0 + state.rand01() * (span - 5.0);
            state.wheels[i].state = st;
            state.wheels[i].timer = t;
        }
        let w = &mut state.wheels[i];
        let (target, rate) = match w.state {
            s if s == 0.0 => (10.0, 5.0),
            s if s <= 25.0 => (20.0, 5.0),
            s if s <= 50.0 => (-20.0, 5.0),
            s if s <= 65.0 => (60.0, 20.0),
            s if s <= 80.0 => (-60.0, 20.0),
            s if s <= 85.0 => (max, 20.0),
            s if s <= 90.0 => (-max, 20.0),
            _ => (0.0, 80.0),
        };
        let step = rate * dt;
        w.speed += (target - w.speed).clamp(-step, step);
        w.speed = w.speed.clamp(-max, max);
    }
    for w in &mut state.wheels {
        let turn = Quat::from_rotation_z((w.speed * dt).to_radians());
        w.pose = Iso::new(w.pose.position, (w.pose.rotation * turn).normalize());
        sim.world.move_kinematic(w.body, w.pose);
    }
    state.time += dt * std::env::var("GB_EVENT_TIME_SCALE").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
    let now = state.time;
    // Landslide boulders.
    if state.boulders.is_some() {
        let rr = [state.rand01(), state.rand01(), state.rand01()];
        let shift = state.track_shift;
        let b = state.boulders.as_mut().unwrap();
        for item in &mut b.items {
            if item.live {
                item.age += dt;
            }
        }
        if now >= b.start && now <= b.end && now >= b.next && !b.spawns.is_empty() {
            if let Some(ii) = b.items.iter().position(|i| !i.live) {
                let sp = b.spawns[(rr[0] * b.spawns.len() as f32) as usize % b.spawns.len()];
                if let Ok(g) = globals.get(sp) {
                    let pos = mirror_position(g.translation()) + Vec3::new((rr[1] - 0.5) * 6.0 + shift, 0.0, (rr[2] - 0.5) * 6.0);
                    let item = &mut b.items[ii];
                    for (body, rest) in &item.bodies {
                        sim.world.restore_body(*body);
                        sim.world.teleport(*body, Iso::new(pos + (rest.position - item.root_pos), rest.rotation));
                    }
                    item.live = true;
                    item.age = 0.0;
                    if let Ok(mut v) = visibility.get_mut(item.entity) {
                        *v = Visibility::Visible;
                    }
                    info!("landslide boulder at {pos:?}");
                }
            }
            b.next = now + 2.0 + rr[1] * 3.0;
        }
        for item in &mut b.items {
            let lowest = item.bodies.iter().map(|(bd, _)| sim.world.pose(*bd).position.y).fold(f32::MAX, f32::min);
            if item.live && (item.age > 40.0 || lowest < -80.0) {
                for (bd, _) in &item.bodies {
                    sim.world.set_linear_velocity(*bd, Vec3::ZERO);
                    sim.world.remove_body(*bd);
                }
                item.live = false;
                if let Ok(mut v) = visibility.get_mut(item.entity) {
                    *v = Visibility::Hidden;
                }
            }
        }
    }
    // Sharks.
    if !state.sharks.is_empty() {
        let level = state.water_level.unwrap_or(0.0);
        let edt = dt * std::env::var("GB_EVENT_TIME_SCALE").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        for i in 0..state.sharks.len() {
            let r = [state.rand01(), state.rand01(), state.rand01()];
            let mut sh = std::mem::replace(&mut state.sharks[i], Shark::placeholder());
            shark_step(&mut sh, &mut sim, level, dt, edt, now, r);
            state.sharks[i] = sh;
        }
    }
    // Trawler capsize.
    if let Some(c) = &mut state.capsize {
        if now > c.start {
            let t = (now - c.start).min(60.0);
            let roll = (t / 60.0).powf(1.5) * 1.1;
            let sink = t * 0.1;
            let rot = Quat::from_rotation_z(roll) * c.pose.rotation;
            sim.world.move_kinematic(c.hull, Iso::new(c.pose.position - Vec3::Y * sink, rot));
        }
    }
    // Wait until the stage's materials have been swapped to the vinyl shader (a few frames after load).
    if !state.tints_applied && !state.container_tints.is_empty() && vinyl_query.iter().next().is_some() {
        state.tints_applied = true;
        for (prefix, colour) in &state.container_tints {
            for (h, n) in &vinyl_query {
                if n.0.starts_with(prefix.as_str()) {
                    if let Some(m) = vinyl.get_mut(&h.0) {
                        m.base.base_color = *colour;
                    }
                }
            }
        }
        for (prefix, colour) in &state.container_tints {
            for (handle, name) in &named_materials {
                if name.starts_with(prefix.as_str()) {
                    if let Some(m) = materials.get_mut(handle) {
                        m.base_color = *colour;
                    }
                }
            }
        }
        info!("stage events: tinted {} container colour(s)", state.container_tints.len());
    }
    // Fans.
    for f in &mut state.fans {
        f.clock = (f.clock + dt) % (f.off + f.windup + f.spin);
        let power = if f.clock < f.off {
            0.0
        } else if f.clock < f.off + f.windup {
            (f.clock - f.off) / f.windup.max(0.01)
        } else {
            1.0
        };
        if power > 0.2 {
            if let Ok(g) = globals.get(f.trigger) {
                for (b, p) in &points {
                    if inside(g, f.center, f.half, mirror_position(*p)) {
                        sim.world.add_force(*b, Vec3::Y * (20.0 * f.thrust * 1.6 * power) * (dt / fixed.timestep().as_secs_f32()), 5);
                    }
                }
            }
        }
    }
    // Elevator malfunction: one car's cable snaps; it becomes a free body and drops down the shaft.
    if let Some((at, car)) = state.lift_failure {
        if now >= at && car < state.lifts.len() {
            let body = state.lifts[car].body;
            sim.world.set_kinematic(body, false);
            sim.world.set_use_gravity(body, true);
            let n = sim.world.release_joints_of(body);
            for c in state.lifts[car].cables.clone() {
                sim.world.set_kinematic(c, false);
            }
            info!("elevator car released {n} joint(s)");
            let snap_at = sim.world.pose(body).position;
            sim.sound_queue.push(("GB SFX METAL CABLE SNAP".into(), 1.0, Some(snap_at)));
            state.lifts.remove(car);
            state.lift_failure = None;
            info!("elevator {car} malfunction: cable snapped");
        }
    }
    // Gondola cables.
    if !state.cables.is_empty() {
        let beast_hit: HashMap<usize, i32> = sim
            .actors
            .iter()
            .flat_map(|a| Part::ALL.iter().enumerate().map(move |(k, p)| (a.beast.body(*p), a.interact[k])))
            .collect();
        let mut hits: Vec<(usize, f32)> = Vec::new();
        for c in &sim.world.contacts {
            if c.kind != gb_phys::ContactKind::Enter {
                continue;
            }
            let Some(cab) = state.cables.iter().position(|k| sim.world.actors[c.this].body == Some(k.body)) else { continue };
            let dmg = hit_damage(&sim, c, &beast_hit);
            hits.push((cab, dmg));
        }
        for (cab, dmg) in hits {
            let k = &mut state.cables[cab];
            if k.health <= 0.0 {
                continue;
            }
            k.health -= dmg;
            if k.health <= 0.0 {
                let n = sim.world.release_joints_of(k.body);
                info!("gondola cable snapped ({n} joints released)");
                let at = sim.world.pose(k.body).position;
                sim.sound_queue.push(("GB SFX METAL CABLE SNAP".into(), 1.0, Some(at)));
            }
        }
    }
    // Cranes.
    for i in 0..state.cranes.len() {
        let (r1, r2) = (state.rand01(), state.rand01());
        let c = &mut state.cranes[i];
        if now >= c.next {
            c.target = (r1 * 2.0 - 1.0) * c.max;
            c.next = now + c.delay.0 + r2 * (c.delay.1 - c.delay.0);
        }
        let step = c.speed * dt;
        c.offset += (c.target - c.offset).clamp(-step, step);
        for (b, rest) in &c.bodies {
            sim.world.move_kinematic(*b, Iso::new(rest.position + c.axis * c.offset, rest.rotation));
        }
    }
    // Elevator cars.
    for l in &mut state.lifts {
        if l.floors.len() < 2 {
            continue;
        }
        let target = l.start_y + (l.floors[l.index] - l.floors[0]);
        if l.wait > 0.0 {
            l.wait -= dt;
        } else {
            let dy = target - l.pose.position.y;
            let step = 2.0 * dt;
            if dy.abs() <= step {
                l.pose = Iso::new(Vec3::new(l.pose.position.x, target, l.pose.position.z), l.pose.rotation);
                l.wait = 4.0;
                if l.going_up {
                    if l.index + 1 >= l.floors.len() {
                        l.going_up = false;
                        l.index -= 1;
                    } else {
                        l.index += 1;
                    }
                } else if l.index == 0 {
                    l.going_up = true;
                    l.index = 1;
                } else {
                    l.index -= 1;
                }
            } else {
                l.pose = Iso::new(l.pose.position + Vec3::Y * dy.signum() * step, l.pose.rotation);
            }
        }
        sim.world.move_kinematic(l.body, l.pose);
    }
    // Scrolling track pieces.
    if state.track.is_some() {
        let (r1, r2) = (state.rand01(), state.rand01());
        let track = state.track.as_mut().unwrap();
        for ap in &mut track.active {
            ap.sz -= track.speed * dt;
        }
        // Recycle the piece that has scrolled out of sight at the back.
        if track.active.first().is_some_and(|f| f.sz + track.length < -track.length * 2.5) {
            let gone = track.active.remove(0);
            for (body, _) in &track.pieces[gone.idx].bodies {
                sim.world.move_kinematic(*body, Iso::new(Vec3::new(0.0, -3000.0, 0.0), Quat::IDENTITY));
            }
            if let Ok(mut v) = visibility.get_mut(track.pieces[gone.idx].entity) {
                *v = Visibility::Hidden;
            }
            let last = track.active.last().map(|l| (l.idx, l.sx, l.sz)).unwrap_or((0, 0.0, 0.0));
            let (sx, sz) = (last.1 + track.pieces[last.0].dx, last.2 + track.length);
            // Wander left / right but come back when the track drifts more than `maxWonderArea` from the centre.
            let mut kind = if r1 < 0.34 { 0 } else if r1 < 0.67 { 1 } else { 2 };
            if sx > 45.0 {
                kind = 2;
            } else if sx < -45.0 {
                kind = 1;
            }
            let free: Vec<usize> = (0..track.pieces.len())
                .filter(|i| track.pieces[*i].kind == kind && !track.active.iter().any(|a| a.idx == *i))
                .collect();
            let idx = free.get((r2 * free.len() as f32) as usize % free.len().max(1)).copied().unwrap_or(gone.idx);
            track.active.push(ActivePiece { idx, sx, sz });
            for (body, _) in &track.pieces[idx].bodies {
                sim.world.restore_body(*body);
            }
            if let Ok(mut v) = visibility.get_mut(track.pieces[idx].entity) {
                *v = Visibility::Visible;
            }
        }
        // Centre line under the train.
        let mut centre = 0.0;
        for ap in &track.active {
            let lz = track.ref_z - ap.sz;
            if lz >= 0.0 && lz < track.length {
                let poly = &track.pieces[ap.idx].poly;
                centre = ap.sx
                    + match poly.iter().position(|(_, z)| *z >= lz) {
                        Some(0) | None => poly.first().map_or(0.0, |p| p.0),
                        Some(k) => {
                            let (a, b) = (poly[k - 1], poly[k]);
                            a.0 + (b.0 - a.0) * ((lz - a.1) / (b.1 - a.1).max(1e-3)).clamp(0.0, 1.0)
                        }
                    };
                break;
            }
        }
        let shift = -centre;
        if std::env::var_os("GB_TRACK_DEBUG").is_some() && sim.world.steps % 240 == 0 {
            let kinds: Vec<u8> = track.active.iter().map(|a| track.pieces[a.idx].kind).collect();
            info!("track: shift {shift:.1} centre {centre:.1} pieces {kinds:?}");
            for (k, ac) in sim.actors.iter().enumerate() {
                let p = sim.world.pose(ac.beast.body(Part::Hips)).position;
                info!("  actor {k} state {} hips {:.1} {:.1} {:.1}", ac.state, p.x, p.y, p.z);
            }
        }
        state.track_shift = shift;
        let track = state.track.as_ref().unwrap();
        for ap in &track.active {
            for (body, rest) in &track.pieces[ap.idx].bodies {
                // Pool parking runs after this module's setup, so make sure active pieces are in the simulation.
                sim.world.restore_body(*body);
                let p = Iso::new(rest.position + Vec3::new(ap.sx + shift, 0.0, ap.sz), rest.rotation);
                sim.world.move_kinematic(*body, p);
            }
        }
    }
    // Falling props.
    {
        let mut spawns: Vec<(usize, usize)> = Vec::new();
        for (si, sp) in state.props.iter_mut().enumerate() {
            sp.since += dt;
            for item in &mut sp.items {
                if item.live {
                    item.age += dt;
                }
            }
            if sp.since >= sp.next {
                if let Some(ii) = sp.items.iter().position(|i| !i.live) {
                    spawns.push((si, ii));
                }
            }
        }
        for (si, ii) in spawns {
            let r = [state.rand01(), state.rand01(), state.rand01(), state.rand01(), state.rand01(), state.rand01()];
            let sp = &mut state.props[si];
            let Ok(g) = globals.get(sp.entity) else { continue };
            let off = Vec3::new((r[0] * 2.0 - 1.0) * sp.offset.x, r[1] * sp.offset.y, (r[2] * 2.0 - 1.0) * sp.offset.z);
            let pos = mirror_position(g.translation()) + off;
            let rot = Quat::from_euler(EulerRot::YXZ, r[3] * std::f32::consts::TAU, r[4] * std::f32::consts::TAU, r[5] * std::f32::consts::TAU);
            let item = &mut sp.items[ii];
            let _ = rot;
            for (body, rest) in &item.bodies {
                sim.world.restore_body(*body);
                sim.world.teleport(*body, Iso::new(pos + (rest.position - item.root_pos), rest.rotation));
            }
            item.live = true;
            item.age = 0.0;
            if std::env::var_os("GB_PROP_DEBUG").is_some() {
                info!("prop spawned at {pos:?} ({} bodies)", item.bodies.len());
            }
            if let Ok(mut v) = visibility.get_mut(item.entity) {
                *v = Visibility::Visible;
            }
            let (min, max) = (sp.delay_min, sp.delay_max);
            let nr = r[3];
            sp.since = 0.0;
            sp.next = min + (max - min).max(0.0) * nr;
        }
        for sp in &mut state.props {
            for item in &mut sp.items {
                if item.live && std::env::var_os("GB_PROP_DEBUG").is_some() && item.age > 6.0 && item.age < 6.1 {
                    let p = sim.world.pose(item.bodies[0].0).position;
                    info!("prop after 6s at {p:?}");
                }
                let lowest = item.bodies.iter().map(|(b, _)| sim.world.pose(*b).position.y).fold(f32::MAX, f32::min);
                if item.live && (item.age > 25.0 || lowest < -60.0) {
                    for (body, _) in &item.bodies {
                        sim.world.set_linear_velocity(*body, Vec3::ZERO);
                        sim.world.remove_body(*body);
                    }
                    item.live = false;
                    if let Ok(mut v) = visibility.get_mut(item.entity) {
                        *v = Visibility::Hidden;
                    }
                }
            }
        }
    }
    // Doors and shutters.
    if !state.doors.is_empty() {
        let step_scale = (dt / fixed.timestep().as_secs_f32()).max(0.0);
        for d in &mut state.doors {
            let inside = globals.get(d.trigger).is_ok_and(|g| {
                points.iter().any(|(_, p)| inside(g, d.center, d.half, mirror_position(*p)))
            });
            if inside {
                d.outside_time = 0.0;
                d.inside_time += dt;
                if d.inside_time >= d.wait_open {
                    d.open = true;
                }
            } else {
                d.inside_time = 0.0;
                d.outside_time += dt;
                if d.outside_time >= d.wait_close {
                    d.open = false;
                }
            }
            let f = if d.open { d.open_force } else { d.close_force } * step_scale;
            if d.torque {
                sim.world.add_torque(d.target, f, 2);
            } else {
                sim.world.add_force(d.target, f, 2);
            }
        }
    }
    // Wave height at a world xz (the sea shader's Gerstner sum), 0 on stages without waves.
    let wave_t = time.elapsed_secs_wrapped();
    let wave = |x: f32, z: f32| sea.as_ref().map_or(0.0, |s| s.height(x, z, wave_t));
    // Beasts in water float at the surface (Actor swim state is not ported; this keeps heads above water so sharks
    // and the trawler stay the hazard): upward acceleration grows with depth, plus water drag.
    if let Some(level) = state.water_level {
        let step_scale = dt / fixed.timestep().as_secs_f32();
        let mut splash: Option<Vec3> = None;
        for (b, p) in &points {
            let depth = level + wave(p.x, p.z) - p.y;
            if depth > 0.0 && depth < 6.0 {
                let v = sim.world.linear_velocity(*b);
                let a = Vec3::Y * (20.0 + 24.0 * depth.min(1.5)) - v * 1.8;
                sim.world.add_force(*b, a * step_scale, 5);
                // A body crossing the surface fast makes a splash (once per body until it leaves the water).
                if depth < 0.6 && v.y < -2.0 && state.in_water.insert(*b) {
                    splash = Some(*p);
                }
            } else if depth <= -0.3 {
                state.in_water.remove(b);
            }
        }
        // Swimming (stand-in for the Swim state's stroke cycle): a beast in the water follows its stick with a
        // paddling push on the hips and chest, so it can swim back to the shore instead of just bobbing.
        for k in 0..sim.actors.len() {
            if !crate::round::alive(sim.actors[k].state) || sim.parked[k] {
                continue;
            }
            let hips = sim.actors[k].beast.body(Part::Hips);
            let p = sim.world.pose(hips).position;
            let depth = level + wave(p.x, p.z) - p.y;
            if depth < 0.15 {
                continue;
            }
            let dir = Vec3::new(
                sim.inputs[k].analogue(gb_logic::input::HORIZONTAL),
                0.0,
                sim.inputs[k].analogue(gb_logic::input::VERTICAL),
            );
            if dir.length() < 0.1 {
                continue;
            }
            let push = dir.clamp_length_max(1.0) * 9.0 * step_scale;
            for part in [Part::Hips, Part::Chest] {
                let body = sim.actors[k].beast.body(part);
                sim.world.add_force(body, push, 5);
            }
        }
        if let Some(at) = splash {
            if now - state.last_splash > 0.3 {
                state.last_splash = now;
                sim.sound_queue.push(("GB SFX WATER SPLASHESnew".into(), 0.7, Some(at)));
            }
        }
    }
    // Floating bodies (buoys, ice, crane containers).
    if let Some(level) = state.water_level {
        let step_scale = dt / fixed.timestep().as_secs_f32();
        for f in &mut state.floaters {
            // The source law (acceleration = force*2*depth/falloff below the surface) settles each body some
            // distance under its authored height (pivot vs collider bounds are not the same point), so buoys,
            // ice and the Trawler hull sat too low. Use the same stiffness as a spring about the authored rest
            // height: a = g + k * (rest - y) - c * vy, which holds the saved pose and still bobs when pushed.
            let pose_pos = sim.world.pose(f.body).position;
            let pose_y = pose_pos.y;
            let rest_y = f.rest_y + wave(pose_pos.x, pose_pos.z);
            // Buoyancy only acts in the water: bodies that carry SimpleBuoyancy but ride above it (the Ferris
            // wheel's burger cars, crane containers) must hang freely.
            if pose_y > level + 1.5 && f.rest_y > level + 1.5 {
                continue;
            }
            let vy = sim.world.linear_velocity(f.body).y;
            // x4: the source applies its force over every buoyancy collider of the body; one point under-supports
            // a floe with beasts on it (ice was pushed under the surface).
            let k = f.force * 2.0 / f.falloff.max(0.1) * 4.0;
            let g = 20.0;
            let _ = level;
            f.bias = (f.bias + k * (rest_y - pose_y) * dt * 1.5).clamp(-40.0, 120.0);
            let a = (g + f.bias + k * (rest_y - pose_y) - 2.0 * k.sqrt() * 0.6 * vy).clamp(0.0, 250.0);
            sim.world.add_force(f.body, Vec3::Y * a * step_scale, 5);
        }
    }
    // RotateOverTime: Transform.Rotate(speed * dt, Space.Self), e.g. the background 'City Pivot'.
    for (entity, speed) in &state.spinners {
        if let Ok(mut tf) = transforms.get_mut(*entity) {
            let d = *speed * dt;
            let q = Quat::from_euler(EulerRot::ZXY, d.z.to_radians(), d.x.to_radians(), d.y.to_radians());
            tf.rotation = (tf.rotation * mirror_rotation(q)).normalize();
        }
    }
    // Debug: GB_RELOAD_TEST=<step> reloads the stage once (exercises the between-rounds reset); GB_ROAD_DEBUG logs a tile.
    if let Some(n) = std::env::var("GB_RELOAD_TEST").ok().and_then(|v| v.parse::<u64>().ok()) {
        if sim.world.steps >= n && !DEBUG_RELOADED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            info!("debug: reload_stage");
            sim.reload_stage();
        }
    }
    if std::env::var_os("GB_ROAD_DEBUG").is_some() && sim.world.steps / 120 != state.debug_road_bucket {
        state.debug_road_bucket = sim.world.steps / 120;
        if let Some(r) = state.roads.first() {
            info!("road[0] at step {}: {:?} -> {:?}", sim.world.steps, sim.world.pose(r.body).position, r.end);
        }
    }
    // Scrolling roads (the Trucks stage's treadmill).
    for r in &mut state.roads {
        let to = r.end - r.pose.position;
        let step = r.speed * dt;
        let position = if to.length() <= step { r.start } else { r.pose.position + to.normalize() * step };
        r.pose = Iso::new(position, r.pose.rotation);
        sim.world.move_kinematic(r.body, r.pose);
    }
    // Truck wander. Forces are per physics step, so scale by frame time over step time.
    let step_scale = dt / fixed.timestep().as_secs_f32();
    for i in 0..state.trucks.len() {
        let r1 = state.rand01() * 2.0 - 1.0;
        let r2 = state.rand01() * 2.0 - 1.0;
        let r3 = state.rand01() * 10.0;
        let t = &mut state.trucks[i];
        t.next_direction -= dt;
        if t.next_direction <= 0.0 {
            t.direction = Vec3::new(t.force * r1, 0.0, t.force * r2);
            t.next_direction = r3;
        }
        let p = sim.world.pose(t.body).position;
        let rel = p - t.start;
        let mut f = Vec3::ZERO;
        if rel.x < t.min.x {
            f += Vec3::X * t.force * 4.0;
        } else if rel.x > t.max.x {
            f += Vec3::NEG_X * t.force * 4.0;
        }
        if rel.z < t.min.z {
            f += Vec3::Z * t.force * 2.0;
        } else if rel.z > t.max.z {
            f += Vec3::NEG_Z * t.force * 2.0;
        }
        t.out_of_bounds = f != Vec3::ZERO;
        if !t.out_of_bounds {
            f = t.direction;
        }
        sim.world.add_force(t.body, f * step_scale, 0);
    }

    // PoolSpawner timers and PoolSpawnTriggers.
    for s in &mut state.spawners {
        s.since += dt;
    }
    let mut spawn_requests: Vec<usize> = Vec::new();
    for (i, s) in state.spawners.iter().enumerate() {
        if s.since >= s.next {
            spawn_requests.push(i);
        }
    }
    for t in &mut state.triggers {
        t.timer += dt;
        if t.timer < t.repeat {
            continue;
        }
        let Ok(g) = globals.get(t.entity) else { continue };
        if points
            .iter()
            .any(|(_, p)| inside(g, t.center, t.half, mirror_position(*p)))
        {
            t.timer = 0.0;
            spawn_requests.push(t.spawner);
        }
    }
    spawn_requests.sort_unstable();
    spawn_requests.dedup();
    for si in spawn_requests {
        let (since, delay_min) = (state.spawners[si].since, state.spawners[si].delay_min);
        let Some(&ti) = state.spawners[si].pool.iter().find(|&&t| !state.trains[t].live) else {
            continue;
        };
        // A trigger only spawns once DelayMin has elapsed; the timer path always has.
        if since < delay_min {
            continue;
        }
        let spawner_entity = state.spawners[si].entity;
        let Ok(spawner_global) = globals.get(spawner_entity) else { continue };
        let origin_bevy = spawner_global.translation();
        let train = &mut state.trains[ti];
        let center = mirror_position(origin_bevy) + train.box_center;
        let pose = Iso::new(center, Quat::IDENTITY);
        let body = match train.body {
            Some(b) => {
                sim.world.restore_body(b);
                sim.world.teleport(b, pose);
                b
            }
            None => {
                let b = sim.world.add_box(pose, train.box_half, 10_000.0, train.layer, train.velocity);
                sim.world.set_use_gravity(b, false);
                train.body = Some(b);
                b
            }
        };
        sim.world.set_linear_velocity(body, train.velocity);
        train.live = true;
        info!("train {} spawned at {:?}", train.node, center);
        if let Ok(mut v) = visibility.get_mut(train.entity) {
            *v = Visibility::Visible;
        }
        let next_min = state.spawners[si].delay_min;
        let next_max = state.spawners[si].delay_max;
        let r = state.rand01();
        let s = &mut state.spawners[si];
        s.since = 0.0;
        s.next = next_min + (next_max - next_min).max(0.0) * r;
    }

    // Live trains: keep the kinematic velocity, follow the body, push, despawn.
    let mut pushes: Vec<(usize, Vec3)> = Vec::new();
    let mut despawn: Vec<usize> = Vec::new();
    for (ti, train) in state.trains.iter().enumerate() {
        if !train.live {
            continue;
        }
        let Some(body) = train.body else { continue };
        sim.world.set_linear_velocity(body, train.velocity);
        let pose = sim.world.pose(body);
        let shown = pose.position + train.velocity * alpha;
        let root_unity = shown - train.box_center;
        let spawner_global = globals.get(state.spawners[train.spawner].entity).ok();
        if let (Ok(mut tf), Some(sg)) = (transforms.get_mut(train.entity), spawner_global) {
            tf.translation = mirror_position(root_unity) - sg.translation();
        }
        for push in &train.pushes {
            let Ok(g) = globals.get(push.entity) else { continue };
            for (b, p) in &points {
                if inside(g, push.center, push.half, mirror_position(*p)) {
                    pushes.push((*b, push.direction * push.force));
                }
            }
        }
        for zone in &state.zones {
            if let Ok(g) = globals.get(zone.entity) {
                if inside(g, zone.center, zone.half, mirror_position(pose.position)) {
                    despawn.push(ti);
                    break;
                }
            }
        }
    }
    for (b, f) in pushes {
        sim.world.add_force(b, f, 0);
    }
    for ti in despawn {
        let (entity, body) = (state.trains[ti].entity, state.trains[ti].body);
        if let Some(b) = body {
            sim.world.set_linear_velocity(b, Vec3::ZERO);
            sim.world.remove_body(b);
        }
        if let Ok(mut v) = visibility.get_mut(entity) {
            *v = Visibility::Hidden;
        }
        info!("train {} despawned", state.trains[ti].node);
        state.trains[ti].live = false;
    }
}

fn init(
    sim: &mut Sim,
    map: &NodeEntities,
    state: &mut StageEvents,
    visibility: &mut Query<&mut Visibility>,
    transforms_init: &mut Query<&mut Transform>,
) {
    let nodes = sim.scenes[0].1.nodes.clone();
    // Pooled (Clone) instances are inactive until their spawner activates them.
    for (i, node) in nodes.iter().enumerate() {
        // Only pooled instances ("X(Clone)") start hidden. Hiding every inactive node made scene
        // props that a script or the game mode switches on (e.g. the Rooftop's doors) vanish.
        let pooled = node.path.rsplit('/').next().is_some_and(|n| n.contains("(Clone)"));
        if !node.active && !node.render_override && pooled {
            if let Some(&e) = map.0.get(&(0, i)) {
                if let Ok(mut v) = visibility.get_mut(e) {
                    *v = Visibility::Hidden;
                }
            }
        }
    }
    let script_of = |node: &gb_phys::source::Node, name: &str| -> Option<serde_json::Value> {
        node.components
            .iter()
            .find(|c| c.script.as_deref() == Some(name))
            .map(|c| c.data.clone())
    };
    // Spawners and their pools.
    let mut spawner_of_node: HashMap<usize, usize> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        let (Some(spawner), Some(pool)) = (script_of(node, "PoolSpawner"), script_of(node, "SyncPool")) else {
            continue;
        };
        let Some(&entity) = map.0.get(&(0, i)) else { continue };
        let pool_nodes: Vec<usize> = pool["_Pool"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["node"].as_u64().map(|n| n as usize))
            .collect();
        let (delay_min, delay_max) = (
            spawner["DelayMin"].as_f64().unwrap_or(4.0) as f32,
            spawner["DelayMax"].as_f64().unwrap_or(30.0) as f32,
        );
        let si = state.spawners.len();
        spawner_of_node.insert(i, si);
        let mut indices = Vec::new();
        for pn in pool_nodes {
            let Some(item) = nodes.get(pn) else { continue };
            let Some(train_data) = script_of(item, "Train") else { continue };
            let Some(&train_entity) = map.0.get(&(0, pn)) else { continue };
            let (box_center, box_half) = collider_box(item).unwrap_or((Vec3::ZERO, Vec3::splat(1.0)));
            // Descendant TrainPush triggers.
            let mut pushes = Vec::new();
            for (j, d) in nodes.iter().enumerate() {
                let Some(push) = script_of(d, "TrainPush") else { continue };
                let mut cur = d.parent;
                let mut under = false;
                while let Some(p) = cur {
                    if p == pn {
                        under = true;
                        break;
                    }
                    cur = nodes[p].parent;
                }
                if !under {
                    continue;
                }
                let (center, half) = collider_box(d).unwrap_or((Vec3::ZERO, Vec3::ZERO));
                if let Some(&e) = map.0.get(&(0, j)) {
                    pushes.push(Push {
                        entity: e,
                        direction: vec3(&push["direction"]),
                        force: push["force"].as_f64().unwrap_or(0.0) as f32,
                        center,
                        half,
                    });
                }
            }
            indices.push(state.trains.len());
            state.trains.push(Train {
                node: pn,
                entity: train_entity,
                velocity: vec3(&train_data["moveAmount"]),
                box_center,
                box_half,
                layer: item.layer,
                body: None,
                live: false,
                pushes,
                spawner: si,
            });
            if let Ok(mut v) = visibility.get_mut(train_entity) {
                *v = Visibility::Hidden;
            }
        }
        let r = state.rand01();
        state.spawners.push(Spawner {
            entity,
            pool: indices,
            delay_min,
            delay_max,
            since: 0.0,
            next: delay_min + (delay_max - delay_min).max(0.0) * r,
        });
    }
    for (i, node) in nodes.iter().enumerate() {
        if let Some(t) = script_of(node, "PoolSpawnTrigger") {
            let target = t["TargetSpawner"]["node"].as_u64().map(|n| n as usize);
            let (Some(si), Some(&entity), Some((center, half))) = (
                target.and_then(|n| spawner_of_node.get(&n).copied()),
                map.0.get(&(0, i)),
                collider_box(node),
            ) else {
                continue;
            };
            state.triggers.push(Trigger {
                entity,
                spawner: si,
                repeat: t["RepeatTime"].as_f64().unwrap_or(1.5) as f32,
                timer: 0.0,
                center,
                half,
            });
        }
        if script_of(node, "TrainDespawnZone").is_some() {
            if let (Some(&entity), Some((center, half))) = (map.0.get(&(0, i)), collider_box(node)) {
                state.zones.push(Zone { entity, center, half });
            }
        }
    }
    for (i, node) in nodes.iter().enumerate() {
        if let Some(d) = script_of(node, "RotateOverTime") {
            if d["rotate"].as_u64().unwrap_or(1) != 0 {
                if let Some(&e) = map.0.get(&(0, i)) {
                    state.spinners.push((e, vec3(&d["rotationSpeed"])));
                }
            }
        }
    }
    // GerstnerLiquid.offset shifts the visible sea relative to the Liquid node (Trawler: -0.6 m).
    for (i, node) in nodes.iter().enumerate() {
        if let Some(d) = script_of(node, "GerstnerLiquid") {
            let offset = d["offset"].as_f64().unwrap_or(0.0) as f32;
            if let Some(&e) = map.0.get(&(0, i)) {
                if let Ok(mut tf) = transforms_init.get_mut(e) {
                    tf.translation.y += offset;
                }
            }
        }
    }
    // Dynamic assemblies that a stage script holds up every frame (the script is not ported): the Crane's
    // arm/hook bodies (Containers, Crane) and the Gondola base would otherwise just fall. Freeze them in
    // place (kinematic) so what hangs from them (containers, cables, the platform) stays up.
    // `GB_NO_HOLD=1` disables.
    if std::env::var_os("GB_NO_HOLD").is_none() {
        let inst = sim.scenes[0].0;
        let mut frozen = 0;
        let mut frozen_bodies: HashSet<usize> = HashSet::new();
        let mut freeze = |sim: &mut Sim, node: usize| {
            if let Some(&body) = sim.world.instances[inst].bodies.get(&node) {
                sim.world.set_kinematic(body, true);
                frozen += 1;
                frozen_bodies.insert(body);
            }
        };
        for (i, node) in nodes.iter().enumerate() {
            if let Some(d) = script_of(node, "Crane") {
                for key in ["machineDeck", "mainJib", "pistonStart", "pistonMiddle", "pistonEnd", "armTip", "hook", "lowerHalf"] {
                    if let Some(n) = d[key]["node"].as_u64() {
                        freeze(sim, n as usize);
                    }
                }
            }
            // Cable segments of the frozen platforms: left dynamic they tear apart against the held ends.
            if script_of(node, "Gondola_Cable").is_some() || script_of(node, "RopeBreak").is_some() {
                freeze(sim, i);
            }
            // Trawler_Mechanics keeps the hull afloat at `floatHeight` every frame: hold it where it was saved.
            if let Some(d) = script_of(node, "Trawler_Mechanics") {
                if let Some(n) = d["trawlerHull"]["node"].as_u64() {
                    freeze(sim, n as usize);
                }
            }
            if node.path.ends_with("gondola_base") || node.path.ends_with("containerFrameLower") || node.path.ends_with("containerFrameUpper") {
                freeze(sim, i);
            }
        }
        if frozen > 0 {
            info!("stage events: {frozen} scripted body(ies) held in place");
        }
        // Crane_RandomPointMover: driven cranes slide their whole (held) assembly along the gantry rails.
        let poses_c = sim.scenes[0].1.world_poses(gb_phys::Pose::IDENTITY);
        let movers: Vec<serde_json::Value> = nodes
            .iter()
            .flat_map(|n| n.components.iter().filter(|c| c.script.as_deref() == Some("Crane_RandomPointMover")))
            .map(|c| c.data.clone())
            .collect();
        for mover in &movers {
            let Some(root) = mover["crane"]["node"].as_u64().map(|n| n as usize) else { continue };
            let mut bodies = Vec::new();
            for (j, _) in nodes.iter().enumerate() {
                let mut cur = Some(j);
                let mut under = false;
                while let Some(c) = cur {
                    if c == root {
                        under = true;
                        break;
                    }
                    cur = nodes[c].parent;
                }
                if !under {
                    continue;
                }
                if let Some(&b) = sim.world.instances[inst].bodies.get(&j) {
                    if frozen_bodies.contains(&b) {
                        bodies.push((b, sim.world.pose(b)));
                    }
                }
            }
            if bodies.is_empty() {
                continue;
            }
            let axis = (poses_c[root].rotation * Vec3::X).normalize_or_zero();
            state.cranes.push(CraneMove {
                bodies,
                axis,
                offset: 0.0,
                target: 0.0,
                max: mover["maxDistanceBaseCanMove"].as_f64().unwrap_or(4.0) as f32,
                speed: mover["baseMovementSpeed"].as_f64().unwrap_or(2.0) as f32,
                delay: (
                    mover["stateChangeDelayMin"].as_f64().unwrap_or(1.0) as f32,
                    mover["stateChangeDelayMax"].as_f64().unwrap_or(20.0) as f32,
                ),
                next: 5.0,
            });
        }
        if !state.cranes.is_empty() {
            info!("stage events: {} driven crane(s)", state.cranes.len());
        }
    }
    // Bendable rails / bridge frames (Bendable|VariableBreakJoint chains) are limit-only joints with no
    // spring: with gravity they sagged 0.3 m at load. The authored scene holds them at rest, so keep gravity
    // off for those bones; they still bend and break when beasts hit them. `GB_BEND_GRAVITY=1` restores it.
    if std::env::var_os("GB_BEND_GRAVITY").is_none() {
        let inst = sim.scenes[0].0;
        let mut held = 0;
        for (i, node) in nodes.iter().enumerate() {
            if script_of(node, "BendableBreakJoint").is_some() || script_of(node, "VariableBreakJoint").is_some() {
                if let Some(&body) = sim.world.instances[inst].bodies.get(&i) {
                    sim.world.set_use_gravity(body, false);
                    sim.world.set_angular_drag(body, 4.0);
                    held += 1;
                }
            }
        }
        if held > 0 {
            info!("stage events: {held} bendable bone(s) held against gravity");
        }
    }
    for node in nodes.iter() {
        if let Some(d) = script_of(node, "FanController") {
            let Some(vol) = d["_beastEffectorVolume"]["node"].as_u64().map(|n| n as usize) else { continue };
            let (Some((center, half)), Some(&trigger)) = (collider_box(&nodes[vol]), map.0.get(&(0, vol))) else { continue };
            state.fans.push(Fan {
                trigger,
                center,
                half,
                thrust: d["_beastVolumeThrustMagnitude"].as_f64().unwrap_or(0.7) as f32,
                spin: d["FanSpinTime"].as_f64().unwrap_or(16.0) as f32,
                windup: d["FanWindupTime"].as_f64().unwrap_or(2.0) as f32,
                off: d["FanDisabledTime"].as_f64().unwrap_or(20.0) as f32,
                clock: 0.0,
            });
        }
    }
    if !state.fans.is_empty() {
        info!("stage events: {} fan(s)", state.fans.len());
    }
    // Train landslide.
    if let Some(tp) = nodes.iter().find_map(|n| script_of(n, "TrackPool")) {
        let inst = sim.scenes[0].0;
        let spawns: Vec<Entity> = tp["landSlideBoulderSpawns"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["node"].as_u64())
            .filter_map(|n| map.0.get(&(0, n as usize)).copied())
            .collect();
        let mut items = Vec::new();
        let poses_b = sim.scenes[0].1.world_poses(gb_phys::Pose::IDENTITY);
        for pool in tp["boulderPool"].as_array().into_iter().flatten() {
            let Some(pn) = pool["node"].as_u64().map(|n| n as usize) else { continue };
            let Some(pd) = script_of(&nodes[pn], "Pool") else { continue };
            for p in pd["_Pool"].as_array().into_iter().flatten() {
                let Some(n) = p["node"].as_u64().map(|n| n as usize) else { continue };
                let mut bodies = Vec::new();
                for (j, _) in nodes.iter().enumerate() {
                    let mut cur = Some(j);
                    let mut under = false;
                    while let Some(c) = cur {
                        if c == n {
                            under = true;
                            break;
                        }
                        cur = nodes[c].parent;
                    }
                    if under {
                        if let Some(&b) = sim.world.instances[inst].bodies.get(&j) {
                            bodies.push((b, sim.world.pose(b)));
                        }
                    }
                }
                if let (false, Some(&e)) = (bodies.is_empty(), map.0.get(&(0, n))) {
                    items.push(PropItem { entity: e, bodies, root_pos: poses_b[n].position, live: false, age: 0.0 });
                }
            }
        }
        if !items.is_empty() && !spawns.is_empty() {
            let lo = tp["startEscalationRange"]["x"].as_f64().unwrap_or(30.0) as f32;
            let hi = tp["startEscalationRange"]["y"].as_f64().unwrap_or(120.0) as f32;
            let dur = tp["escalationDurationRange"]["x"].as_f64().unwrap_or(120.0) as f32;
            let start = lo + state.rand01() * (hi - lo);
            info!("stage events: landslide in {start:.0}s for {dur:.0}s ({} boulders)", items.len());
            state.boulders = Some(Boulders { spawns, items, start, end: start + dur, next: 0.0 });
        }
    }
    // Sharks and the Trawler capsize.
    {
        let inst = sim.scenes[0].0;
        let poses_s = sim.scenes[0].1.world_poses(gb_phys::Pose::IDENTITY);
        for (i, node) in nodes.iter().enumerate() {
            if let Some(c) = script_of(node, "Gondola_Cable") {
                if let Some(&body) = sim.world.instances[inst].bodies.get(&i) {
                    state.cables.push(Cable { body, health: c["health"].as_f64().unwrap_or(20.0) as f32 });
                }
            }
            let Some(shark) = script_of(node, "SharkActor") else { continue };
            let mut bodies = Vec::new();
            for (j, _) in nodes.iter().enumerate() {
                let mut cur = Some(j);
                let mut under = false;
                while let Some(c) = cur {
                    if c == i {
                        under = true;
                        break;
                    }
                    cur = nodes[c].parent;
                }
                if under {
                    if let Some(&b) = sim.world.instances[inst].bodies.get(&j) {
                        bodies.push((b, sim.world.pose(b)));
                    }
                }
            }
            if bodies.is_empty() {
                continue;
            }
            for (b, _) in &bodies {
                sim.world.set_kinematic(*b, true);
            }
            // The retail Buoy stage has no sharks (the scene still carries them): park them far below.
            if sim.stage_name == "buoy" {
                for (b, _) in &bodies {
                    sim.world.move_kinematic(*b, Iso::new(Vec3::new(0.0, -3000.0, 0.0), Quat::IDENTITY));
                }
                continue;
            }
            let node_pos = |key: &str| -> Vec<Vec3> {
                shark[key]
                    .as_array()
                    .map(|a| a.iter().filter_map(|n| n["node"].as_u64()).filter_map(|n| poses_s.get(n as usize)).map(|p| p.position).collect())
                    .unwrap_or_default()
            };
            let part = |key: &str| shark[key]["node"].as_u64().and_then(|n| poses_s.get(n as usize)).map(|p| p.position);
            let centre = part("torso").unwrap_or(poses_s[i].position);
            let (head, tail) = (part("head"), part("tailEnd"));
            let fwd0 = match (head, tail) {
                (Some(h), Some(t)) if (h - t).length() > 0.1 => (h - t).normalize(),
                _ => Vec3::Z,
            };
            let mouth = head.map_or(2.0, |h| (h - centre).length() + 0.5);
            let f = |k: &str, d: f32| shark[k].as_f64().unwrap_or(d as f64) as f32;
            let r = state.rand01();
            state.sharks.push(Shark {
                bodies,
                centre,
                fwd0,
                mouth,
                search: node_pos("searchNodes"),
                retreat: node_pos("retreatNodes"),
                dive: node_pos("diveNodes"),
                sleep: (f("minSleepingTime", 60.0), f("maxSleepingTime", 120.0)),
                searching: (f("minSearchingTime", 5.0), f("maxSearchingTime", 10.0)),
                attacking: (f("minAttackingTime", 5.0), f("maxAttackingTime", 10.0)),
                breach_depth: f("breachDepth", -8.0),
                breach_height: f("breachHeight", 8.0),
                speed: f("maxVelocity", 10.0),
                state: SharkState::Sleeping,
                timer: 0.0,
                state_len: 0.0,
                pos: centre,
                dir: fwd0,
                node: 0,
                target: None,
                held: Vec::new(),
                arc: None,
                started: false,
                start_at: f("startDelay", 60.0) * (0.9 + 0.2 * r),
                health: f("health", 100.0),
                max_health: f("maxHealth", 100.0),
                jaw: shark["jaw"]["node"].as_u64().and_then(|n| {
                    let body = *sim.world.instances[inst].bodies.get(&(n as usize))?;
                    Some((body, poses_s.get(n as usize)?.position))
                }),
                jaw_open: 0.0,
            });
        }
        if !state.sharks.is_empty() {
            info!("stage events: {} shark(s)", state.sharks.len());
        }
        if let Some(d) = nodes.iter().find_map(|n| script_of(n, "Trawler_Mechanics")) {
            if let Some(h) = d["trawlerHull"]["node"].as_u64() {
                if let Some(&hull) = sim.world.instances[inst].bodies.get(&(h as usize)) {
                    let delay = d["sinkDelay"].as_f64().unwrap_or(150.0) as f32;
                    state.capsize = Some(Capsize { hull, pose: sim.world.pose(hull), start: delay });
                    info!("stage events: trawler capsizes after {delay}s");
                }
            }
        }
    }
    // Elevator cars become kinematic shuttles.
    for node in nodes.iter() {
        if let Some(d) = script_of(node, "Elevators_Car") {
            let inst = sim.scenes[0].0;
            let Some(carn) = d["car"]["node"].as_u64() else { continue };
            let Some(&body) = sim.world.instances[inst].bodies.get(&(carn as usize)) else { continue };
            let floors: Vec<f32> = d["floors"].as_array().into_iter().flatten().filter_map(|v| v.as_f64()).map(|v| v as f32).collect();
            sim.world.set_kinematic(body, true);
            let pose = sim.world.pose(body);
            // The rope segments under this car's `Cables` groups hang straight; as free dynamic chains they sagged
            // and bent when the car moved, so they stay put until the cable snaps.
            let mut cables = Vec::new();
            for (j, cn) in nodes.iter().enumerate() {
                if script_of(cn, "Elevators_Cable").is_none() {
                    continue;
                }
                let mut cur = cn.parent;
                let mut under = false;
                while let Some(c) = cur {
                    if Some(c) == d["m_GameObject"]["node"].as_u64().map(|v| v as usize) {
                        under = true;
                        break;
                    }
                    cur = nodes[c].parent;
                }
                if under {
                    if let Some(&b) = sim.world.instances[inst].bodies.get(&j) {
                        sim.world.set_kinematic(b, true);
                        cables.push(b);
                    }
                }
            }
            state.lifts.push(Lift { body, pose, start_y: pose.position.y, floors, index: 1, going_up: true, wait: 3.0, cables });
        }
    }
    if !state.lifts.is_empty() {
        if let Some(d) = nodes.iter().find_map(|n| script_of(n, "Elevators_Logic")) {
            let lo = d["minMalfunctionTime"].as_f64().unwrap_or(120.0) as f32;
            let hi = d["maxMalfunctionTime"].as_f64().unwrap_or(240.0) as f32;
            let at = lo + state.rand01() * (hi - lo);
            let car = (state.rand01() * state.lifts.len() as f32) as usize % state.lifts.len();
            state.lift_failure = Some((at, car));
            info!("stage events: elevator {car} fails at {at:.0}s");
        }
        info!("stage events: {} elevator car(s)", state.lifts.len());
    }
    // Train stage: straight track pieces scroll; the train itself is held at the origin.
    {
        let inst = sim.scenes[0].0;
        let poses_t = sim.scenes[0].1.world_poses(gb_phys::Pose::IDENTITY);
        if let Some(tp) = nodes.iter().find_map(|n| script_of(n, "TrackPool")) {
            let speed = tp["trackMovementSpeed"].as_f64().unwrap_or(12.0) as f32;
            let mut pieces: Vec<TrackPiece> = Vec::new();
            for (kind, key) in [(0u8, "trackPoolStright"), (1u8, "trackPoolLeft"), (2u8, "trackPoolRight")] {
                let pool_node = tp[key]["node"].as_u64().map(|n| n as usize);
                let Some(pool) = pool_node.and_then(|p| script_of(&nodes[p], "Pool")) else { continue };
                for p in pool["_Pool"].as_array().into_iter().flatten() {
                    let Some(n) = p["node"].as_u64().map(|n| n as usize) else { continue };
                    let mut bodies = Vec::new();
                    for (j, _) in nodes.iter().enumerate() {
                        let mut cur = Some(j);
                        let mut under = false;
                        while let Some(c) = cur {
                            if c == n {
                                under = true;
                                break;
                            }
                            cur = nodes[c].parent;
                        }
                        if under {
                            if let Some(&b) = sim.world.instances[inst].bodies.get(&j) {
                                sim.world.set_kinematic(b, true);
                                bodies.push((b, sim.world.pose(b)));
                            }
                        }
                    }
                    // The piece's TrackNode chain gives its centre line.
                    let mut poly: Vec<(f32, f32)> = script_of(&nodes[n], "TrackMover")
                        .map(|m| {
                            m["_connections"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|c| c["node"].as_u64())
                                .filter_map(|k| poses_t.get(k as usize))
                                .map(|p| (p.position.x, p.position.z))
                                .collect()
                        })
                        .unwrap_or_default();
                    poly.sort_by(|a, b| a.1.total_cmp(&b.1));
                    let dx = match poly.as_slice() {
                        [.., a, b] if (b.1 - a.1).abs() > 1e-3 => b.0 + (b.0 - a.0) / (b.1 - a.1) * (100.0 - b.1),
                        _ => 0.0,
                    };
                    if let (false, Some(&e)) = (bodies.is_empty(), map.0.get(&(0, n))) {
                        pieces.push(TrackPiece { entity: e, bodies, kind, poly, dx });
                    }
                }
            }
            // Start with five straight sections (the train's starting stretch), then wander.
            let mut active: Vec<ActivePiece> = Vec::new();
            let mut sx = 0.0;
            for k in 0..8 {
                let want = if k < 5 { 0 } else if k % 2 == 0 { 1 } else { 2 };
                let idx = (0..pieces.len()).find(|i| pieces[*i].kind == want && !active.iter().any(|a| a.idx == *i));
                let Some(idx) = idx.or_else(|| (0..pieces.len()).find(|i| !active.iter().any(|a| a.idx == *i))) else { break };
                active.push(ActivePiece { idx, sx, sz: (k as f32 - 2.0) * 100.0 });
                sx += pieces[idx].dx;
            }
            // Pieces that are not on the track wait out of sight. Pooled items ship parked (out of the simulation), so the
            // ones on the track are put back first.
            for (i, piece) in pieces.iter().enumerate() {
                let on = active.iter().any(|a| a.idx == i);
                for (body, _) in &piece.bodies {
                    sim.world.restore_body(*body);
                    if !on {
                        sim.world.move_kinematic(*body, Iso::new(Vec3::new(0.0, -3000.0, 0.0), Quat::IDENTITY));
                    }
                }
                if let Ok(mut v) = visibility.get_mut(piece.entity) {
                    *v = if on { Visibility::Visible } else { Visibility::Hidden };
                }
            }
            if !pieces.is_empty() {
                info!("stage events: {} track piece(s) ({} on the track) at {speed} m/s", pieces.len(), active.len());
                state.track = Some(Track { pieces, active, speed, length: 100.0, ref_z: -30.0 });
            }
            // The train (cars + bogies) sits still on the origin; its NodeFollower forces are not ported.
            let mut held = 0;
            if let Some(train_root) = nodes.iter().position(|n| n.path == "Train") {
                for (j, _) in nodes.iter().enumerate() {
                    let mut cur = Some(j);
                    let mut under = false;
                    while let Some(c) = cur {
                        if c == train_root {
                            under = true;
                            break;
                        }
                        cur = nodes[c].parent;
                    }
                    if under {
                        if let Some(&b) = sim.world.instances[inst].bodies.get(&j) {
                            sim.world.set_kinematic(b, true);
                            held += 1;
                        }
                    }
                }
            }
            if held > 0 {
                info!("stage events: {held} train body(ies) held");
            }
        }
    }
    // Park every pooled (Clone) body (the scene saves them in the world), then register prop spawners.
    {
        let inst = sim.scenes[0].0;
        let mut parked = 0;
        for (i, node) in nodes.iter().enumerate() {
            // Any body at or below a pooled "(Clone)" root (Chute's meat keeps its body on a child bone).
            let pooled = node.path.split('/').any(|n| n.contains("(Clone)"))
                && !node.path.contains("TrackPool (Stright)");
            if pooled {
                if let Some(&body) = sim.world.instances[inst].bodies.get(&i) {
                    sim.world.remove_body(body);
                    parked += 1;
                }
            }
        }
        let poses_all = sim.scenes[0].1.world_poses(gb_phys::Pose::IDENTITY);
        for (i, node) in nodes.iter().enumerate() {
            let Some(spawner) = script_of(node, "PoolSpawner") else { continue };
            let pool = script_of(node, "Pool").or_else(|| script_of(node, "SyncPool"));
            let (Some(pool), Some(&entity)) = (pool, map.0.get(&(0, i))) else { continue };
            let mut items = Vec::new();
            let mut is_train = false;
            for p in pool["_Pool"].as_array().into_iter().flatten() {
                let Some(n) = p["node"].as_u64().map(|n| n as usize) else { continue };
                if script_of(&nodes[n], "Train").is_some() {
                    is_train = true;
                }
                let mut bodies = Vec::new();
                for (j, d) in nodes.iter().enumerate() {
                    let mut cur = Some(j);
                    let mut under = false;
                    while let Some(c) = cur {
                        if c == n {
                            under = true;
                            break;
                        }
                        cur = nodes[c].parent;
                    }
                    if under {
                        if let Some(&b) = sim.world.instances[inst].bodies.get(&j) {
                            let _ = d;
                            bodies.push((b, sim.world.pose(b)));
                        }
                    }
                }
                if let (false, Some(&e)) = (bodies.is_empty(), map.0.get(&(0, n))) {
                    items.push(PropItem { entity: e, bodies, root_pos: poses_all[n].position, live: false, age: 0.0 });
                }
            }
            if is_train || items.is_empty() {
                continue;
            }
            let (delay_min, delay_max) = (
                spawner["DelayMin"].as_f64().unwrap_or(3.0) as f32,
                spawner["DelayMax"].as_f64().unwrap_or(8.0) as f32,
            );
            let r = state.rand01();
            state.props.push(PropSpawner {
                entity,
                items,
                delay_min,
                delay_max,
                offset: vec3(&spawner["RandomSpawnOffset"]),
                since: 0.0,
                next: delay_min + (delay_max - delay_min).max(0.0) * r,
            });
        }
        if parked > 0 || !state.props.is_empty() {
            info!("stage events: parked {parked} pooled bodies, {} prop spawner(s)", state.props.len());
        }
    }
    for node in nodes.iter() {
        if let Some(d) = script_of(node, "OnTriggerStayApplyForce") {
            // validGameMode is a GameModeEnum mask: -1 = every mode, otherwise Melee (1) must be set.
            let mask = d["validGameMode"].as_i64().unwrap_or(-1);
            if mask != -1 && mask & 1 == 0 {
                continue;
            }
            let inst = sim.scenes[0].0;
            let (Some(target), Some((center, half))) = (d["target"]["node"].as_u64(), collider_box(node)) else { continue };
            let Some(&body) = sim.world.instances[inst].bodies.get(&(target as usize)) else { continue };
            let idx = nodes.iter().position(|n| std::ptr::eq(n, node)).unwrap_or(0);
            let Some(&entity) = map.0.get(&(0, idx)) else { continue };
            state.doors.push(Door {
                trigger: entity,
                center,
                half,
                target: body,
                torque: d["forceType"].as_i64() == Some(1),
                open_force: vec3(&d["openForce"]),
                close_force: vec3(&d["closeForce"]),
                wait_open: d["_waitOpenTime"].as_f64().unwrap_or(0.5) as f32,
                wait_close: d["_waitClosedTime"].as_f64().unwrap_or(1.5) as f32,
                inside_time: 0.0,
                outside_time: 0.0,
                open: false,
            });
        }
    }
    // ContainerTinter (Trucks/Train): each tinter picks one of its `colors` per match and sets it on materialA and
    // materialB. The exporter names those "Container A/B/C Shade A/B"; tinter k drives container letter k.
    let tinters: Vec<Vec<Color>> = nodes
        .iter()
        .flat_map(|n| n.components.iter().filter(|c| c.script.as_deref() == Some("ContainerTinter")))
        .map(|c| {
            c.data["colors"].as_array().into_iter().flatten().map(|v| {
                let f = |k: &str| v[k].as_f64().unwrap_or(1.0) as f32;
                Color::srgb(f("r"), f("g"), f("b"))
            }).collect()
        })
        .collect();
    for (k, palette) in tinters.iter().enumerate() {
        if palette.is_empty() {
            continue;
        }
        let pick = palette[(state.rand01() * palette.len() as f32) as usize % palette.len()];
        state.container_tints.push((format!("Container {} Shade", (b'A' + k as u8) as char), pick));
    }
    // TruckWheels: both wheels spin about their local X at `speed` deg/s (4000 = 40 m/s road on 0.6 m wheels).
    for node in nodes.iter() {
        if let Some(d) = script_of(node, "TruckWheels") {
            let speed = d["speed"].as_f64().unwrap_or(0.0) as f32;
            for key in ["leftWheel", "rightWheel"] {
                if let Some(&e) = d[key]["node"].as_u64().and_then(|n| map.0.get(&(0, n as usize))) {
                    state.spinners.push((e, Vec3::new(speed, 0.0, 0.0)));
                }
            }
        }
    }
    let poses = sim.scenes[0].1.world_poses(gb_phys::Pose::IDENTITY);
    for (i, node) in nodes.iter().enumerate() {
        if script_of(node, "Liquid").is_some() && state.water_level.is_none() {
            state.water_level = Some(poses[i].position.y);
        }
        if let Some(d) = script_of(node, "SimpleBuoyancy") {
            if d["IsEnabled"].as_u64().unwrap_or(1) != 0 {
                if let Some(&body) = sim.world.instances[sim.scenes[0].0].bodies.get(&i) {
                    state.floaters.push(Floater {
                        body,
                        force: d["force"].as_f64().unwrap_or(1.0) as f32,
                        falloff: d["forceFalloff"].as_f64().unwrap_or(0.5) as f32,
                        // Authored height from the scene (physics has already sagged a few frames by now).
                        rest_y: poses[i].position.y,
                        bias: 0.0,
                    });
                }
            }
        }
    }
    for (i, node) in nodes.iter().enumerate() {
        let inst = sim.scenes[0].0;
        if let Some(d) = script_of(node, "Road") {
            let (Some(a), Some(b)) = (d["start"]["node"].as_u64(), d["end"]["node"].as_u64()) else { continue };
            let (Some(pa), Some(pb)) = (poses.get(a as usize), poses.get(b as usize)) else { continue };
            if let Some(&body) = sim.world.instances[inst].bodies.get(&i) {
                let pose = sim.world.pose(body);
                state.roads.push(Road {
                    body,
                    pose,
                    // The treadmill scrolls the opposite way to the authored start->end.
                    start: mirror_position(pb.position),
                    end: mirror_position(pa.position),
                    speed: d["speed"].as_f64().unwrap_or(0.0) as f32,
                });
            }
        }
        if let Some(d) = script_of(node, "TruckBase") {
            if let Some(&body) = sim.world.instances[inst].bodies.get(&i) {
                state.trucks.push(Truck {
                    body,
                    start: sim.world.pose(body).position,
                    min: vec3(&d["min"]),
                    max: vec3(&d["max"]),
                    force: d["force"].as_f64().unwrap_or(0.4) as f32,
                    out_of_bounds: false,
                    direction: Vec3::ZERO,
                    next_direction: 0.0,
                });
            }
        }
    }
    for (i, node) in nodes.iter().enumerate() {
        if let Some(d) = script_of(node, "WheelRotator") {
            if let Some(&body) = sim.world.instances[sim.scenes[0].0].bodies.get(&i) {
                let pose = sim.world.pose(body);
                state.wheels.push(Wheel {
                    body,
                    pose,
                    speed: 0.0,
                    state: 0.0,
                    // secondsUntilStartEsclationMin/Max: calm turning before the first escalation.
                    timer: d["secondsUntilStartEsclationMin"].as_f64().unwrap_or(30.0) as f32,
                    max_speed: d["MaxSpeed"].as_f64().unwrap_or(95.0) as f32,
                });
            }
        }
    }
    if !state.roads.is_empty() || !state.trucks.is_empty() {
        info!("stage events: {} road tile(s), {} truck(s)", state.roads.len(), state.trucks.len());
    }
    if !state.doors.is_empty() {
        info!("stage events: {} force door(s)", state.doors.len());
    }
    if !state.floaters.is_empty() {
        info!("stage events: {} floating body(ies), water level {:?}", state.floaters.len(), state.water_level);
    }
    if !state.wheels.is_empty() {
        info!("stage events: {} wheel axle(s)", state.wheels.len());
    }
    if !state.trains.is_empty() {
        info!(
            "stage events: {} spawner(s), {} train(s), {} trigger(s), {} despawn zone(s)",
            state.spawners.len(),
            state.trains.len(),
            state.triggers.len(),
            state.zones.len()
        );
    }
}

/// Debug (`GB_DRIFT_REPORT=1`): once, ~2 s in, print the stage bodies that moved furthest from their
/// authored pose (joint/solver problems show up here before they look like deformed railings).
pub fn drift_report(sim: NonSend<Sim>, mut done: Local<bool>) {
    if *done || std::env::var_os("GB_DRIFT_REPORT").is_none() || sim.world.steps < 100 {
        return;
    }
    *done = true;
    let (inst, src, _) = &sim.scenes[0];
    let poses = src.world_poses(gb_phys::Pose::IDENTITY);
    let mut moved: Vec<(f32, usize, Vec3, Vec3)> = Vec::new();
    for (node, body) in &sim.world.instances[*inst].bodies {
        let expected = poses[*node].position;
        let pose = sim.world.pose(*body);
        let actual = pose.position;
        // Rotation error (degrees) counts as drift too: a few degrees on a bone chain bends a skinned rail.
        let angle = pose.rotation.angle_between(poses[*node].rotation).to_degrees();
        moved.push(((actual - expected).length() + angle * 0.01, *node, expected, actual));
    }
    moved.sort_by(|a, b| b.0.total_cmp(&a.0));
    if let Ok(filter) = std::env::var("GB_DRIFT_FILTER") {
        moved.retain(|(_, n, _, _)| src.nodes[*n].path.contains(filter.as_str()));
    }
    info!("drift report: {} bodies, top movers:", moved.len());
    for (d, node, e, a) in moved.iter().take(10) {
        info!("  node {node} {} moved score {d:.3}  expected {e:?} actual {a:?}", src.nodes[*node].path.rsplit('/').next().unwrap_or(""));
    }
}
