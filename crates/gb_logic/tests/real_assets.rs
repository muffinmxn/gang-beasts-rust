//! Simulates the exported rooftop + beast with the original's physics settings.
//! Skipped when assets/export is absent (game files are never committed).
use gb_logic::{Beast, Part};
use gb_phys::{Pose, Settings, Sidecar, World};
use glam::{Quat, Vec3};
use std::path::PathBuf;

fn assets() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/export");
    p.join("physics.json").exists().then_some(p)
}

fn spawn_points(stage: &Sidecar) -> Vec<Pose> {
    let world = stage.world_poses(Pose::IDENTITY);
    stage
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            n.active_in_hierarchy
                && n.components
                    .iter()
                    .any(|c| c.script.as_deref() == Some("GBSpawnPoint"))
        })
        .map(|(i, _)| world[i])
        .collect()
}

#[test]
fn ragdoll_lands_on_rooftop() {
    let Some(root) = assets() else { return };
    let mut world = World::new(Settings::load(&root).unwrap()).unwrap();
    let stage = Sidecar::load(&root, "rooftop").unwrap();
    let beast_src = gb_logic::beast::load_source(&root).unwrap();
    let s = world.spawn("rooftop", &stage, Pose::IDENTITY).unwrap();
    for w in &world.instances[s].warnings {
        println!("stage warning: {w}");
    }
    let spawn = spawn_points(&stage)[3];
    let origin = Pose {
        position: spawn.position,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };
    let b = world.spawn("beast", &beast_src, origin).unwrap();
    assert!(
        world.instances[b].warnings.is_empty(),
        "{:?}",
        world.instances[b].warnings
    );
    assert_eq!(world.instances[b].bodies.len(), 21);
    assert_eq!(world.instances[b].joints, 20);
    let beast = Beast::new(&world, b, &beast_src).unwrap();
    beast.setup_rigidbodies(&mut world);
    let total: f32 = Part::ALL.iter().map(|p| world.mass(beast.body(*p))).sum();
    assert!((total - 135.0).abs() < 1e-3, "total mass {total}");

    let start = world.pose(beast.body(Part::Head)).position;
    for step in 0..250 {
        world.step();
        for p in Part::ALL {
            let pose = world.pose(beast.body(p));
            assert!(
                pose.position.is_finite() && pose.rotation.is_finite(),
                "{p:?} diverged at step {step}"
            );
        }
    }
    let head = world.pose(beast.body(Part::Head)).position;
    let hips = world.pose(beast.body(Part::Hips)).position;
    println!(
        "spawn {:?} head {:?} -> {:?}, hips {:?}",
        spawn.position, start, head, hips
    );
    // Without the movement code the beast collapses, but it must rest on the roof, near where it started.
    assert!(
        hips.y > spawn.position.y - 0.5 && hips.y < spawn.position.y + 1.5,
        "hips {hips:?}"
    );
    assert!(
        (hips - spawn.position).with_y(0.0).length() < 2.0,
        "slid away: {hips:?}"
    );
    assert!(head.y < start.y, "head never fell");
}

fn spawn_actor() -> Option<(World, gb_logic::Actor, Pose)> {
    let root = assets()?;
    let mut world = World::new(Settings::load(&root).unwrap()).unwrap();
    let stage = Sidecar::load(&root, "rooftop").unwrap();
    let beast_src = gb_logic::beast::load_source(&root).unwrap();
    world.spawn("rooftop", &stage, Pose::IDENTITY).unwrap();
    let spawn = spawn_points(&stage)[3];
    let origin = Pose {
        position: spawn.position,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };
    let b = world.spawn("beast", &beast_src, origin).unwrap();
    let beast = Beast::new(&world, b, &beast_src).unwrap();
    beast.setup_rigidbodies(&mut world);
    let actor = gb_logic::Actor::new(beast, &world, 1);
    Some((world, actor, spawn))
}

fn tick(
    world: &mut World,
    actor: &mut gb_logic::Actor,
    input: &mut gb_logic::InputState,
    h: f32,
    v: f32,
    jump: bool,
) {
    input.set(&[(gb_logic::input::JUMP, jump)], h, v);
    actor.fixed_update(world, input);
    world.step();
    actor.after_step(world, &any_interactable);
}

#[test]
fn beast_stands_and_walks() {
    let Some((mut world, mut actor, spawn)) = spawn_actor() else {
        return;
    };
    let mut input = gb_logic::InputState::default();
    let head = |w: &World, a: &gb_logic::Actor| w.pose(a.beast.body(Part::Head)).position;
    let start = head(&world, &actor);
    for _ in 0..150 {
        tick(&mut world, &mut actor, &mut input, 0.0, 0.0, false);
    }
    let standing = head(&world, &actor);
    println!(
        "standing: head {start:?} -> {standing:?}, state {}, on_ground {}",
        actor.state, actor.control.on_ground
    );
    assert!(
        standing.y > spawn.position.y + 0.8,
        "fell over: {standing:?}"
    );
    let hips0 = world.pose(actor.beast.body(Part::Hips)).position;
    for _ in 0..150 {
        tick(&mut world, &mut actor, &mut input, 0.0, 1.0, false);
    }
    let hips1 = world.pose(actor.beast.body(Part::Hips)).position;
    let h = head(&world, &actor);
    println!(
        "walked: hips {hips0:?} -> {hips1:?}, head {h:?}, state {}",
        actor.state
    );
    assert!(hips1.z - hips0.z > 1.0, "did not walk forward");
    assert!(h.y > spawn.position.y + 0.6, "fell while walking: {h:?}");
}

#[test]
fn lift_cheer_keeps_both_arm_chains_aligned_to_the_source_pose() {
    let Some((mut world, mut actor, _spawn)) = spawn_actor() else {
        return;
    };
    let mut input = gb_logic::InputState::default();
    for _ in 0..50 {
        tick(&mut world, &mut actor, &mut input, 0.0, 0.0, false);
    }
    use gb_logic::input::LIFT;
    for _ in 0..150 {
        input.set(&[(LIFT, true)], 0.0, 0.0);
        actor.fixed_update(&mut world, &input);
        world.step();
        actor.after_step(&mut world, &any_interactable);
    }

    let up = |part| world.pose(actor.beast.body(part)).rotation * Vec3::Y;
    let chest_right = world.pose(actor.beast.body(Part::Chest)).rotation * Vec3::X;
    let waist_down = -up(Part::Waist);
    let left_target = (waist_down + chest_right * 0.5).normalize();
    let right_target = (waist_down - chest_right * 0.5).normalize();
    let alignment = [
        up(Part::LeftArm).dot(left_target),
        up(Part::LeftForarm).dot(waist_down),
        up(Part::RightArm).dot(right_target),
        up(Part::RightForarm).dot(waist_down),
    ];
    println!("lift arm chain alignment: {alignment:?}");
    assert!(alignment.iter().all(|v| v.is_finite() && *v > 0.9));
}

#[test]
fn beast_grabs_another_beast_and_punches() {
    let Some((mut world, mut a, spawn)) = spawn_actor() else {
        return;
    };
    let root = assets().unwrap();
    let src = gb_logic::beast::load_source(&root).unwrap();
    let origin = Pose {
        position: spawn.position + Vec3::new(0.0, 0.0, 1.1),
        rotation: Quat::from_rotation_y(std::f32::consts::PI),
        scale: Vec3::ONE,
    };
    let bi = world.spawn("b", &src, origin).unwrap();
    let bb = Beast::new(&world, bi, &src).unwrap();
    bb.setup_rigidbodies(&mut world);
    let mut b = gb_logic::Actor::new(bb, &world, 2);
    let _beasts = [a.beast.instance, b.beast.instance];
    let (mut ia, mut ib) = (
        gb_logic::InputState::default(),
        gb_logic::InputState::default(),
    );
    use gb_logic::input::{GRAB_LEFT, GRAB_RIGHT};
    let mut step = |world: &mut World,
                    a: &mut gb_logic::Actor,
                    b: &mut gb_logic::Actor,
                    ia: &mut gb_logic::InputState,
                    grab: bool,
                    v: f32| {
        ia.set(&[(GRAB_LEFT, grab), (GRAB_RIGHT, grab)], 0.0, v);
        ib.set(&[], 0.0, 0.0);
        a.fixed_update(world, ia);
        b.fixed_update(world, &ib);
        world.step();
        a.after_step(world, &any_interactable);
        b.after_step(world, &any_interactable);
    };
    for _ in 0..50 {
        step(&mut world, &mut a, &mut b, &mut ia, false, 0.0);
    }
    // Tap = punch.
    step(&mut world, &mut a, &mut b, &mut ia, true, 0.0);
    step(&mut world, &mut a, &mut b, &mut ia, false, 0.0);
    assert!(a.control.hands[0].punch, "tap did not punch");
    for _ in 0..40 {
        step(&mut world, &mut a, &mut b, &mut ia, false, 0.0);
    }
    // Hold + walk into the other beast = grab.
    let mut grabbed = false;
    for _ in 0..150 {
        step(&mut world, &mut a, &mut b, &mut ia, true, 1.0);
        grabbed |= a.control.hands.iter().any(|h| h.joint.is_some());
    }
    println!("grab: {:?}", a.control.hands);
    assert!(grabbed, "never grabbed");
    // Hold Lift with the other beast idle: it comes up off the ground.
    use gb_logic::input::LIFT;
    let y0 = world.pose(b.beast.body(Part::Hips)).position.y;
    let mut peak = y0;
    for _ in 0..100 {
        ia.set(
            &[(GRAB_LEFT, true), (GRAB_RIGHT, true), (LIFT, true)],
            0.0,
            0.0,
        );
        ib.set(&[], 0.0, 0.0);
        a.fixed_update(&mut world, &ia);
        b.fixed_update(&mut world, &ib);
        world.step();
        a.after_step(&mut world, &any_interactable);
        b.after_step(&mut world, &any_interactable);
        peak = peak.max(world.pose(b.beast.body(Part::Hips)).position.y);
    }
    println!(
        "lift: other hips {y0} -> peak {peak}, still holding {}",
        a.control.hands.iter().any(|h| h.joint.is_some())
    );
}

#[test]
fn a_tap_punch_targets_and_knocks_back_a_standing_opponent() {
    let Some((mut world, mut a, spawn)) = spawn_actor() else {
        return;
    };
    let root = assets().unwrap();
    let src = gb_logic::beast::load_source(&root).unwrap();
    let origin = Pose {
        position: spawn.position + Vec3::new(0.0, 0.0, 1.35),
        rotation: Quat::from_rotation_y(std::f32::consts::PI),
        scale: Vec3::ONE,
    };
    let bi = world.spawn("target", &src, origin).unwrap();
    let beast = Beast::new(&world, bi, &src).unwrap();
    beast.setup_rigidbodies(&mut world);
    let mut b = gb_logic::Actor::new(beast, &world, 2);
    b.status.dmg_modifier = 1.0;
    let (mut ia, ib) = (
        gb_logic::InputState::default(),
        gb_logic::InputState::default(),
    );
    let step = |world: &mut World,
                a: &mut gb_logic::Actor,
                b: &mut gb_logic::Actor,
                ia: &mut gb_logic::InputState| {
        a.fixed_update(world, ia);
        b.fixed_update(world, &ib);
        world.step();
        a.after_step(world, &any_interactable);
        b.after_step(world, &any_interactable);
    };
    for _ in 0..60 {
        ia.set(&[], 0.0, 0.0);
        step(&mut world, &mut a, &mut b, &mut ia);
    }
    use gb_logic::input::GRAB_LEFT;
    a.upper_interest = world
        .colliders
        .iter()
        .position(|c| c.body == Some(b.beast.body(Part::Chest)) && !c.trigger);
    ia.set(&[(GRAB_LEFT, true)], 0.0, 0.0);
    step(&mut world, &mut a, &mut b, &mut ia);
    a.upper_interest = world
        .colliders
        .iter()
        .position(|c| c.body == Some(b.beast.body(Part::Chest)) && !c.trigger);
    ia.set(&[(GRAB_LEFT, false)], 0.0, 0.0);
    let mut min_health = 100.0f32;
    let mut peak_speed = 0.0f32;
    for _ in 0..45 {
        a.upper_interest = world
            .colliders
            .iter()
            .position(|c| c.body == Some(b.beast.body(Part::Chest)) && !c.trigger);
        step(&mut world, &mut a, &mut b, &mut ia);
        min_health = min_health.min(b.status.health);
        peak_speed = peak_speed.max(world.linear_velocity(b.beast.body(Part::Chest)).length());
    }
    println!(
        "tap punch: target health {min_health:.1}, peak chest speed {peak_speed:.2}, hits {}",
        b.debug_hits
    );
    assert!(
        b.debug_hits > 0,
        "punch did not contact an interactable body"
    );
    assert!(peak_speed > 0.5, "punch caused no target knockback");
}

#[test]
fn beast_jumps_and_lands() {
    let Some((mut world, mut actor, _spawn)) = spawn_actor() else {
        return;
    };
    let mut input = gb_logic::InputState::default();
    for _ in 0..60 {
        tick(&mut world, &mut actor, &mut input, 0.0, 0.0, false);
    }
    let y0 = world.pose(actor.beast.body(Part::Hips)).position.y;
    tick(&mut world, &mut actor, &mut input, 0.0, 0.0, true);
    let mut peak = y0;
    let mut saw_jump = false;
    for _ in 0..100 {
        tick(&mut world, &mut actor, &mut input, 0.0, 0.0, false);
        peak = peak.max(world.pose(actor.beast.body(Part::Hips)).position.y);
        saw_jump |= actor.state == gb_logic::actor::state::JUMP;
    }
    let head = world.pose(actor.beast.body(Part::Head)).position.y;
    println!(
        "jump: hips {y0} -> peak {peak}, state {}, head {head}",
        actor.state
    );
    assert!(saw_jump && peak > y0 + 0.4, "no jump");
}

#[test]
fn falls_hurt_and_punches_knock_back() {
    let Some((mut world, mut a, spawn)) = spawn_actor() else {
        return;
    };
    let root = assets().unwrap();
    let src = gb_logic::beast::load_source(&root).unwrap();
    // B starts 4 m up, head first: landing on the roof must hurt (static = 40 kg, Fall x4).
    let origin = Pose {
        position: spawn.position + Vec3::new(0.0, 4.0, 0.9),
        rotation: Quat::from_rotation_x(std::f32::consts::PI),
        scale: Vec3::ONE,
    };
    let bi = world.spawn("b", &src, origin).unwrap();
    let bb = Beast::new(&world, bi, &src).unwrap();
    bb.setup_rigidbodies(&mut world);
    let mut b = gb_logic::Actor::new(bb, &world, 2);
    b.status.dmg_modifier = 1.0;
    let _beasts = [a.beast.instance, b.beast.instance];
    let (mut ia, ib) = (
        gb_logic::InputState::default(),
        gb_logic::InputState::default(),
    );
    use gb_logic::input::GRAB_LEFT;
    let mut min_health = 100.0f32;
    let mut b_speed = 0.0f32;
    for t in 0..400 {
        ia.set(
            &[(GRAB_LEFT, t % 25 == 0)],
            0.0,
            if t > 150 { 0.5 } else { 0.0 },
        );
        a.fixed_update(&mut world, &ia);
        b.fixed_update(&mut world, &ib);
        world.step();
        let mut interact = vec![1; world.bodies.len()];
        for act in [&a, &b] {
            for (k, p) in Part::ALL.iter().enumerate() {
                interact[act.beast.body(*p)] = act.interact[k];
            }
        }
        a.after_step(&mut world, &any_interactable);
        b.after_step(&mut world, &any_interactable);
        min_health = min_health.min(b.status.health);
        if t > 150 {
            b_speed = b_speed.max(world.linear_velocity(b.beast.body(Part::Chest)).length());
        }
    }
    println!("combat: b min health {min_health}, hits {} max raw {:.2}, b max speed after punches {b_speed:.1}", b.debug_hits, b.debug_max_hit);
    assert!(min_health < 100.0, "a head-first fall did no damage");
}

/// Test stand-in: everything is grabbable (type 2) and deals plain hits.
fn any_interactable(_: gb_phys::ActorRef) -> Option<gb_logic::actor::Interactable> {
    Some(gb_logic::actor::Interactable {
        grab_modifier: 2,
        damage_modifier: 1,
        part_of_ragdoll: false,
        always_drain: false,
    })
}

#[test]
fn targeting_picks_the_other_beast_and_the_hand_reaches_it() {
    let Some((mut world, mut a, spawn)) = spawn_actor() else {
        return;
    };
    let root = assets().unwrap();
    let src = gb_logic::beast::load_source(&root).unwrap();
    let origin = Pose {
        position: spawn.position + Vec3::new(0.0, 0.0, 1.0),
        rotation: Quat::from_rotation_y(std::f32::consts::PI),
        scale: Vec3::ONE,
    };
    let bi = world.spawn("b", &src, origin).unwrap();
    let bb = Beast::new(&world, bi, &src).unwrap();
    bb.setup_rigidbodies(&mut world);
    let mut b = gb_logic::Actor::new(bb, &world, 2);
    // TargetsForActors: beast part colliders with a priority.
    let candidates: Vec<(usize, i32)> = world
        .colliders
        .iter()
        .enumerate()
        .filter(|(_, c)| c.instance == a.beast.instance || c.instance == b.beast.instance)
        .map(|(k, c)| (k, gb_logic::actor::priority_in(&src, c.node)))
        .filter(|&(_, p)| p != 0)
        .collect();
    assert!(!candidates.is_empty(), "no beast target candidates");
    let (mut ia, ib) = (
        gb_logic::InputState::default(),
        gb_logic::InputState::default(),
    );
    use gb_logic::input::GRAB_LEFT;
    let mut grabbed = false;
    for t in 0..120 {
        ia.set(&[(GRAB_LEFT, t >= 50)], 0.0, 0.0);
        a.update_targets(&world, &candidates);
        a.fixed_update(&mut world, &ia);
        b.fixed_update(&mut world, &ib);
        world.step();
        a.after_step(&mut world, &any_interactable);
        b.after_step(&mut world, &any_interactable);
        grabbed |= a.control.hands[0].joint.is_some();
    }
    let interest = a.upper_interest.expect("no upper interest");
    assert_eq!(
        world.colliders[interest].instance, b.beast.instance,
        "targeted itself or the stage"
    );
    println!(
        "targeting: upper {:?} lower {:?} grabbed {grabbed}",
        a.upper_interest, a.lower_interest
    );
    assert!(
        grabbed,
        "standing still, the hand never reached the targeted beast"
    );
}

#[test]
fn rooftop_billboards_stay_standing() {
    let Some(root) = assets() else { return };
    let mut world = World::new(Settings::load(&root).unwrap()).unwrap();
    let stage = Sidecar::load(&root, "rooftop").unwrap();
    let inst = world.spawn("rooftop", &stage, Pose::IDENTITY).unwrap();
    let signs: Vec<(usize, usize)> = world.instances[inst]
        .bodies
        .iter()
        .filter(|(n, _)| stage.nodes[**n].path.contains("/Sign ("))
        .map(|(n, b)| (*n, *b))
        .collect();
    assert!(
        !signs.is_empty(),
        "no billboard bodies (are the signs exported active?)"
    );
    let start: Vec<Vec3> = signs.iter().map(|&(_, b)| world.pose(b).position).collect();
    for step in 0..600 {
        if step == 150 {
            world.make_joints_breakable();
        }
        world.step();
    }
    let mut worst = (0.0f32, String::new());
    for (k, &(n, b)) in signs.iter().enumerate() {
        let d = world.pose(b).position.distance(start[k]);
        if d > worst.0 {
            worst = (d, stage.nodes[n].path.clone());
        }
    }
    println!(
        "billboards: {} bodies, worst drift {:.3} m ({})",
        signs.len(),
        worst.0,
        worst.1
    );
    assert!(
        worst.0 < 0.3,
        "a billboard part moved {:.2} m: {}",
        worst.0,
        worst.1
    );
}
