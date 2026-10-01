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

/// The Train stage's endless track (`TrackPool` 12 m/s, `TrackMover`, pieces `trackSectionOffset` 100 m long): the
/// train stays at the origin while the straight pieces scroll underneath and recycle at the front. Simplified: only
/// the straight pool is used (the turn pieces and boulder landslides are not ported).
struct Track {
    pieces: Vec<TrackPiece>,
    speed: f32,
    length: f32,
}

struct TrackPiece {
    entity: Entity,
    bodies: Vec<(usize, Iso)>,
    z: f32,
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
    key: (usize, usize),
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

pub fn stage_events(
    mut sim: NonSendMut<Sim>,
    map: Res<NodeEntities>,
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    mut state: Local<StageEvents>,
    mut transforms: Query<&mut Transform>,
    globals: Query<&GlobalTransform>,
    mut visibility: Query<&mut Visibility>,
) {
    if sim.lobby {
        return;
    }
    let Some((_, stage, _)) = sim.scenes.first() else { return };
    let key = (stage.nodes.len(), sim.world.instances.len());
    if state.key != key {
        *state = StageEvents { key, rng: 0x9E3779B9 ^ key.0 as u32, ..Default::default() };
    }
    if !state.ready {
        let n = sim.scenes[0].1.nodes.len();
        if !scene_mapping_complete(map.0.keys().copied(), 0, n) {
            return;
        }
        state.ready = true;
        init(&mut sim, &map, &mut state, &mut visibility);
    }
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
    // Scrolling track pieces.
    if let Some(track) = &mut state.track {
        let total = track.length * track.pieces.len() as f32;
        let back = -track.length * 2.5;
        for piece in &mut track.pieces {
            piece.z -= track.speed * dt;
            if piece.z < back {
                piece.z += total;
            }
            for (body, rest) in &piece.bodies {
                let p = Iso::new(rest.position + Vec3::new(0.0, 0.0, piece.z), rest.rotation);
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
            let rot = Quat::from_euler(EulerRot::YXZ, r[3] * 6.283, r[4] * 6.283, r[5] * 6.283);
            let item = &mut sp.items[ii];
            let _ = rot;
            for (body, rest) in &item.bodies {
                sim.world.restore_body(*body);
                sim.world.teleport(*body, Iso::new(pos + (rest.position - item.root_pos), rest.rotation));
            }
            item.live = true;
            item.age = 0.0;
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
    // Floating bodies (buoys, ice, crane containers).
    if let Some(level) = state.water_level {
        let step_scale = dt / fixed.timestep().as_secs_f32();
        for f in &state.floaters {
            // The source law (acceleration = force*2*depth/falloff below the surface) settles each body some
            // distance under its authored height (pivot vs collider bounds are not the same point), so buoys,
            // ice and the Trawler hull sat too low. Use the same stiffness as a spring about the authored rest
            // height: a = g + k * (rest - y) - c * vy, which holds the saved pose and still bobs when pushed.
            let pose_y = sim.world.pose(f.body).position.y;
            let vy = sim.world.linear_velocity(f.body).y;
            let k = f.force * 2.0 / f.falloff.max(0.1);
            let g = 20.0;
            let _ = level;
            let a = (g + k * (f.rest_y - pose_y) - 2.0 * k.sqrt() * 0.6 * vy).clamp(0.0, 80.0);
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
    // Dynamic assemblies that a stage script holds up every frame (the script is not ported): the Crane's
    // arm/hook bodies (Containers, Crane) and the Gondola base would otherwise just fall. Freeze them in
    // place (kinematic) so what hangs from them (containers, cables, the platform) stays up.
    // `GB_NO_HOLD=1` disables.
    if std::env::var_os("GB_NO_HOLD").is_none() {
        let inst = sim.scenes[0].0;
        let mut frozen = 0;
        let mut freeze = |sim: &mut Sim, node: usize| {
            if let Some(&body) = sim.world.instances[inst].bodies.get(&node) {
                sim.world.set_kinematic(body, true);
                frozen += 1;
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
            if node.path.ends_with("gondola_base") || node.path.ends_with("containerFrameLower") || node.path.ends_with("containerFrameUpper") {
                freeze(sim, i);
            }
        }
        if frozen > 0 {
            info!("stage events: {frozen} scripted body(ies) held in place");
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
    // Train stage: straight track pieces scroll; the train itself is held at the origin.
    {
        let inst = sim.scenes[0].0;
        let poses_t = sim.scenes[0].1.world_poses(gb_phys::Pose::IDENTITY);
        if let Some(tp) = nodes.iter().find_map(|n| script_of(n, "TrackPool")) {
            let speed = tp["trackMovementSpeed"].as_f64().unwrap_or(12.0) as f32;
            let pool_node = tp["trackPoolStright"]["node"].as_u64().map(|n| n as usize);
            let mut pieces = Vec::new();
            if let Some(pool) = pool_node.and_then(|p| script_of(&nodes[p], "Pool")) {
                for (k, p) in pool["_Pool"].as_array().into_iter().flatten().enumerate() {
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
                    if let (false, Some(&e)) = (bodies.is_empty(), map.0.get(&(0, n))) {
                        pieces.push(TrackPiece { entity: e, bodies, z: (k as f32 - 2.0) * 100.0 });
                    }
                }
            }
            let _ = &poses_t;
            if !pieces.is_empty() {
                for piece in &pieces {
                    if let Ok(mut v) = visibility.get_mut(piece.entity) {
                        *v = Visibility::Visible;
                    }
                }
                info!("stage events: {} scrolling track piece(s) at {speed} m/s", pieces.len());
                state.track = Some(Track { pieces, speed, length: 100.0 });
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
                        rest_y: sim.world.pose(body).position.y,
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
                    start: mirror_position(pa.position),
                    end: mirror_position(pb.position),
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
                    timer: 0.0,
                    max_speed: d["MaxSpeed"].as_f64().unwrap_or(95.0) as f32,
                });
            }
        }
    }
    let _ = state.trains.iter().map(|t| t.node).count();
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
    info!("drift report: {} bodies, top movers:", moved.len());
    for (d, node, e, a) in moved.iter().take(10) {
        info!("  node {node} {} moved score {d:.3}  expected {e:?} actual {a:?}", src.nodes[*node].path.rsplit('/').next().unwrap_or(""));
    }
}
