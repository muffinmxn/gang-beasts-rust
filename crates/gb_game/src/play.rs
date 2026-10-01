//! Playable test: PhysX world + ported Actor logic on a 50 Hz FixedUpdate, driving the glTF bones.
//! Any number of beasts; keyboard/mouse and each gamepad pick which beast they control.
use bevy::gltf::GltfExtras;
use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::prelude::*;
use gb_logic::{input as act, Actor, Beast, InputState, Part};
use gb_phys::{
    source::{mirror_position, mirror_rotation},
    Iso, Pose, Settings, Sidecar, World,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

mod events;

/// Everything PhysX owns lives on the main thread.
pub struct Sim {
    pub world: World,
    beast_src: Sidecar,
    /// Kill Volume trigger boxes (Unity space): pose, half extents.
    kill_boxes: Vec<(Iso, Vec3)>,
    /// Breakable glass (Fracture) and its shards.
    pub fractures: crate::fracture::Fractures,
    /// Melee round flow (GameMode_Gang + GameManagerNew.EndRoundOrGame).
    pub round: crate::round::Round,
    /// (collider, priority) targets for TargetingHandeler, and the collider count it was built for.
    target_candidates: (Vec<(usize, i32)>, usize),
    pub actors: Vec<Actor>,
    inputs: Vec<InputState>,
    /// Physics scene index (into `scenes`) of each actor.
    actor_scene: Vec<usize>,
    /// Unity-space spawn pose of each actor.
    actor_spawn: Vec<Pose>,
    /// (instance, sidecar, Bevy-space root placement) per spawned scene.
    scenes: Vec<(usize, Sidecar, Mat4)>,
    /// Previous/current PhysX poses, used to render fixed-step motion smoothly.
    body_pose_history: HashMap<usize, (Iso, Iso)>,
    /// Bevy-space spawn points of the stage.
    spawn_points: Vec<Transform>,
    next_spawn: usize,
    /// Actor driven by keyboard + mouse.
    pub kbm: usize,
    /// Paused local match; physics, scoring and round timers stop together.
    pub paused: bool,
    /// Actor driven by each gamepad.
    pads: HashMap<Entity, usize>,
    /// Scenes that still need a SceneRoot entity.
    pending_roots: Vec<usize>,
    /// GlobalCameraMarker.CameraOffsets (Unity space) and the selected one.
    cam_offsets: Vec<Vec3>,
    /// Camera anchor relative to the tracked group, authored by GlobalCameraMarker.
    cam_anchor_offset: Vec3,
    /// Source group-composer framing settings for the active virtual camera.
    cam_framing: CameraFraming,
    /// CinemachineCollider.m_CollideAgainst from the active stage virtual camera.
    cam_obstacle_mask: u32,
    /// CinemachineCollider.m_CameraRadius used by the source sphere sweep.
    cam_obstacle_radius: f32,
    /// CinemachineConfiner bounding box (Bevy space centre, rotation, half extents): the camera
    /// position never leaves it (the subway's walls hide the void around the station).
    cam_confiner: Option<(Vec3, Quat, Vec3)>,
    /// Stage InteractableObject data by (instance, node).
    stage_interact:
        std::collections::HashMap<(usize, usize), Option<gb_logic::actor::Interactable>>,
    pub cam_index: usize,
    /// Screen right / screen up-the-stage as Unity-space ground directions (for camera-relative input).
    pub cam_right: Vec3,
    pub cam_forward: Vec3,
    /// Menu lobby (BeastMenuSpawner): beasts join per device, no rounds, pause or follow camera.
    pub lobby: bool,
    /// Lobby stance hold: body -> the pose captured after the beast settles, held so it stands
    /// instead of slumping. Per actor; None until the settle window elapses.
    lobby_hold: Vec<Option<Vec<(usize, Iso)>>>,
    /// Frames counted before the stance is captured.
    lobby_settle: Vec<u32>,
    /// Lobby: which beast each input device drives (None = keyboard + mouse).
    pub device_actor: HashMap<Option<Entity>, usize>,
    /// Lobby: readied up (arms raised, per actor).
    pub lobby_ready: Vec<bool>,
    /// Lobby: backed out; bodies out of the simulation and hidden until the slot is reused.
    pub parked: Vec<bool>,
    /// Render-only Aquarium tentacle bones; source controllers are inactive in the scene bundle.
    tentacle_bones: Vec<TentacleBone>,
    /// Unlocked source PlayerColors swatches, in database order.
    player_colors: Vec<Color>,
    /// Selected palette swatch; additional local players continue from it.
    player_color_start: usize,
    /// Source GLB's Primary_Color factor; identifies the actor-controlled material slot.
    primary_material_color: Color,
    /// `GB_AUTO_INPUT` actions (scripted keyboard input for captures); empty when unset.
    auto_actions: Vec<String>,
}

/// Which spawned SceneRoot corresponds to which physics scene.
#[derive(Component)]
pub struct PhysicsScene(pub usize);

#[derive(Resource, Default)]
pub struct NodeEntities(pub HashMap<(usize, usize), Entity>);

#[derive(Clone, Copy)]
struct TentacleBone {
    node: usize,
    chain: u8,
    segment: usize,
    count: usize,
}

/// `CinemachineConfiner.m_BoundingVolume` (a BoxCollider node) as a world-space box.
fn camera_confiner(src: &Sidecar) -> Option<(Vec3, Quat, Vec3)> {
    let volume = src.nodes.iter().find_map(|n| {
        let c = n.components.iter().find(|c| c.script.as_deref() == Some("CinemachineConfiner"))?;
        if c.data["m_Enabled"].as_u64() == Some(0) {
            return None;
        }
        c.data["m_BoundingVolume"]["node"].as_u64().map(|v| v as usize)
    })?;
    let node = src.nodes.get(volume)?;
    let collider = node.components.iter().find(|c| c.kind == "BoxCollider")?;
    let world = src.world_poses(Pose::IDENTITY);
    let w = world.get(volume)?;
    let center = gb_phys::source::vec3(&collider.data["m_Center"]);
    let size = gb_phys::source::vec3(&collider.data["m_Size"]);
    let centre_world = w.position + w.rotation * (w.scale * mirror_position(center));
    Some((centre_world, w.rotation, (w.scale * size * 0.5).abs()))
}

fn scene_mapping_complete(
    keys: impl Iterator<Item = (usize, usize)>,
    scene: usize,
    expected: usize,
) -> bool {
    scene_mapped_count(keys, scene) >= expected
}

fn scene_mapped_count(keys: impl Iterator<Item = (usize, usize)>, scene: usize) -> usize {
    keys.filter(|(mapped_scene, _)| *mapped_scene == scene)
        .count()
}

#[derive(Clone, Copy, Debug)]
struct CameraFraming {
    screen_y: f32,
    group_size: f32,
    min_distance: f32,
    max_distance: f32,
    max_fov: f32,
}

impl Default for CameraFraming {
    fn default() -> Self {
        Self {
            screen_y: 0.5,
            group_size: 0.8,
            min_distance: 8.0,
            max_distance: 20.0,
            max_fov: 40.0,
        }
    }
}

fn camera_framing(sidecar: &Sidecar) -> Option<CameraFraming> {
    sidecar
        .nodes
        .iter()
        .filter(|node| node.active_in_hierarchy)
        .flat_map(|node| &node.components)
        .find(|component| component.script.as_deref() == Some("GangBeastsGroupComposer"))
        .map(|component| {
            let number = |name: &str, fallback: f32| {
                component.data[name]
                    .as_f64()
                    .map_or(fallback, |value| value as f32)
            };
            CameraFraming {
                screen_y: number("m_ScreenY", 0.5),
                group_size: number("m_GroupFramingSize", 0.8),
                min_distance: number("m_MinimumDistance", 8.0),
                max_distance: number("m_MaximumDistance", 20.0),
                max_fov: number("m_MaximumFOV", 40.0),
            }
        })
}

fn camera_obstacle_radius(sidecar: &Sidecar) -> Option<f32> {
    sidecar
        .nodes
        .iter()
        .filter(|node| node.active_in_hierarchy)
        .flat_map(|node| &node.components)
        .find(|component| component.script.as_deref() == Some("CinemachineCollider"))
        .and_then(|component| component.data["m_CameraRadius"].as_f64())
        .map(|radius| radius as f32)
}

#[derive(Component)]
pub struct FollowCam;

#[derive(Component)]
struct Hud;

impl Sim {
    pub fn spawn_beast(&mut self, spawn: Transform) -> Result<usize, String> {
        let origin = Pose {
            position: mirror_position(spawn.translation),
            rotation: mirror_rotation(spawn.rotation),
            scale: Vec3::ONE,
        };
        let n = self.actors.len();
        let instance = self
            .world
            .spawn(&format!("beast{n}"), &self.beast_src, origin)?;
        let beast = Beast::new(&self.world, instance, &self.beast_src)?;
        beast.setup_rigidbodies(&mut self.world);
        self.actors
            .push(Actor::new(beast, &self.world, 0x5eed + n as u32 * 7919));
        self.inputs.push(InputState::default());
        self.scenes
            .push((instance, self.beast_src.clone(), spawn.compute_matrix()));
        self.actor_scene.push(self.scenes.len() - 1);
        self.actor_spawn.push(origin);
        self.lobby_ready.push(false);
        self.parked.push(false);
        self.pending_roots.push(self.scenes.len() - 1);
        for body in self.world.instances[instance].bodies.values().copied() {
            let pose = self.world.pose(body);
            self.body_pose_history.insert(body, (pose, pose));
        }
        Ok(n)
    }

    pub fn player_color(&self, k: usize) -> Color {
        // Gang mode: a gang shares its colour (the gang's first fighter).
        let k = self.round.mode.team_of(k);
        self.player_colors[(self.player_color_start + k) % self.player_colors.len().max(1)]
    }

    /// First local player's palette colour, if the palette loaded. Used for costume tint when
    /// there is no menu (a stage launch), where `menu.selected_color()` is unavailable.
    pub fn default_color(&self) -> Option<Color> {
        (!self.player_colors.is_empty()).then(|| self.player_color(0))
    }

    /// GameManagerNew reloads the level between rounds: put every stage body back where it
    /// started, restore shattered glass, clear shards and respawn every beast.
    fn reload_stage(&mut self) {
        for &(b, pose) in &self.round.stage_poses {
            self.world.restore_body(b);
            self.world.teleport(b, pose);
            self.world.set_linear_velocity(b, Vec3::ZERO);
            self.world.set_angular_velocity(b, Vec3::ZERO);
            self.body_pose_history.insert(b, (pose, pose));
        }
        self.fractures.reset(&mut self.world);
        for k in 0..self.actors.len() {
            self.respawn(k);
        }
    }

    /// Render scene index of actor k's beast (PhysicsScene / NodeEntities key).
    pub fn actor_scene_of(&self, actor: usize) -> usize {
        self.actor_scene[actor]
    }

    /// Name of a beast prefab node (sidecar index), e.g. "actor_chest_collider".
    pub fn beast_node_name(&self, node: usize) -> &str {
        self.beast_src.nodes[node].name()
    }

    /// The beast sidecar (for `RemoveUnseenMesh`: rest-pose bone positions).
    pub fn beast_sidecar(&self) -> &Sidecar {
        &self.beast_src
    }

    fn actor_bodies(&self, actor: usize) -> Vec<usize> {
        let instance = self.actors[actor].beast.instance;
        self.world.instances[instance].bodies.values().copied().collect()
    }

    /// Lobby back-out: take the beast out of the world and hide it.
    pub fn park_beast(&mut self, actor: usize) {
        for b in self.actor_bodies(actor) {
            self.world.remove_body(b);
        }
        self.parked[actor] = true;
        self.lobby_ready[actor] = false;
    }

    /// Lobby rejoin into a parked slot: back into the world at `spawn`, standing.
    pub fn unpark_beast(&mut self, actor: usize, spawn: Transform) {
        self.actor_spawn[actor] = Pose {
            position: mirror_position(spawn.translation),
            rotation: mirror_rotation(spawn.rotation),
            scale: Vec3::ONE,
        };
        for b in self.actor_bodies(actor) {
            self.world.restore_body(b);
        }
        self.respawn(actor);
        for b in self.actor_bodies(actor) {
            self.world.set_linear_velocity(b, Vec3::ZERO);
            self.world.set_angular_velocity(b, Vec3::ZERO);
        }
        self.parked[actor] = false;
        self.lobby_ready[actor] = false;
    }

    fn respawn(&mut self, actor: usize) {
        let Some(&scene) = self.actor_scene.get(actor) else {
            return;
        };
        let (instance, src, _) = &self.scenes[scene];
        let poses = src.world_poses(self.actor_spawn[actor]);
        let bodies: Vec<(usize, usize)> = self.world.instances[*instance]
            .bodies
            .iter()
            .map(|(n, b)| (*n, *b))
            .collect();
        for (node, body) in bodies {
            let pose = Iso::of(&poses[node]);
            self.world.teleport(body, pose);
            self.body_pose_history.insert(body, (pose, pose));
        }
        self.actors[actor].revive();
    }
}

pub fn build(
    root: &Path,
    stage: &str,
    spawn_points: Vec<Transform>,
    first: usize,
    players: usize,
    wins_to_win: u32,
    player_color_start: usize,
) -> Result<Sim, String> {
    let mut world = World::new(Settings::load(root)?)?;
    let stage_src = Sidecar::load(root, stage)?;
    let tentacle_bones = aquarium_tentacle_bones(&stage_src);
    let beast_src = gb_logic::beast::load_source(root)?;
    let player_colors = load_player_colors(root)?;
    let player_color_start = player_color_start % player_colors.len().max(1);
    let primary_material_color = glb_material_color(root, "beast", "Primary_Color")?;
    let s = world.spawn(stage, &stage_src, Pose::IDENTITY)?;
    let mut sim = Sim {
        world,
        beast_src,
        actors: vec![],
        inputs: vec![],
        actor_scene: vec![],
        actor_spawn: vec![],
        scenes: vec![(s, stage_src, Mat4::IDENTITY)],
        body_pose_history: HashMap::new(),
        next_spawn: first + players,
        spawn_points,
        kbm: 0,
        paused: false,
        pads: HashMap::new(),
        pending_roots: vec![],
        cam_offsets: vec![],
        cam_anchor_offset: Vec3::ZERO,
        cam_framing: CameraFraming::default(),
        cam_obstacle_mask: 524_304,
        cam_obstacle_radius: 4.0,
        cam_confiner: None,
        stage_interact: Default::default(),
        target_candidates: (vec![], usize::MAX),
        kill_boxes: vec![],
        fractures: Default::default(),
        round: crate::round::Round::with_wins(
            {
                let mut names = crate::round::colour_names(root);
                if !names.is_empty() {
                    let palette_len = names.len();
                    names.rotate_left(player_color_start % palette_len);
                }
                names
            },
            wins_to_win,
        ),
        cam_index: 0,
        cam_right: Vec3::X,
        lobby: false,
        lobby_hold: vec![],
        lobby_settle: vec![],
        device_actor: HashMap::new(),
        lobby_ready: vec![],
        parked: vec![],
        cam_forward: Vec3::Z,
        tentacle_bones,
        player_colors,
        player_color_start,
        primary_material_color,
        auto_actions: std::env::var("GB_AUTO_INPUT")
            .map(|script| {
                script
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    };
    // Read the active source composer screen position and camera marker settings.
    for n in &sim.scenes[0].1.nodes {
        for c in &n.components {
            if c.script.as_deref() == Some("GlobalCameraMarker") && n.active_in_hierarchy {
                // CameraMarker is serialized at (0, 8, 14) for Aquarium; use the source anchor
                // instead of deriving camera height and distance from the viewer start pose.
                sim.cam_anchor_offset = Vec3::from_array(n.transform.translation);
                if let Some(list) = c.data["CameraOffsets"].as_array() {
                    for (k, o) in list.iter().enumerate() {
                        sim.cam_offsets
                            .push(gb_phys::source::vec3(&o["TargetOffset"]));
                        if o["IsDefaultOffset"].as_i64() == Some(1) {
                            sim.cam_index = k;
                        }
                    }
                }
            }
        }
    }
    sim.cam_framing = camera_framing(&sim.scenes[0].1).unwrap_or_default();
    if std::env::var_os("GB_CAM_DEBUG").is_some() {
        let source_composers: Vec<_> = sim.scenes[0]
            .1
            .nodes
            .iter()
            .flat_map(|node| {
                node.components.iter().filter_map(move |component| {
                    (component.script.as_deref() == Some("GangBeastsGroupComposer")).then(|| {
                        (
                            node.path.as_str(),
                            node.active_in_hierarchy,
                            component.data["m_ScreenY"].clone(),
                        )
                    })
                })
            })
            .collect();
        eprintln!(
            "loaded GangBeastsGroupComposers: {source_composers:?}; selected framing={:?}",
            sim.cam_framing
        );
    }
    // Read the active CinemachineCollider mask; it excludes broad Default-layer aquarium scenery.
    for node in &sim.scenes[0].1.nodes {
        if !node.active_in_hierarchy {
            continue;
        }
        for component in &node.components {
            if component.script.as_deref() == Some("CinemachineCollider") {
                sim.cam_obstacle_mask = component.data["m_CollideAgainst"]["m_Bits"]
                    .as_u64()
                    .unwrap_or(524_304) as u32;
            }
        }
    }
    sim.cam_obstacle_radius = camera_obstacle_radius(&sim.scenes[0].1).unwrap_or(4.0);
    sim.cam_confiner = camera_confiner(&sim.scenes[0].1);
    {
        let src = &sim.scenes[0].1;
        let kill_layer = sim
            .world
            .settings
            .layers
            .iter()
            .position(|l| l == "Kill Volume");
        let poses = src.world_poses(gb_phys::Pose::IDENTITY);
        for (i, n) in src.nodes.iter().enumerate() {
            if !n.active_in_hierarchy || Some(n.layer as usize) != kill_layer {
                continue;
            }
            for c in n.components.iter().filter(|c| c.kind == "BoxCollider") {
                let size = gb_phys::source::vec3(&c.data["m_Size"]);
                let center = gb_phys::source::vec3(&c.data["m_Center"]);
                let p = poses[i];
                let pose = Iso::new(p.position + p.rotation * (center * p.scale), p.rotation);
                sim.kill_boxes.push((pose, (size * p.scale).abs() * 0.5));
            }
        }
        info!("{} kill volumes", sim.kill_boxes.len());
    }
    sim.fractures =
        crate::fracture::Fractures::from_scene(&sim.world, sim.scenes[0].0, &sim.scenes[0].1);
    // Stage state at load, restored when the stage "reloads" between rounds.
    let stage_instance = sim.scenes[0].0;
    sim.round.stage_poses = sim.world.instances[stage_instance]
        .bodies
        .values()
        .map(|&b| (b, sim.world.pose(b)))
        .collect();
    info!("{} breakable glass panes", sim.fractures.glass.len());
    for (i, _) in sim.scenes[0].1.nodes.iter().enumerate() {
        let io = gb_logic::actor::interactable_in(&sim.scenes[0].1, i);
        sim.stage_interact.insert((s, i), io);
    }
    if sim.cam_offsets.is_empty() {
        sim.cam_offsets.push(Vec3::new(0.0, 5.0, 8.0));
    }
    if stage == "aquarium" && sim.cam_offsets.len() >= 3 {
        // The Google reference uses an oblique composition. Start on the authored negative-X
        // diagonal to approximate it; Q/E still selects all three source offsets during play.
        sim.cam_index = sim.cam_offsets.len() - 1;
    }
    if let Some(index) = std::env::var("GB_CAM_INDEX")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|index| *index < sim.cam_offsets.len())
    {
        sim.cam_index = index;
    }
    if players > 0 {
        let at = sim.spawn_points[first % sim.spawn_points.len()];
        sim.spawn_beast(at)?;
    }
    for player in 1..players {
        let spawn = sim.spawn_points[(first + player) % sim.spawn_points.len()];
        sim.spawn_beast(spawn)?;
    }
    for w in sim.world.instances.iter().flat_map(|i| &i.warnings) {
        warn!("physics: {w}");
    }
    Ok(sim)
}

fn load_player_colors(root: &Path) -> Result<Vec<Color>, String> {
    let path = root.join("player-colors.json");
    let data: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    let colors = data["colors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let rgba = entry["rgba"].as_array()?;
            (rgba.len() == 4).then(|| {
                Color::linear_rgba(
                    rgba[0].as_f64().unwrap_or(0.0) as f32,
                    rgba[1].as_f64().unwrap_or(0.0) as f32,
                    rgba[2].as_f64().unwrap_or(0.0) as f32,
                    rgba[3].as_f64().unwrap_or(1.0) as f32,
                )
            })
        })
        .collect::<Vec<_>>();
    if colors.is_empty() {
        return Err(format!(
            "{}: no valid PlayerColors swatches",
            path.display()
        ));
    }
    Ok(colors)
}

fn glb_material_color(root: &Path, asset: &str, name: &str) -> Result<Color, String> {
    let path = root.join(format!("{asset}.glb"));
    let glb = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if glb.len() < 20 || &glb[..4] != b"glTF" {
        return Err(format!("{}: invalid GLB header", path.display()));
    }
    let size = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    if size > glb.len() - 20 || u32::from_le_bytes(glb[16..20].try_into().unwrap()) != 0x4e4f534a {
        return Err(format!("{}: invalid GLB JSON chunk", path.display()));
    }
    let doc: serde_json::Value = serde_json::from_slice(&glb[20..20 + size])
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let factor = doc["materials"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|material| material["name"].as_str() == Some(name))
        .and_then(|material| material["pbrMetallicRoughness"]["baseColorFactor"].as_array())
        .filter(|rgba| rgba.len() == 4)
        .ok_or_else(|| format!("{}: material {name} has no base color", path.display()))?;
    Ok(Color::linear_rgba(
        factor[0].as_f64().unwrap_or(1.0) as f32,
        factor[1].as_f64().unwrap_or(1.0) as f32,
        factor[2].as_f64().unwrap_or(1.0) as f32,
        factor[3].as_f64().unwrap_or(1.0) as f32,
    ))
}

pub fn plugin(app: &mut App) {
    app.insert_resource(Time::<Fixed>::from_seconds(0.02))
        .init_resource::<NodeEntities>()
        .add_systems(Startup, (spawn_hud, spawn_pause_ui, crate::round::spawn_ui))
        .add_systems(FixedUpdate, simulate)
        .add_systems(
            Update,
            (
                toggle_pause,
                ragdoll_debug,
                controls,
                spawn_roots,
                map_nodes,
                events::stage_events,
                events::drift_report,
                apply_player_colors,
                sync_bodies,
                crate::fracture::render,
                animate_tentacles,
                follow_camera,
                nametags,
                hide_parked,
                update_hud,
                crate::round::update_ui,
            )
                .chain(),
        );
}

fn axis(keys: &ButtonInput<KeyCode>, neg: &[KeyCode], pos: &[KeyCode]) -> f32 {
    let n = neg.iter().any(|k| keys.pressed(*k)) as i32 as f32;
    let p = pos.iter().any(|k| keys.pressed(*k)) as i32 as f32;
    p - n
}

#[derive(Default, Clone, Copy)]
struct Raw {
    h: f32,
    v: f32,
    buttons: [bool; 6],
}

fn kbm_raw(keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>) -> Raw {
    let held = |k: &[KeyCode]| k.iter().any(|k| keys.pressed(*k));
    Raw {
        h: axis(
            keys,
            &[KeyCode::KeyA, KeyCode::ArrowLeft],
            &[KeyCode::KeyD, KeyCode::ArrowRight],
        ),
        v: axis(
            keys,
            &[KeyCode::KeyS, KeyCode::ArrowDown],
            &[KeyCode::KeyW, KeyCode::ArrowUp],
        ),
        buttons: [
            held(&[KeyCode::Space]),
            held(&[KeyCode::ControlLeft, KeyCode::KeyC]),
            held(&[KeyCode::KeyF]),
            held(&[KeyCode::ShiftLeft]),
            mouse.pressed(MouseButton::Left),
            mouse.pressed(MouseButton::Right),
        ],
    }
}

fn pad_raw(g: &Gamepad) -> Raw {
    let (mut h, mut v) = (
        g.get(GamepadAxis::LeftStickX).unwrap_or(0.0),
        g.get(GamepadAxis::LeftStickY).unwrap_or(0.0),
    );
    if h.abs() < 0.15 && v.abs() < 0.15 {
        (h, v) = (0.0, 0.0);
    }
    let p = |b: GamepadButton| g.pressed(b);
    Raw {
        h,
        v,
        buttons: [
            p(GamepadButton::South),
            p(GamepadButton::East),
            p(GamepadButton::West),
            p(GamepadButton::North),
            p(GamepadButton::LeftTrigger),
            p(GamepadButton::RightTrigger),
        ],
    }
}

/// `GB_RAGDOLL_DEBUG=1`: report collider pairs whose world AABBs overlap (penetration) with the
/// overlap depth, the joint-connected flag and the layer pair. A penetrating pair that is NOT
/// joint-connected, or that overlaps deeply despite the collision matrix saying they collide, is
/// what "snagging" looks like from the physics side. Runs every 30 sim frames.
fn ragdoll_debug(sim: NonSend<Sim>, mut frames: Local<u32>) {
    if std::env::var_os("GB_RAGDOLL_DEBUG").is_none() {
        return;
    }
    *frames += 1;
    if *frames % 30 != 0 {
        return;
    }
    for (actor, &scene) in sim.actor_scene.iter().enumerate() {
        let instance = sim.scenes[scene].0;
        let src = &sim.beast_src;
        // Motion trace: a snag shows up as the ragdoll's speed collapsing while input says walk.
        if let Some(&body) = sim.world.instances[instance]
            .bodies
            .values()
            .next()
        {
            let pose = sim.world.pose(body);
            let v = sim.world.linear_velocity(body);
            let a = &sim.actors[actor];
            // Fastest body part: a punch that reads "too strong" shows up as a hand moving at a
            // speed no shipping punch reaches (the source applies its forces as velocity changes,
            // so the joint limits are what should cap this).
            let mut fastest = (0.0f32, String::new());
            for (&node, &body) in &sim.world.instances[instance].bodies {
                let v = sim.world.linear_velocity(body).length();
                if v > fastest.0 {
                    fastest = (v, src.nodes[node].name().to_string());
                }
            }
            println!(
                "ragdoll actor {actor}: root {:?} speed {:.2} up {:.2} state {} dir {:?} run {:.2} af {:.2} fastest {} {:.1} m/s",
                pose.position,
                v.length(),
                pose.rotation.mul_vec3(Vec3::Y).y,
                a.state,
                a.control.direction,
                a.movement.run_force,
                a.applyed_force,
                fastest.1,
                fastest.0
            );
        }
        let colliders: Vec<usize> = sim
            .world
            .colliders
            .iter()
            .enumerate()
            .filter(|(_, c)| c.instance == instance)
            .map(|(i, _)| i)
            .collect();
        let mut worst: Vec<(f32, String)> = Vec::new();
        for (n, &ca) in colliders.iter().enumerate() {
            for &cb in colliders.iter().skip(n + 1) {
                let (pa, ea) = sim.world.collider_bounds(ca);
                let (pb, eb) = sim.world.collider_bounds(cb);
                let overlap = (ea + eb) - (pa - pb).abs();
                let depth = overlap.min_element();
                if depth <= 0.01 {
                    continue;
                }
                let (ba, bb) = (
                    sim.world.colliders[ca].body,
                    sim.world.colliders[cb].body,
                );
                let connected = match (ba, bb) {
                    (Some(a), Some(b)) => sim.world.joint_connected(a, b),
                    _ => false,
                };
                let name = |c: usize| {
                    src.nodes
                        .get(sim.world.colliders[c].node)
                        .map(|n| n.name().to_string())
                        .unwrap_or_else(|| "?".into())
                };
                let (na, nb) = (name(ca), name(cb));
                // Only report pairs that can actually collide: the layer matrix allows them,
                // they are not joint-connected, and BodyBase's IgnoreCollision does not cover
                // them. A deep overlap between such a pair is a real snag.
                if connected || gb_phys::world_ignore_pair(&na, &nb) {
                    continue;
                }
                worst.push((
                    depth,
                    format!(
                        "{} <-> {} depth {:.3} layers {}/{}",
                        na,
                        nb,
                        depth,
                        src.nodes.get(sim.world.colliders[ca].node).map_or(0, |n| n.layer),
                        src.nodes.get(sim.world.colliders[cb].node).map_or(0, |n| n.layer),
                    ),
                ));
            }
        }
        worst.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, line) in worst.iter().take(8) {
            println!("ragdoll actor {actor}: {line}");
        }
    }
}

/// `GB_AUTO_INPUT=walk,punch,kick,jump,duck,lift,left,right,back`: scripted input for the
/// keyboard actor, so a capture can reproduce walking/punching snags without a human at the
/// keyboard. Applied inside `simulate` (the real input driver overwrites `inputs` every fixed
/// step, so a separate system cannot inject). Punch/kick press for half of a 1 s cycle.
fn auto_raw(sim: &Sim) -> Option<Raw> {
    if sim.auto_actions.is_empty() || sim.lobby {
        return None;
    }
    let cycle = (sim.world.steps / 25) % 2 == 0;
    let mut raw = Raw::default();
    for a in &sim.auto_actions {
        match a.as_str() {
            "walk" => raw.h += 1.0,
            "back" => raw.h -= 1.0,
            "left" => raw.v -= 1.0,
            "right" => raw.v += 1.0,
            "jump" => raw.buttons[0] = true,
            "duck" => raw.buttons[1] = true,
            "kick" => raw.buttons[2] = cycle,
            "lift" => raw.buttons[3] = true,
            "punch" => {
                raw.buttons[4] = cycle;
                raw.buttons[5] = cycle;
            }
            _ => {}
        }
    }
    Some(raw)
}

/// Gamepad: left stick, A jump, B duck (tap: headbutt), X kick, Y lift, LB/RB arms.
/// Keyboard + mouse: WASD/arrows, Space jump, Ctrl duck/headbutt, F kick, Shift lift, LMB/RMB arms.
fn simulate(
    mut sim: NonSendMut<Sim>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    pads: Query<(Entity, &Gamepad)>,
) {
    let sim = &mut *sim;
    if sim.paused {
        return;
    }
    let n = sim.actors.len();
    let mut raw = vec![Raw::default(); n];
    let mut add = |i: usize, r: Raw| {
        if n == 0 {
            return;
        }
        let t = &mut raw[i % n];
        t.h = (t.h + r.h).clamp(-1.0, 1.0);
        t.v = (t.v + r.v).clamp(-1.0, 1.0);
        for k in 0..6 {
            t.buttons[k] |= r.buttons[k];
        }
    };
    if sim.lobby {
        // Lobby beasts aren't controlled; a readied beast holds Lift with free hands, which
        // raises both arms over its head (LiftCheck -> ArmActionCheering).
        for k in 0..n {
            if sim.lobby_ready.get(k).copied().unwrap_or(false) {
                add(k, Raw { h: 0.0, v: 0.0, buttons: [false, false, false, true, false, false] });
            }
        }
    } else if n > 0 {
        add(sim.kbm, kbm_raw(&keys, &mouse));
        for (e, g) in &pads {
            let next = sim.pads.len();
            let target = *sim.pads.entry(e).or_insert(next % n);
            add(target, pad_raw(g));
        }
        // Scripted input for captures (see auto_raw): the keyboard actor walks/punches on a
        // fixed cycle, so a snag can be reproduced without a human at the keyboard.
        if let Some(raw) = auto_raw(sim) {
            add(sim.kbm, raw);
        }
    }
    let names = [
        act::JUMP,
        act::DUCK,
        act::KICK,
        act::LIFT,
        act::GRAB_LEFT,
        act::GRAB_RIGHT,
    ];
    for (k, r) in raw.iter().enumerate() {
        let buttons: Vec<(&'static str, bool)> = names.iter().copied().zip(r.buttons).collect();
        // The game's input driver makes the stick camera-relative before it reaches InputState.
        let world = sim.cam_right * r.h + sim.cam_forward * r.v;
        sim.inputs[k].set(&buttons, world.x, world.z);
    }
    // InteractableObjectManager.TargetsForActors: every collider of an interactable whose
    // priority isn't Ignore. Rebuilt when colliders are added (spawns).
    if sim.target_candidates.1 != sim.world.colliders.len() {
        let beasts: Vec<usize> = sim.actors.iter().map(|a| a.beast.instance).collect();
        let mut list = vec![];
        for (k, c) in sim.world.colliders.iter().enumerate() {
            let src = if beasts.contains(&c.instance) {
                Some(&sim.beast_src)
            } else {
                sim.scenes.iter().find(|s| s.0 == c.instance).map(|s| &s.1)
            };
            let Some(src) = src else { continue };
            let priority = gb_logic::actor::priority_in(src, c.node);
            if priority != 0 {
                list.push((k, priority));
            }
        }
        sim.target_candidates = (list, sim.world.colliders.len());
    }
    for actor in sim.actors.iter_mut() {
        actor.update_targets(&sim.world, &sim.target_candidates.0);
    }
    for (actor, input) in sim.actors.iter_mut().zip(&sim.inputs) {
        actor.fixed_update(&mut sim.world, input);
    }
    sim.world.step();
    // Lobby (BeastMenuSpawner): beasts stand on their marker and should hold that pose instead of
    // slumping into a heap while the player picks a costume. Every body gets a velocity spring
    // toward its spawn pose (position + rotation), which keeps the whole ragdoll upright without
    // teleporting, so the readied arms-over-head pose can still drive the arms.
    // GB_NO_LOBBY_HOLD=1 disables (A/B).
    if sim.lobby && std::env::var_os("GB_NO_LOBBY_HOLD").is_none() {
        for k in 0..sim.actors.len() {
            // A readied beast raises its arms (Lift). Release the hold then, or the arms cannot
            // move at all - the player must be able to ready up.
            if sim.parked.get(k).copied().unwrap_or(false)
                || sim.lobby_ready.get(k).copied().unwrap_or(false)
            {
                // Forget the frozen stance: after un-readying (or a rejoin) the beast settles
                // again, so the raised arms fall all the way down before the hold re-captures.
                if sim.lobby_hold.len() > k {
                    sim.lobby_hold[k] = None;
                    sim.lobby_settle[k] = 0;
                }
                continue;
            }
            // Let the ragdoll settle (arms fall naturally), then freeze that stance. The whole
            // body is held: holding only the torso lets the free arms pull it into a crouch.
            const SETTLE_STEPS: u32 = 75;
            if sim.lobby_hold.len() <= k {
                sim.lobby_hold.resize(k + 1, None);
                sim.lobby_settle.resize(k + 1, 0);
            }
            if sim.lobby_hold[k].is_none() {
                sim.lobby_settle[k] += 1;
                if sim.lobby_settle[k] < SETTLE_STEPS {
                    continue;
                }
                let scene = sim.actor_scene[k];
                let instance = sim.scenes[scene].0;
                sim.lobby_hold[k] = Some(
                    sim.world.instances[instance]
                        .bodies
                        .values()
                        .copied()
                        .map(|body| (body, sim.world.pose(body)))
                        .collect(),
                );
            }
            let hold = sim.lobby_hold[k].clone().unwrap_or_default();
            for (body, target) in hold {
                let pose = sim.world.pose(body);
                let delta = target.position - pose.position;
                sim.world
                    .set_linear_velocity(body, (delta * 14.0).clamp_length_max(4.0));
                let spin = (target.rotation * pose.rotation.inverse()).to_scaled_axis() * 14.0;
                sim.world
                    .set_angular_velocity(body, spin.clamp_length_max(8.0));
            }
        }
    }
    for actor in &sim.actors {
        for part in gb_logic::Part::ALL {
            let body = actor.beast.body(part);
            let pose = sim.world.pose(body);
            let history = sim.body_pose_history.entry(body).or_insert((pose, pose));
            history.0 = history.1;
            history.1 = pose;
        }
    }
    // Everything a beast can touch: beast parts (live hit types) and stage InteractableObjects.
    let mut beast_parts: std::collections::HashMap<usize, gb_logic::actor::Interactable> =
        Default::default();
    for a in &sim.actors {
        for (k, p) in gb_logic::Part::ALL.iter().enumerate() {
            beast_parts.insert(
                a.beast.body(*p),
                gb_logic::actor::Interactable {
                    grab_modifier: 2,
                    damage_modifier: a.interact[k],
                    part_of_ragdoll: true,
                    always_drain: false,
                },
            );
        }
    }
    let stage = &sim.stage_interact;
    let lookup = |r: gb_phys::ActorRef| -> Option<gb_logic::actor::Interactable> {
        if let Some(b) = r.body {
            if let Some(i) = beast_parts.get(&b) {
                return Some(*i);
            }
        }
        stage.get(&(r.instance, r.node)).copied().flatten()
    };
    let n = sim.actors.len();
    for k in 0..n {
        let world = &mut sim.world;
        sim.actors[k].after_step(world, &lookup);
    }
    // Round start (countdown stand-in until the round flow is ported): scene joints become
    // breakable. By then the signs have settled and PhysX has put them to sleep.
    if sim.world.steps == 150 {
        sim.world.make_joints_breakable();
    }
    // Fracture: glass impacts, shattering and shard lifetime.
    let dt = sim.world.settings.fixed_timestep;
    {
        let Sim {
            fractures, world, ..
        } = &mut *sim;
        fractures.step(world, dt);
    }
    // Debug: GB_DEBUG_KILL=1 kills player 2 two seconds in (exercises the round flow).
    if sim.world.steps == 100 && std::env::var_os("GB_DEBUG_KILL").is_some() && sim.actors.len() > 1
    {
        let Sim { actors, world, .. } = &mut *sim;
        actors[1].kill(world);
    }
    // Round flow.
    {
        let alive: Vec<bool> = sim
            .actors
            .iter()
            .map(|a| crate::round::alive(a.state))
            .collect();
        let colors: Vec<Color> = (0..sim.actors.len()).map(|k| sim.player_color(k)).collect();
        let dt = sim.world.settings.fixed_timestep;
        if !sim.lobby {
            sim.round.step(dt, &alive, &colors);
        }
        if sim.round.reset_requested {
            sim.round.reset_requested = false;
            sim.reload_stage();
        }
    }
    // Kill Volume triggers.
    for k in 0..n {
        let hips = sim
            .world
            .pose(sim.actors[k].beast.body(Part::Hips))
            .position;
        let inside = sim.kill_boxes.iter().any(|(pose, half)| {
            let local = pose.rotation.inverse() * (hips - pose.position);
            local.abs().cmple(*half).all()
        });
        if inside {
            let world = &mut sim.world;
            sim.actors[k].kill(world);
        }
    }
}

/// N / Select: spawn an NPC. [ / ]: switch the keyboard's beast. D-pad left/right: switch a pad's
/// beast. R: respawn the keyboard's beast; Start: respawn the pad's beast.
fn controls(
    mut sim: NonSendMut<Sim>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<(Entity, &Gamepad)>,
) {
    let sim = &mut *sim;
    if keys.just_pressed(KeyCode::KeyN)
        || pads
            .iter()
            .any(|(_, g)| g.just_pressed(GamepadButton::Select))
    {
        if !sim.spawn_points.is_empty() {
            let at = sim.spawn_points[sim.next_spawn % sim.spawn_points.len()];
            sim.next_spawn += 1;
            if let Err(e) = sim.spawn_beast(at) {
                error!("spawn failed: {e}");
            }
        }
    }
    let n = sim.actors.len();
    if keys.just_pressed(KeyCode::BracketRight) {
        sim.kbm = (sim.kbm + 1) % n;
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        sim.kbm = (sim.kbm + n - 1) % n;
    }
    if keys.just_pressed(KeyCode::KeyR) {
        let a = sim.kbm;
        sim.respawn(a);
    }
    let last = sim.cam_offsets.len() - 1;
    let left = keys.just_pressed(KeyCode::KeyQ)
        || pads
            .iter()
            .any(|(_, g)| g.just_pressed(GamepadButton::DPadLeft));
    let right = keys.just_pressed(KeyCode::KeyE)
        || pads
            .iter()
            .any(|(_, g)| g.just_pressed(GamepadButton::DPadRight));
    if left && sim.cam_index > 0 {
        sim.cam_index -= 1;
    }
    if right && sim.cam_index < last {
        sim.cam_index += 1;
    }
    if keys.just_pressed(KeyCode::F9) {
        let on = !sim.world.drive_limits_are_forces;
        sim.world.set_drive_limits_are_forces(on);
    }
    for (e, g) in &pads {
        let next = sim.pads.len();
        let t = sim.pads.entry(e).or_insert(next % n);
        if g.just_pressed(GamepadButton::RightThumb) {
            *t = (*t + 1) % n;
        }
        if g.just_pressed(GamepadButton::LeftThumb) {
            *t = (*t + n - 1) % n;
        }
        let t = *t;
        if g.just_pressed(GamepadButton::Start) {
            sim.respawn(t);
        }
    }
}

fn spawn_roots(mut sim: NonSendMut<Sim>, mut commands: Commands, assets: Res<AssetServer>) {
    for scene in sim.pending_roots.drain(..) {
        // The physics places every beast node itself, relative to an identity root.
        commands.spawn((
            SceneRoot(assets.load(GltfAssetLabel::Scene(0).from_asset("beast.glb"))),
            Transform::IDENTITY,
            PhysicsScene(scene),
        ));
    }
}

/// Wait for glTF scenes to spawn, then index their node entities by (scene, sidecar node).
fn map_nodes(
    sim: NonSend<Sim>,
    mut map: ResMut<NodeEntities>,
    roots: Query<(Entity, &PhysicsScene)>,
    children: Query<&Children>,
    extras: Query<&GltfExtras>,
    meshes: Query<(), With<Mesh3d>>,
    material_names: Query<&bevy::gltf::GltfMaterialName>,
    mut clear: ResMut<ClearColor>,
    mut mesh_materials: Query<&mut MeshMaterial3d<StandardMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for (root, scene) in &roots {
        let Some((_, source, _)) = sim.scenes.get(scene.0) else {
            continue;
        };
        let mapped_before = scene_mapped_count(map.0.keys().copied(), scene.0);
        if mapped_before >= source.nodes.len() {
            continue;
        }
        for e in children.iter_descendants(root) {
            if meshes.contains(e) {
                // Bones leave the bind-pose bounds as the ragdoll moves.
                commands
                    .entity(e)
                    .insert(bevy::render::view::NoFrustumCulling);
                // The exporter's fallback for a material it couldn't resolve (e.g. the rooftop's
                // Water plane) is plain white; Unity never shows it that way, so hide it.
                // Debug: GB_HIDE_MAT=<material name> hides meshes using it.
                let hide_debug = std::env::var("GB_HIDE_MAT").ok();
                if material_names.get(e).is_ok_and(|n| {
                    // `default` is the exporter's unresolved-material fallback; the `SolidVoid`
                    // effect (stage backdrops named "Void"/"Void <Stage>") and `FogCard` volumes
                    // are Unity effect geometry that a plain mesh renderer would draw as an opaque
                    // box enclosing the camera.
                    let lower = n.0.to_ascii_lowercase();
                    let hide = n.0 == "default"
                        || lower.contains("void")
                        || lower.contains("fog card")
                        || lower.contains("fogcard")
                        || Some(&n.0) == hide_debug.as_ref();
                    if hide && std::env::var_os("GB_HIDE_DEBUG").is_some() {
                        info!("hiding mesh with material '{}'", n.0);
                    }
                    hide
                }) {
                    // SolidVoid is the stage's dark backdrop: where it is hidden the camera sees the
                    // clear colour, so make that the void's black instead of the daytime sky.
                    let is_void = material_names.get(e).is_ok_and(|n| n.0.to_ascii_lowercase().contains("void"));
                    if is_void && std::env::var_os("GB_HIDE_VOID").is_none() {
                        // Draw the backdrop as the flat black GangBeasts/Effects/SolidVoid fill
                        // (single sided, so a camera inside the box sees through it).
                        clear.0 = Color::BLACK;
                        if let Ok(mut handle) = mesh_materials.get_mut(e) {
                            handle.0 = materials.add(StandardMaterial {
                                base_color: Color::BLACK,
                                unlit: true,
                                cull_mode: Some(bevy::render::render_resource::Face::Back),
                                ..default()
                            });
                        }
                    } else {
                        if is_void {
                            clear.0 = Color::BLACK;
                        }
                        commands.entity(e).insert(Visibility::Hidden);
                    }
                }
            }
            // "URP FX/Water4" surfaces ("Water - Containers", ...): the exporter wrote the authored colour in
            // Unity's gamma space as if linear, which turned the deep sea turquoise. Decode it and draw the
            // sea as a glossy translucent sheet. (Waves / foam / depth fade are not ported.)
            if meshes.contains(e) && material_names.get(e).is_ok_and(|n| n.0.starts_with("Water")) {
                if let Ok(handle) = mesh_materials.get(e) {
                    if let Some(src) = materials.get(&handle.0) {
                        if !matches!(src.alpha_mode, AlphaMode::Blend) {
                            let mut sea = src.clone();
                            let c = src.base_color.to_linear();
                            let dec = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
                            sea.base_color = Color::linear_rgba(dec(c.red), dec(c.green), dec(c.blue), c.alpha.clamp(0.6, 0.95));
                            sea.alpha_mode = AlphaMode::Blend;
                            sea.perceptual_roughness = 0.12;
                            sea.metallic = 0.0;
                            sea.reflectance = 0.5;
                            sea.emissive = LinearRgba::BLACK;
                            let new = materials.add(sea);
                            if let Ok(mut h) = mesh_materials.get_mut(e) {
                                h.0 = new;
                            }
                        }
                    }
                }
            }
            let Ok(x) = extras.get(e) else { continue };
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&x.value) {
                if let Some(n) = v["gb_node"].as_u64() {
                    map.0.insert((scene.0, n as usize), e);
                }
            }
        }
        let mapped_after = scene_mapped_count(map.0.keys().copied(), scene.0);
        if mapped_after >= source.nodes.len() {
            info!(
                "ragdoll scene {} mapped all {} nodes",
                scene.0, mapped_after
            );
        }
    }
}

#[derive(Component)]
/// Marks a mesh whose material we replaced with the palette tint.
/// Source behaviour (1:1): `BodyHandeler.SetSkinColor` (0x436d50) calls
/// `Material.SetColor(Actor._primaryColor)` on the head and body materials — the player colour
/// REPLACES the material colour. Multiplying the authored albedo (which is a dark red,
/// 0.68/0.22/0.22) is what made every palette swatch render as a shade of red.
struct TintedPrimary {
    /// The palette colour applied, in linear space (for re-tint comparisons).
    base: Vec3,
}

/// Marks a beast mesh whose near-white material got the authored-albedo correction, storing the
/// corrected base so repeated colour switches do not gamma-decode and re-correct it again (each
/// switch used to darken the head/eyes).
#[derive(Component)]
struct BeastCorrected {
    base: Vec3,
}

fn apply_player_colors(
    sim: NonSend<Sim>,
    menu: Option<Res<crate::menu::Menu>>,
    map: Res<NodeEntities>,
    children: Query<&Children>,
    markers: Query<&TintedPrimary>,
    corrected: Query<&BeastCorrected>,
    mut material_handles: Query<&mut MeshMaterial3d<StandardMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut applied: Local<HashMap<usize, usize>>,
) {
    if sim.player_colors.is_empty() {
        return;
    }
    // In the menu the lobby drives the chosen swatch live; stages use the CLI start index.
    let base = menu.as_ref().map_or(sim.player_color_start, |m| m.player_color);
    for (player, &scene) in sim.actor_scene.iter().enumerate() {
        // Each local player continues from the chosen swatch (the source hands successive players
        // the next unlocked colour); using the same index for everyone made every beast the same
        // colour.
        let chosen = base + player;
        if applied.get(&scene) == Some(&chosen) {
            continue;
        }
        let Some((_, source, _)) = sim.scenes.get(scene) else {
            continue;
        };
        if !scene_mapping_complete(map.0.keys().copied(), scene, source.nodes.len()) {
            continue;
        }
        let color = sim.player_colors[chosen % sim.player_colors.len()];
        let mut tinted = 0;
        for (node_index, node) in source.nodes.iter().enumerate() {
            if !(node.path.ends_with("/actor_body_skinnedMesh")
                || node.path.ends_with("/actor_head_skinnedMesh"))
            {
                continue;
            }
            let Some(&entity) = map.0.get(&(scene, node_index)) else {
                continue;
            };
            tinted += tint_primary_materials(
                entity,
                color,
                sim.primary_material_color,
                &children,
                &markers,
                &mut material_handles,
                &mut materials,
                &mut commands,
            );
        }
        // The beast's own near-white body material (beast.glb's "White") is what reads blown in
        // the lobby. It sits on nodes outside the body/head mesh paths, so walk EVERY mapped node
        // of this actor's scene and correct any near-white albedo.
        for ((scene_index, _), entity) in map.0.iter() {
            if *scene_index != scene {
                continue;
            }
            // Skip the primary-colour meshes: tint_primary_materials already handled them, and
            // running the gamma conversion again would double-darken the body.
            if markers.get(*entity).is_ok() {
                continue;
            }
            let Ok(mut handle) = material_handles.get_mut(*entity) else { continue };
            let Some(material) = materials.get(&handle.0) else { continue };
            // Already corrected once: reuse the stored authored base. Without this every colour
            // switch gamma-decoded the corrected colour again and the head/eyes drifted darker.
            let base = if let Ok(marker) = corrected.get(*entity) {
                marker.base
            } else {
                // Unity exported its material colours in GAMMA space into the glTF factors; the
                // loader treats them as linear, which washes every beast/costume material out
                // (gbrender hit the same thing: "the glb had them as linear, which washed
                // everything out"). Convert gamma -> linear once per material, then correct.
                let authored = material.base_color.to_linear();
                let gamma_to_linear = |v: f32| {
                    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
                };
                Vec3::new(
                    gamma_to_linear(authored.red),
                    gamma_to_linear(authored.green),
                    gamma_to_linear(authored.blue),
                )
            };
            let luma = base.x * 0.2126 + base.y * 0.7152 + base.z * 0.0722;
            if luma <= 0.02 {
                continue;
            }
            let correction = beast_albedo_correction();
            let mut adjusted = material.clone();
            adjusted.base_color = Color::linear_rgb(
                base.x * correction[0],
                base.y * correction[1],
                base.z * correction[2],
            );
            adjusted.perceptual_roughness = adjusted.perceptual_roughness.max(0.8);
            adjusted.metallic = 0.0;
            *handle = MeshMaterial3d(materials.add(adjusted));
            commands.entity(*entity).insert(BeastCorrected { base });
        }
        if tinted > 0 {
            applied.insert(scene, chosen);
            info!(
                "player {} uses source color swatch {}",
                player + 1,
                player % sim.player_colors.len() + 1
            );
        } else {
            warn!(
                "player {} primary-color meshes were not found after scene load",
                player + 1
            );
        }
        applied.insert(scene, chosen);
    }
}

fn tint_primary_materials(
    root: Entity,
    color: Color,
    source_color: Color,
    children: &Query<&Children>,
    markers: &Query<&TintedPrimary>,
    material_handles: &mut Query<&mut MeshMaterial3d<StandardMaterial>>,
    materials: &mut Assets<StandardMaterial>,
    commands: &mut Commands,
) -> usize {
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    let mut tinted = 0;
    while let Some(entity) = stack.pop() {
        if !seen.insert(entity) {
            continue;
        }
        let stored = markers.get(entity).ok().map(|marker| marker.base);
        if let Ok(mut handle) = material_handles.get_mut(entity) {
            let replacement = materials.get(&handle.0).and_then(|source| {
                (stored.is_some() || colors_match(source.base_color, source_color)).then(|| {
                    let mut material = source.clone();
                    // Source behaviour (1:1): `BodyHandeler.SetSkinColor` (0x436d50) sets the head
                    // and body material colour to `Actor._primaryColor` — a REPLACE. The authored
                    // prefab colour (0.68/0.22/0.22, a dark red) is only the default for beasts
                    // with no assigned colour, so multiplying it turned every palette swatch into
                    // a shade of red.
                    let tint = color.to_linear();
                    let applied = Vec3::new(tint.red, tint.green, tint.blue);
                    material.base_color = Color::linear_rgb(applied.x, applied.y, applied.z);
                    material.emissive = LinearRgba::BLACK;
                    // Soft vinyl, not metal: the references show a broad dull sheen, no sharp
                    // coloured reflections (VinylOrMetal isn't ported yet).
                    material.metallic = 0.0;
                    material.perceptual_roughness = 0.8;
                    material.reflectance = 0.3;
                    (material, applied)
                })
            });
            if let Some((material, authored)) = replacement {
                *handle = MeshMaterial3d(materials.add(material));
                commands.entity(entity).insert(TintedPrimary { base: authored });
                tinted += 1;
            } else if let Some(material) = materials.get(&handle.0) {
                // The beast's own near-white body material (beast.glb's "White") is what reads
                // blown in the lobby: measured 233,208,203 against the reference's warm cream
                // 214,199,180. Apply the same measured correction as the costumes to the beast's
                // own white albedo, so the naked body sits at the reference level.
                let base = material.base_color.to_linear();
                let luma = base.red * 0.2126 + base.green * 0.7152 + base.blue * 0.0722;
                if luma > 0.75 {
                    let correction = beast_albedo_correction();
                    let mut adjusted = material.clone();
                    adjusted.base_color = Color::linear_rgb(
                        base.red * correction[0],
                        base.green * correction[1],
                        base.blue * correction[2],
                    );
                    adjusted.perceptual_roughness = adjusted.perceptual_roughness.max(0.8);
                    adjusted.metallic = 0.0;
                    *handle = MeshMaterial3d(materials.add(adjusted));
                }
            }
        }
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter());
        }
    }
    tinted
}

/// Beast body albedo correction (linear multipliers) for the near-white body meshes. The
/// authored beast materials are already gamma-decoded to linear, so the authored value is
/// correct; this stays at 1.0 unless a beast-only measurement says otherwise. Earlier values
/// (0.74/0.77/0.80) came from `beast_colour.py`, which sampled scenery in its "beast" cluster —
/// the same reason the beast read pale (see `light_probes::apply_to_beast`). `GB_BEAST_TINT`
/// overrides for A/B.
fn beast_albedo_correction() -> [f32; 3] {
    // Through the knob store so the in-game panel (F2) can change it live.
    crate::devgui::knob_rgb("GB_BEAST_TINT", [1.0, 1.0, 1.0])
}

fn colors_match(a: Color, b: Color) -> bool {    let a = a.to_linear();
    let b = b.to_linear();
    (a.red - b.red).abs() < 1e-4
        && (a.green - b.green).abs() < 1e-4
        && (a.blue - b.blue).abs() < 1e-4
        && (a.alpha - b.alpha).abs() < 1e-4
}

/// Recompute world transforms of every node that has (or sits under) a body and write local
/// Transforms, parents first, so skinning follows the ragdoll.
fn sync_bodies(
    sim: NonSend<Sim>,
    map: Res<NodeEntities>,
    fixed_time: Res<Time<Fixed>>,
    mut transforms: Query<&mut Transform>,
) {
    let alpha = fixed_time.overstep_fraction();
    if std::env::var_os("GB_NO_BODY_SYNC").is_some() {
        return;
    }
    for (scene, (instance, src, root)) in sim.scenes.iter().enumerate() {
        let inst = &sim.world.instances[*instance];
        if inst.bodies.is_empty()
            || !scene_mapping_complete(map.0.keys().copied(), scene, src.nodes.len())
            || !map.0.contains_key(&(scene, 0))
        {
            continue;
        }
        let mut world_of: Vec<Mat4> = Vec::with_capacity(src.nodes.len());
        let mut moved = vec![false; src.nodes.len()];
        for (i, node) in src.nodes.iter().enumerate() {
            let t = &node.transform;
            let local = Mat4::from_scale_rotation_translation(
                Vec3::from_array(t.scale),
                Quat::from_array(t.rotation),
                Vec3::from_array(t.translation),
            );
            // Scene roots sit under an identity SceneRoot; place them at the scene's origin.
            let (parent, parent_moved) = match node.parent {
                Some(p) => (world_of[p], moved[p]),
                None => (*root, *root != Mat4::IDENTITY),
            };
            let w = if let Some(&b) = inst.bodies.get(&i) {
                let pose = sim.body_pose_history.get(&b).map_or_else(
                    || sim.world.pose(b),
                    |&(previous, current)| interpolate_pose(previous, current, alpha),
                );
                let (scale, _, _) = (parent * local).to_scale_rotation_translation();
                moved[i] = true;
                Mat4::from_scale_rotation_translation(
                    scale,
                    mirror_rotation(pose.rotation),
                    mirror_position(pose.position),
                )
            } else {
                moved[i] = parent_moved;
                parent * local
            };
            world_of.push(w);
            if !moved[i] {
                continue;
            }
            let Some(&e) = map.0.get(&(scene, i)) else {
                continue;
            };
            let scene_parent = if node.parent.is_none() {
                Mat4::IDENTITY
            } else {
                parent
            };
            if let Ok(mut tf) = transforms.get_mut(e) {
                *tf = Transform::from_matrix(scene_parent.inverse() * w);
            }
        }
    }
}

fn interpolate_pose(previous: Iso, current: Iso, alpha: f32) -> Iso {
    let alpha = alpha.clamp(0.0, 1.0);
    Iso::new(
        previous.position.lerp(current.position, alpha),
        previous.rotation.slerp(current.rotation, alpha).normalize(),
    )
}

fn tentacle_bone_path(path: &str) -> Option<(u8, usize)> {
    let rest = path.strip_prefix("Tentacles/tentacle (")?;
    let (chain, rest) = rest.split_once(')')?;
    let chain = chain.parse::<u8>().ok()?;
    // Root 6 is intentionally omitted from TentacleMechanics' serialized controller list.
    if chain == 6 {
        return None;
    }
    let segment = rest.strip_prefix("/tentacle_bone_")?;
    // Only accept the exact joint path, not its many sucker/collider descendants.
    if segment.contains('/') {
        return None;
    }
    Some((chain, segment.parse::<usize>().ok()?))
}

fn aquarium_tentacle_bones(src: &Sidecar) -> Vec<TentacleBone> {
    let mut chains: HashMap<u8, Vec<(usize, usize)>> = HashMap::new();
    for (node_index, node) in src.nodes.iter().enumerate() {
        if node.active_in_hierarchy {
            continue;
        }
        let Some((chain, segment)) = tentacle_bone_path(&node.path) else {
            continue;
        };
        chains.entry(chain).or_default().push((segment, node_index));
    }

    let mut bones = Vec::new();
    for (chain, mut segments) in chains {
        segments.sort_unstable_by_key(|(segment, _)| *segment);
        let count = segments.last().map_or(0, |(segment, _)| segment + 1);
        bones.extend(segments.into_iter().map(|(segment, node)| TentacleBone {
            node,
            chain,
            segment,
            count,
        }));
    }
    bones
}

/// The source rig stores radial roots and 20-bone chains, but their controllers and joints are
/// inactive in the stage export. Shape a visual idle arc through the original joints; attacks
/// remain a separate physics-driven task.
fn tentacle_bone_pose(segment: usize, count: usize, seconds: f32, chain: u8) -> (Vec3, f32) {
    if count < 2 {
        return (Vec3::ZERO, 0.0);
    }
    let progress = segment as f32 / (count - 1) as f32;
    let phase = chain as f32 * 1.71;
    let heading = 1.65 * progress
        + 0.055
            * (seconds * 0.72 + phase - progress * 2.0).sin()
            * (std::f32::consts::PI * progress).sin();
    let radius = 5.5;
    (
        Vec3::new(0.0, radius * heading.sin(), radius * (1.0 - heading.cos())),
        heading,
    )
}

fn tentacle_attack_active(seconds: f32, chain: u8) -> bool {
    // Aquarium's controller waits 20s before the first attack, then uses a 10s attack window.
    // The serialized 15..30s start delay produces staggered attacks, not eight arms at once.
    if seconds < 20.0 {
        return false;
    }
    let phase = (seconds - 20.0 + f32::from(chain) * 3.0).rem_euclid(34.0);
    phase < 10.0
}

fn animate_tentacles(
    sim: NonSend<Sim>,
    map: Res<NodeEntities>,
    time: Res<Time>,
    mut transforms: Query<&mut Transform>,
) {
    if sim.tentacle_bones.is_empty() {
        return;
    }
    let Some((_, source, _)) = sim.scenes.first() else {
        return;
    };
    for bone in &sim.tentacle_bones {
        let Some(source_node) = source.nodes.get(bone.node) else {
            continue;
        };
        let Some(&entity) = map.0.get(&(0, bone.node)) else {
            continue;
        };
        let Ok(mut transform) = transforms.get_mut(entity) else {
            continue;
        };
        // The shipped screenshot shows the Aquarium arms lifted over the tank even between
        // attacks. The exported controllers are disabled, so keep their source pivots and apply
        // the light render-only arc continuously instead of leaving them in a straight bind pose.
        let (mut position, heading) =
            tentacle_bone_pose(bone.segment, bone.count, time.elapsed_secs(), bone.chain);
        if tentacle_attack_active(time.elapsed_secs(), bone.chain) {
            position *= 1.2;
        }
        let source_position = Vec3::from_array(source_node.transform.translation);
        transform.translation = Vec3::new(source_position.x, position.y, position.z);
        transform.rotation =
            Quat::from_array(source_node.transform.rotation) * Quat::from_rotation_x(heading);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        camera_framing, camera_obstacle_radius, camera_offset, colors_match, composer_aim,
        interpolate_pose, scene_mapping_complete, tentacle_attack_active, tentacle_bone_path,
        tentacle_bone_pose, Color, Iso, Quat, Vec3,
    };

    #[test]
    fn ragdoll_sync_waits_for_every_scene_node() {
        let partially_spawned = vec![(0, 0), (0, 1), (0, 2), (1, 0)];
        assert!(!scene_mapping_complete(
            partially_spawned.clone().into_iter(),
            0,
            41
        ));
        assert!(scene_mapping_complete(partially_spawned.into_iter(), 1, 1));

        let complete = (0..41).map(|node| (0, node));
        assert!(scene_mapping_complete(complete, 0, 41));
        assert!(!scene_mapping_complete(
            (0..41).map(|node| (0, node)),
            1,
            41
        ));
    }

    #[test]
    fn ragdoll_render_pose_interpolates_between_fixed_steps() {
        let previous = Iso::new(Vec3::ZERO, Quat::IDENTITY);
        let current = Iso::new(
            Vec3::new(2.0, 4.0, 6.0),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        );
        let halfway = interpolate_pose(previous, current, 0.5);
        assert_eq!(halfway.position, Vec3::new(1.0, 2.0, 3.0));
        assert!(halfway.rotation.is_normalized());
        assert!(
            halfway
                .rotation
                .dot(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4))
                .abs()
                > 0.99999
        );
        assert_eq!(
            interpolate_pose(previous, current, -1.0).position,
            previous.position
        );
        assert_eq!(
            interpolate_pose(previous, current, 2.0).position,
            current.position
        );
    }

    #[test]
    fn aquarium_group_camera_dollies_out_along_the_full_authored_orbit() {
        let anchor = Vec3::new(0.0, 8.0, 14.0);
        let authored_orbit = Vec3::new(6.0, 8.0, 9.6);
        let offset = camera_offset(anchor, authored_orbit, 7.75, 40.0, 0.8, 10.0, 200.0);
        assert!((offset.length() - 30.0).abs() < 0.2);
        assert!((offset.y - anchor.y).abs() < 0.01);
        assert!(offset.x > 0.0 && offset.z > 0.0);
        let single = camera_offset(
            anchor,
            Vec3::new(0.0, 8.0, 10.0),
            0.0,
            40.0,
            0.8,
            10.0,
            200.0,
        );
        assert!((single.length() - anchor.length()).abs() < 0.01);
    }

    #[test]
    fn composer_screen_y_maps_as_a_fraction_of_the_vertical_fov() {
        let target = Vec3::ZERO;
        let camera = Vec3::new(0.0, 0.0, 20.0);
        let centered = composer_aim(target, camera, 40.0, 0.5);
        assert!(centered.abs_diff_eq(target, 1e-5));

        let framed = composer_aim(target, camera, 40.0, 0.6);
        let expected = 20.0 * 0.2 * (20.0f32.to_radians()).tan();
        assert!((framed.y + expected).abs() < 1e-4);
        assert!(
            expected < 2.0,
            "ScreenY 0.6 should shift by 10% of frame height"
        );
    }

    #[test]
    fn aquarium_uses_the_serialized_group_composer_screen_y() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/export");
        let aquarium = gb_phys::Sidecar::load(&root, "aquarium").unwrap();
        let framing = camera_framing(&aquarium).unwrap();
        assert!((framing.screen_y - 0.6).abs() < 1e-5);
        assert!((framing.group_size - 0.8).abs() < 1e-5);
        assert!((framing.min_distance - 10.0).abs() < 1e-5);
        assert!((framing.max_distance - 200.0).abs() < 1e-5);
        assert!((framing.max_fov - 40.0).abs() < 1e-5);
        assert_eq!(camera_obstacle_radius(&aquarium), Some(4.0));
    }

    #[test]
    fn aquarium_tentacle_idle_arc_runs_over_the_rim_and_sways() {
        let count = 20;
        let root = tentacle_bone_pose(0, count, 0.0, 3);
        let crest = tentacle_bone_pose(count / 2, count, 0.0, 3);
        let tip = tentacle_bone_pose(count - 1, count, 0.0, 3);
        let moving_crest = tentacle_bone_pose(count / 2, count, 1.0, 3);
        assert!(root.0.length() < 1e-5);
        assert!(
            crest.0.y > 3.5,
            "crest should rise above the tank rim: {:?}",
            crest.0
        );
        assert!(
            tip.0.z > 5.5,
            "tip should extend over the gallery rim: {:?}",
            tip.0
        );
        assert!(
            tip.1 > std::f32::consts::FRAC_PI_2,
            "tip should curl down over the edge"
        );
        assert!(
            (crest.0 - moving_crest.0).length() > 1e-3,
            "idle tentacle should sway over time"
        );
    }

    #[test]
    fn aquarium_tentacle_attacks_are_delayed_and_staggered() {
        assert!(!(0..8).any(|chain| tentacle_attack_active(19.99, chain)));
        assert!(tentacle_attack_active(20.0, 0));
        assert!(tentacle_attack_active(20.0, 2));
        assert!(!tentacle_attack_active(20.0, 4));
        assert!(tentacle_attack_active(22.0, 2));
        assert!(!tentacle_attack_active(26.0, 2));
        assert!(!tentacle_attack_active(31.0, 0));
    }

    #[test]
    fn tentacle_rig_mapping_selects_joint_nodes_only() {
        assert_eq!(
            tentacle_bone_path("Tentacles/tentacle (7)/tentacle_bone_03"),
            Some((7, 3))
        );
        assert_eq!(
            tentacle_bone_path("Tentacles/tentacle (7)/tentacle_bone_03/tentacle_sucker18"),
            None
        );
        assert_eq!(
            tentacle_bone_path("Tentacles/tentacle (6)/tentacle_bone_03"),
            None
        );
    }

    #[test]
    fn source_primary_color_matches_only_the_authored_tint_material() {
        let primary = Color::linear_rgba(0.6838235, 0.21620888, 0.21620888, 1.0);
        let eye_white = Color::linear_rgba(1.0, 1.0, 1.0, 1.0);
        assert!(colors_match(primary, primary));
        assert!(!colors_match(eye_white, primary));
    }
}

/// GlobalCameraMarker + GangBeastsGroupComposer: sit at the selected offset from the group's
/// centre (D-pad / Q-E switch offsets), obeying its source FOV and distance limits.
fn follow_camera(
    mut sim: NonSendMut<Sim>,
    time: Res<Time>,
    mut cams: Query<(&mut Transform, &mut Projection), With<FollowCam>>,
) {
    if sim.lobby {
        return;
    }
    let hips: Vec<Vec3> = sim
        .actors
        .iter()
        .filter(|a| a.state != gb_logic::actor::state::DEAD)
        .map(|a| sim.world.pose(a.beast.body(Part::Hips)).position)
        .filter(|p| p.y > -50.0)
        .collect();
    if hips.is_empty() {
        return;
    }
    let center = hips.iter().copied().sum::<Vec3>() / hips.len() as f32;
    let spread = hips.iter().map(|p| p.distance(center)).fold(0.0, f32::max);
    let offset = sim.cam_offsets[sim.cam_index];
    let fov = sim.cam_framing.max_fov.to_radians();
    // GangBeastsGroupComposer can dolly out beyond the marker's local orbit range to fit all
    // players. Preserve the full authored camera-offset direction as it scales with that fit.
    let camera_offset = camera_offset(
        sim.cam_anchor_offset,
        offset,
        spread,
        fov.to_degrees(),
        sim.cam_framing.group_size,
        sim.cam_framing.min_distance,
        sim.cam_framing.max_distance,
    );
    // Single-player framing sits a hair close; pull the authored offset out slightly (GB_CAM_ZOOM
    // overrides, 1.0 = authored). Group play keeps the authored fit so multi-player framing is
    // untouched.
    let zoom: f32 = crate::devgui::knob(
        "GB_CAM_ZOOM",
        if sim.actors.len() <= 1 { 1.06 } else { 1.0 },
    );
    let camera_offset = camera_offset * zoom;
    let distance = camera_offset.length();
    let desired_unity = center + camera_offset;
    // CinemachineCollider: pull the camera in front of level geometry (subway walls etc.). The
    // authored radius (4) swept from the players' feet hits the floor right next to them and
    // collapsed the camera onto the players, so the sweep uses a slim sphere and never pulls
    // closer than `GB_CAM_MIN_FRAC` of the framing distance. `GB_NO_CAM_CLEARANCE=1` disables.
    let clear_distance = if std::env::var_os("GB_NO_CAM_CLEARANCE").is_some() {
        f32::INFINITY
    } else {
        let radius = std::env::var("GB_CAM_RADIUS")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.6)
            .min(sim.cam_obstacle_radius);
        let min_frac = std::env::var("GB_CAM_MIN_FRAC")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.45);
        sim.world.camera_clearance(
            center,
            desired_unity,
            sim.cam_obstacle_mask,
            radius,
            distance * min_frac,
        )
    };
    let goal_unity = center + camera_offset.normalize() * distance.min(clear_distance);
    let (goal, target) = (mirror_position(goal_unity), mirror_position(center));
    static CAMERA_DEBUGGED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    let camera_debug =
        std::env::var_os("GB_CAM_DEBUG").is_some() && CAMERA_DEBUGGED.set(()).is_ok();
    for (mut tf, mut proj) in &mut cams {
        if let Projection::Perspective(p) = &mut *proj {
            p.fov = fov;
        }
        // OrbitLerpSpeed 0.075 per frame at 60 fps.
        let k = 1.0 - (1.0f32 - 0.075).powf(time.delta_secs() * 60.0);
        tf.translation = tf.translation.lerp(goal, k);
        // Keep the smoothed camera outside level geometry while it catches up to a new target.
        let from_target = tf.translation - target;
        let safe_distance = clear_distance.min(distance);
        if from_target.length() > safe_distance {
            tf.translation = target + from_target.normalize_or_zero() * safe_distance;
        }
        // Cinemachine screenY is a normalized viewport coordinate (0.5 is centered). Convert
        // the authored offset to a screen-plane distance using the vertical FOV.
        let aim = composer_aim(
            target,
            tf.translation,
            fov.to_degrees(),
            sim.cam_framing.screen_y,
        );
        if let Some((centre, rotation, half)) = sim.cam_confiner {
            let local = rotation.inverse() * (tf.translation - centre);
            tf.translation = centre + rotation * local.clamp(-half, half);
        }
        tf.look_at(aim, Vec3::Y);
        // Debug: GB_CAM_EYE / GB_CAM_AT = "x,y,z" (Unity space) pin the camera for captures.
        let parse = |k: &str| {
            std::env::var(k).ok().and_then(|v| {
                let f: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                (f.len() == 3).then(|| mirror_position(Vec3::new(f[0], f[1], f[2])))
            })
        };
        if let (Some(eye), Some(at)) = (parse("GB_CAM_EYE"), parse("GB_CAM_AT")) {
            *tf = Transform::from_translation(eye).looking_at(at, Vec3::Y);
        }
        if camera_debug {
            eprintln!(
                "follow camera: actors={} hips={:?} center={center:?} spread={spread:.2} anchor={:?} framing={:?} orbit={offset:?} distance={distance:.2} clearance={clear_distance:.2} goal={goal:?} aim={aim:?} camera={:?}",
                hips.len(), hips, sim.cam_anchor_offset,
                sim.cam_framing,
                tf.translation
            );
        }
        let r = *tf.right();
        let f = (*tf.forward()).with_y(0.0).normalize_or_zero();
        sim.cam_right = mirror_position(r.with_y(0.0).normalize_or_zero());
        sim.cam_forward = mirror_position(f);
    }
}

fn composer_aim(target: Vec3, camera: Vec3, fov_degrees: f32, screen_y: f32) -> Vec3 {
    let view = (target - camera).normalize_or_zero();
    let right = view.cross(Vec3::Y).normalize_or_zero();
    let screen_up = right.cross(view).normalize_or_zero();
    let screen_fraction = (screen_y - 0.5) * 2.0;
    let distance = target.distance(camera);
    let offset = distance * screen_fraction * (0.5 * fov_degrees.to_radians()).tan();
    target - screen_up * offset
}

fn camera_offset(
    anchor: Vec3,
    orbit_offset: Vec3,
    spread: f32,
    fov_degrees: f32,
    group_size: f32,
    minimum: f32,
    maximum: f32,
) -> Vec3 {
    let needed = (spread + 1.0) / (group_size * (fov_degrees.to_radians() * 0.5).tan());
    let distance = anchor.length().max(needed).clamp(minimum, maximum);
    let height = anchor.y.clamp(3.0, distance - 0.5);
    let horizontal = (distance * distance - height * height).sqrt();
    let orbit = Vec3::new(orbit_offset.x, 0.0, orbit_offset.z).normalize_or_zero();
    orbit * horizontal + Vec3::Y * height
}

/// LT / RT (or Tab): ActorNameBar over each beast: "Player N" in Liberation Sans (TMP size 32,
/// character spacing 3, centred in a 320 px box), tinted with the player's colour.
fn nametags(
    sim: NonSend<Sim>,
    assets: Res<AssetServer>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    cams: Query<(&Camera, &GlobalTransform), With<FollowCam>>,
    mut tags: Query<(Entity, &Nametag, &mut Node, &mut Visibility)>,
    mut commands: Commands,
) {
    let show = keys.pressed(KeyCode::Tab)
        || pads.iter().any(|g| {
            g.pressed(GamepadButton::LeftTrigger2) || g.pressed(GamepadButton::RightTrigger2)
        });
    let have: Vec<usize> = tags.iter().map(|(_, t, _, _)| t.0).collect();
    for k in 0..sim.actors.len() {
        if !have.contains(&k) {
            commands.spawn((
                Text::new(format!("Player {}", k + 1)),
                TextFont {
                    font: assets.load("ui/fonts/LiberationSans.ttf"),
                    font_size: 28.0,
                    ..default()
                },
                TextColor(sim.player_color(k)),
                TextLayout::new_with_justify(JustifyText::Center),
                TextShadow {
                    offset: Vec2::splat(2.0),
                    color: Color::BLACK.with_alpha(0.8),
                },
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(320.0),
                    ..default()
                },
                Visibility::Hidden,
                Nametag(k),
            ));
        }
    }
    let Ok((camera, cam_tf)) = cams.single() else {
        return;
    };
    for (_, tag, mut node, mut vis) in &mut tags {
        let Some(actor) = sim.actors.get(tag.0) else {
            continue;
        };
        if sim.parked.get(tag.0).copied().unwrap_or(false) {
            *vis = Visibility::Hidden;
            continue;
        }
        let head =
            mirror_position(sim.world.pose(actor.beast.body(Part::Head)).position) + Vec3::Y * 0.6;
        match camera.world_to_viewport(cam_tf, head) {
            Ok(p) if show => {
                node.left = Val::Px(p.x - 160.0);
                node.top = Val::Px(p.y - 19.0);
                *vis = Visibility::Visible;
            }
            _ => *vis = Visibility::Hidden,
        }
    }
}

#[derive(Component)]
struct Nametag(usize);

/// Lobby: hide the render roots of backed-out beasts.
fn hide_parked(sim: NonSend<Sim>, mut roots: Query<(&PhysicsScene, &mut Visibility)>) {
    for (scene, mut vis) in &mut roots {
        let Some(actor) = sim.actor_scene.iter().position(|s| *s == scene.0) else {
            continue;
        };
        let want = if sim.parked[actor] {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *vis != want {
            *vis = want;
        }
    }
}

fn spawn_hud(mut commands: Commands) {
    commands.spawn((
        Text::new(""),
        TextFont {
            font_size: 16.0,
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(10.0),
            padding: UiRect::all(Val::Px(6.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
        Visibility::Visible,
        Hud,
    ));
}

#[derive(Component)]
struct PauseOverlay;

fn spawn_pause_ui(mut commands: Commands, assets: Res<AssetServer>) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(18.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.025, 0.03, 0.045, 0.82)),
            Visibility::Hidden,
            PauseOverlay,
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new("PAUSED"),
                TextFont {
                    font: assets.load("ui/fonts/NotoSans-Black.ttf"),
                    font_size: 48.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                TextLayout::new_with_justify(JustifyText::Center),
            ));
            panel.spawn((
                Text::new("ESC / START   RESUME\nM   MAIN MENU\nQ   QUIT"),
                TextFont {
                    font: assets.load("ui/fonts/NotoSans-Black.ttf"),
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(0.92, 0.72, 0.18)),
                TextLayout::new_with_justify(JustifyText::Center),
            ));
        });
}

fn toggle_pause(
    mut sim: NonSendMut<Sim>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    mut overlay: Query<&mut Visibility, With<PauseOverlay>>,
    mut exit: EventWriter<AppExit>,
) {
    if sim.lobby {
        return;
    }
    let pad_start = pads
        .iter()
        .any(|pad| pad.just_pressed(GamepadButton::Start));
    if keys.just_pressed(KeyCode::Escape) || pad_start {
        sim.paused = !sim.paused;
        for mut visibility in &mut overlay {
            *visibility = if sim.paused {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
        }
    }
    if !sim.paused {
        return;
    }
    if keys.just_pressed(KeyCode::KeyM) {
        match std::env::current_exe()
            .and_then(|exe| std::process::Command::new(exe).arg("--menu").spawn())
        {
            Ok(_) => {
                exit.write(AppExit::Success);
            }
            Err(error) => error!("could not open the local menu: {error}"),
        }
    } else if keys.just_pressed(KeyCode::KeyQ) {
        exit.write(AppExit::Success);
    }
}

fn update_hud(
    sim: NonSend<Sim>,
    keys: Res<ButtonInput<KeyCode>>,
    // Keep developer controls out of the match by default; F1 reveals them.
    mut show_debug: Local<bool>,
    mut hud: Query<(&mut Text, &mut Visibility), With<Hud>>,
    gamepads: Query<&Gamepad>,
) {
    if keys.just_pressed(KeyCode::F1) {
        *show_debug = !*show_debug;
    }
    let visible = *show_debug;
    let state = |s: u32| match s {
        1 => "dead",
        2 => "KO",
        4 => "stand",
        8 => "run",
        16 => "jump",
        32 => "fall",
        64 => "climb",
        128 => "swim",
        256 => "drive",
        512 => "idle",
        _ => "?",
    };
    let mut s = format!(
        "beasts: {}   keyboard -> #{}   [ / ] switch   N spawn NPC   R respawn   Q/E camera   Tab names   F9 drives: {}
",
        sim.actors.len(),
        sim.kbm + 1,
        if sim.world.drive_limits_are_forces { "force" } else { "impulse" }
    );
    let mut pads: Vec<usize> = sim.pads.values().copied().collect();
    pads.sort_unstable();
    for (i, p) in pads.iter().enumerate() {
        s += &format!("pad {} -> #{}   ", i + 1, p + 1);
    }
    if !pads.is_empty() {
        s += "(L3/R3 switch beast, D-pad camera, LT/RT nametags, Select spawn, Start respawn)\n";
    }
    for (i, a) in sim.actors.iter().enumerate() {
        let c = &a.control;
        let mut flags = String::new();
        for (on, name) in [
            (c.duck, " duck"),
            (c.run, " sprint"),
            (c.headbutt, " headbutt"),
            (c.hands[0].grab || c.hands[1].grab, " grab"),
            (c.hands[0].punch || c.hands[1].punch, " punch"),
            (
                c.hands[0].joint.is_some() || c.hands[1].joint.is_some(),
                " holding",
            ),
            (!c.on_ground, " air"),
        ] {
            if on {
                flags += name;
            }
        }
        s += &format!(
            "#{} {}{}  hp {:.0} st {:.0}{}   ",
            i + 1,
            state(a.state),
            flags,
            a.status.health,
            a.status.stamina,
            if a.state == 2 {
                format!(" KO {:.1}s", a.status.unconscious_time)
            } else {
                String::new()
            }
        );
    }
    for g in &gamepads {
        let held: Vec<String> = g.get_pressed().map(|b| format!("{b:?}")).collect();
        if !held.is_empty() {
            s += &format!(
                "
pad buttons: {}",
                held.join(" ")
            );
        }
    }
    for (mut t, mut visibility) in &mut hud {
        t.0 = s.clone();
        *visibility = if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}
