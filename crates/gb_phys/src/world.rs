//! PhysX 4.1 scene configured the way Unity 2021.3 configures it, plus the sidecar importer.
use crate::source::{flag, num, Material, Pose, Settings, Sidecar};
use crate::unity::{self, DriveAxis, Iso, JointDesc, Limit, Motion, Shape};
use glam::{Quat, Vec3};
use physx_sys::*;
use std::collections::HashMap;
use std::ptr::{null, null_mut};

const PX_PHYSICS_VERSION: u32 = (4 << 24) | (1 << 16) | (1 << 8);
const TRIGGER: u32 = 1;

/// BodyBase's `Physics.IgnoreCollision` pairs (`re/decomp/body_v2.c`, 27 unique pairs — the
/// actor disables collision between parts two links apart and inside the torso cluster). Without
/// them adjacent-but-not-joint-connected parts (chest vs arms/head, hips vs thighs, arm vs
/// forearm) push each other and the ragdoll bends/pops — the "self snag" report.
/// Indices are into [`BODY_PARTS`]; 1-based ids go into the shape's filter word3 (0 = not a part).
const BODY_PARTS: [&str; 16] = [
    "Chest", "Head", "LeftArm", "LeftForarm", "RightArm", "RightForarm", "Stomach", "Crotch",
    "Waist", "Hips", "LeftThigh", "LeftLeg", "RightThigh", "RightLeg", "LeftHand", "RightHand",
];
const IGNORE_PAIRS: [(u8, u8); 27] = [
    (0, 1), (0, 2), (0, 3), (0, 4), (0, 5), (0, 6), (0, 7), (0, 9),
    (6, 7), (6, 9), (7, 9),
    (9, 10), (9, 11), (9, 12), (9, 13),
    (2, 3), (2, 14), (3, 14),
    (4, 5), (4, 15), (5, 15),
    (6, 10), (7, 10), (10, 11),
    (6, 12), (7, 12), (12, 13),
];

/// Pair mask per part: bit `j` set = part `i` never collides with part `j`.
const fn ignore_masks() -> [u16; 16] {
    let mut masks = [0u16; 16];
    let mut i = 0;
    while i < IGNORE_PAIRS.len() {
        let (a, b) = IGNORE_PAIRS[i];
        masks[a as usize] |= 1 << b;
        masks[b as usize] |= 1 << a;
        i += 1;
    }
    masks
}
const IGNORE_MASK: [u16; 16] = ignore_masks();

/// Filter word3 for a node name: 1..=16 for actor body parts (`actor_<part>_collider`), 0 otherwise.
fn body_part_id(node_name: &str) -> u32 {
    let stem = node_name.strip_prefix("actor_").unwrap_or(node_name);
    let stem = stem.split('_').next().unwrap_or(stem);
    BODY_PARTS
        .iter()
        .position(|part| part.eq_ignore_ascii_case(stem))
        .map_or(0, |i| i as u32 + 1)
}

/// Whether the source actor ignores collisions between these two nodes (BodyBase's
/// `Physics.IgnoreCollision` pairs). Same-actor only — the runtime filter packs the instance id
/// with the part id for exactly that reason.
pub fn ignore_pair(name_a: &str, name_b: &str) -> bool {
    let (a, b) = (body_part_id(name_a), body_part_id(name_b));
    a != 0 && b != 0 && IGNORE_MASK[(a - 1) as usize] & (1 << (b - 1)) != 0
}

/// Unity layer matrix: word0 = own layer bit, word1 = layers it collides with, word2 = trigger,
/// word3 = (instance << 5) | actor body-part id. The instance is packed in so BodyBase's
/// IgnoreCollision pairs only apply WITHIN one actor — without it beast A's chest was suppressed
/// against beast B's head and the beasts interpenetrated.
unsafe extern "C" fn layer_filter(info: *mut FilterShaderCallbackInfo) -> u16 {
    let info = &mut *info;
    let (a, b) = (info.filterData0, info.filterData1);
    if a.word0 & b.word1 == 0 || b.word0 & a.word1 == 0 {
        return PxFilterFlag::eSUPPRESS as u16;
    }
    // Physics.IgnoreCollision(partA, partB) from the source actor setup, same actor only.
    let (pa, pb) = (a.word3 & 0x1f, b.word3 & 0x1f);
    if a.word3 >> 5 == b.word3 >> 5 && pa != 0 && pb != 0 && pa <= 16 && pb <= 16 {
        if IGNORE_MASK[(pa - 1) as usize] & (1 << (pb - 1)) != 0 {
            return PxFilterFlag::eSUPPRESS as u16;
        }
    }
    (*info.pairFlags).mBits = if (a.word2 | b.word2) & TRIGGER != 0 {
        PxPairFlag::eTRIGGER_DEFAULT
    } else {
        PxPairFlag::eCONTACT_DEFAULT
            | PxPairFlag::eNOTIFY_TOUCH_FOUND
            | PxPairFlag::eNOTIFY_TOUCH_PERSISTS
            | PxPairFlag::eNOTIFY_TOUCH_LOST
            | PxPairFlag::eNOTIFY_CONTACT_POINTS
    } as u16;
    PxFilterFlag::eDEFAULT as u16
}

fn px_vec(v: Vec3) -> PxVec3 {
    PxVec3 {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}
fn px_iso(i: Iso) -> PxTransform {
    let q = i.rotation.normalize();
    unsafe { PxTransform_new_5(&px_vec(i.position), &PxQuat_new_3(q.x, q.y, q.z, q.w)) }
}
fn force_mode(unity: u32) -> PxForceMode::Enum {
    match unity {
        1 => PxForceMode::eIMPULSE,
        2 => PxForceMode::eVELOCITY_CHANGE,
        5 => PxForceMode::eACCELERATION,
        _ => PxForceMode::eFORCE,
    }
}
fn from_px(t: PxTransform) -> Iso {
    Iso::new(
        Vec3::new(t.p.x, t.p.y, t.p.z),
        Quat::from_xyzw(t.q.x, t.q.y, t.q.z, t.q.w),
    )
}

/// A simulated rigidbody and the sidecar node that owns it.
pub struct Body {
    pub instance: usize,
    pub node: usize,
    pub name: String,
    pub kinematic: bool,
    /// Taken out of the simulation (World::remove_body).
    pub removed: bool,
    actor: *mut PxRigidDynamic,
}

pub struct Instance {
    pub name: String,
    /// sidecar node index -> body index
    pub bodies: HashMap<usize, usize>,
    pub statics: usize,
    pub joints: usize,
    pub warnings: Vec<String>,
}

/// One Unity Collider as a PhysX shape (for Collider.bounds / ClosestPoint queries).
pub struct ColliderRef {
    pub instance: usize,
    /// Sidecar node carrying the Collider component.
    pub node: usize,
    pub body: Option<usize>,
    pub trigger: bool,
    /// Box/sphere/capsule/convex: Collider.ClosestPoint works (not on concave meshes).
    pub convex: bool,
    shape: *mut PxShape,
    actor: *mut PxRigidActor,
}

/// What a PhysX actor is, for event reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActorRef {
    pub instance: usize,
    /// Sidecar node the actor was built for (the Rigidbody's node, or the static collider's node).
    pub node: usize,
    pub body: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactKind {
    Enter,
    Stay,
    Exit,
}

#[derive(Clone, Copy, Debug)]
pub struct ContactPoint {
    pub position: Vec3,
    /// Points from `other` toward `this` (Unity's ContactPoint.normal as seen by `this`).
    pub normal: Vec3,
    pub impulse: Vec3,
    pub separation: f32,
}

/// One Unity OnCollisionEnter/Stay/Exit for an actor pair, reported from `this`'s side.
#[derive(Clone, Debug)]
pub struct Contact {
    pub kind: ContactKind,
    pub this: usize,
    pub other: usize,
    pub points: Vec<ContactPoint>,
    /// Collision.relativeVelocity: other's velocity minus this one's, from before the step.
    pub relative_velocity: Vec3,
}

struct RawPair {
    actors: [usize; 2],
    found: u32,
    persists: u32,
    lost: u32,
    points: Vec<ContactPoint>,
}

unsafe extern "C" fn on_contact(
    user: *mut std::ffi::c_void,
    header: *const PxContactPairHeader,
    pairs: *const PxContactPair,
    count: u32,
) {
    let raw = &mut *(user as *mut Vec<RawPair>);
    let header = &*header;
    let id = |a: *mut PxRigidActor| {
        if a.is_null() {
            0
        } else {
            (*(a as *mut PxActor)).userData as usize
        }
    };
    let actors = [id(header.actors[0]), id(header.actors[1])];
    if actors[0] == 0 || actors[1] == 0 {
        return; // removed actor
    }
    let mut entry = RawPair {
        actors,
        found: 0,
        persists: 0,
        lost: 0,
        points: Vec::new(),
    };
    let mut buffer = [std::mem::zeroed::<PxContactPairPoint>(); 32];
    for k in 0..count as usize {
        let pair = &*pairs.add(k);
        let ev = pair.events.mBits as u32;
        if ev & PxPairFlag::eNOTIFY_TOUCH_FOUND != 0 {
            entry.found += 1;
        }
        if ev & PxPairFlag::eNOTIFY_TOUCH_PERSISTS != 0 {
            entry.persists += 1;
        }
        if ev & PxPairFlag::eNOTIFY_TOUCH_LOST != 0 {
            entry.lost += 1;
        }
        let n = PxContactPair_extractContacts(pair, buffer.as_mut_ptr(), buffer.len() as u32);
        for p in &buffer[..n as usize] {
            entry.points.push(ContactPoint {
                position: Vec3::new(p.position.x, p.position.y, p.position.z),
                normal: Vec3::new(p.normal.x, p.normal.y, p.normal.z),
                impulse: Vec3::new(p.impulse.x, p.impulse.y, p.impulse.z),
                separation: p.separation,
            });
        }
    }
    raw.push(entry);
}

pub struct World {
    pub settings: Settings,
    physics: *mut PxPhysics,
    cooking: *mut PxCooking,
    dispatcher: *mut PxDefaultCpuDispatcher,
    scene: *mut PxScene,
    materials: Vec<(Option<Material>, *mut PxMaterial)>,
    pub bodies: Vec<Body>,
    pub instances: Vec<Instance>,
    pub steps: u64,
    /// Indexed by PhysX actor userData - 1.
    pub actors: Vec<ActorRef>,
    /// Every collider created, in creation order.
    pub colliders: Vec<ColliderRef>,
    /// Collision events from the last step, one per side of each rigidbody pair.
    pub contacts: Vec<Contact>,
    touching: HashMap<(usize, usize), u32>,
    raw: Box<Vec<RawPair>>,
    events: *mut PxSimulationEventCallback,
    pre_velocity: Vec<Vec3>,
    /// Velocity changes queued this step by add_force (for pre-solver relative velocities).
    pending_dv: Vec<Vec3>,
    /// Joints created at runtime (grabs); None once removed.
    runtime_joints: Vec<Option<*mut PxD6Joint>>,
    all_joints: Vec<*mut PxD6Joint>,
    /// Body pairs joined by a joint (own, connected), for the ragdoll self-intersection debug
    /// overlay: a penetrating pair that is NOT joint-connected is a real snag.
    pub joint_links: Vec<(Option<usize>, Option<usize>)>,
    /// JointBreakSyncTrigger.InvulnJoint: scene joints held unbreakable until the round starts,
    /// with their cached break force/torque.
    invulnerable_joints: Vec<(*mut PxD6Joint, f32, f32)>,
    /// PxConstraintFlag::eDRIVE_LIMITS_ARE_FORCES: whether JointDrive.maximumForce is a force
    /// (true) or a per-step impulse (false). Which one Unity 2021.3 uses is unverified.
    pub drive_limits_are_forces: bool,
}

/// PhysX allows one foundation per process; every World shares it (and the SDK/cooking objects).
struct Sdk {
    _foundation: *mut PxFoundation, // kept alive for the process
    physics: *mut PxPhysics,
    cooking: *mut PxCooking,
}
unsafe impl Send for Sdk {}
unsafe impl Sync for Sdk {}
static SDK: std::sync::OnceLock<Option<Sdk>> = std::sync::OnceLock::new();

fn sdk() -> Option<&'static Sdk> {
    SDK.get_or_init(|| unsafe {
        let foundation = physx_create_foundation();
        let physics = physx_create_physics(foundation);
        if physics.is_null() {
            return None;
        }
        let params = PxCookingParams_new(PxPhysics_getTolerancesScale(physics));
        let cooking = phys_PxCreateCooking(PX_PHYSICS_VERSION, foundation, &params);
        Some(Sdk {
            _foundation: foundation,
            physics,
            cooking,
        })
    })
    .as_ref()
}

impl World {
    /// Collider.bounds: the collider's world AABB as (center, extents).
    pub fn collider_bounds(&self, c: usize) -> (Vec3, Vec3) {
        let c = &self.colliders[c];
        let b = unsafe { PxShapeExt_getWorldBounds_mut(c.shape, c.actor, 1.0) };
        let (min, max) = (
            Vec3::new(b.minimum.x, b.minimum.y, b.minimum.z),
            Vec3::new(b.maximum.x, b.maximum.y, b.maximum.z),
        );
        ((min + max) * 0.5, (max - min) * 0.5)
    }

    /// Collider.ClosestPointOnBounds
    pub fn closest_point_on_bounds(&self, c: usize, point: Vec3) -> Vec3 {
        let (center, extents) = self.collider_bounds(c);
        point.clamp(center - extents, center + extents)
    }

    /// Collider.ClosestPoint: the point itself when inside, or for concave mesh colliders (which
    /// Unity doesn't support either).
    pub fn closest_point(&self, c: usize, point: Vec3) -> Vec3 {
        let c = &self.colliders[c];
        if !c.convex {
            return point;
        }
        unsafe {
            let pose = PxShapeExt_getGlobalPose_mut(c.shape, c.actor);
            let holder = PxShape_getGeometry(c.shape);
            let mut out = px_vec(point);
            let d = PxGeometryQuery_pointDistance_mut(
                &px_vec(point),
                PxGeometryHolder_any(&holder),
                &pose,
                &mut out,
            );
            if d <= 0.0 {
                point
            } else {
                Vec3::new(out.x, out.y, out.z)
            }
        }
    }

    /// Clamp a camera position to the first source-selected static collider touched by the camera's
    /// sphere. CinemachineCollider uses a camera radius, so a centerline ray misses nearby roofs.
    pub fn camera_clearance(
        &self,
        origin: Vec3,
        destination: Vec3,
        obstacle_mask: u32,
        camera_radius: f32,
        minimum_distance: f32,
    ) -> f32 {
        let delta = destination - origin;
        let distance = delta.length();
        if distance <= 0.01 {
            return distance;
        }

        unsafe {
            const MAX_CAMERA_HITS: usize = 256;
            let filter = PxSceneQueryFilterData {
                data: PxFilterData_new_1(),
                flags: PxQueryFlags {
                    // Treat every shape as a touch so PhysX returns all intersections; select
                    // the nearest source-camera-blocking layer below.
                    mBits: (PxQueryFlag::eSTATIC | PxQueryFlag::eNO_BLOCK) as u16,
                },
                structgen_pad0: [0; 2],
            };
            let mut hits = [PxSweepHit_new(); MAX_CAMERA_HITS];
            let mut blocking_hit = false;
            let sphere = PxSphereGeometry_new_1(camera_radius.max(0.01));
            let geometry = &sphere as *const _ as *const PxGeometry;
            let pose = px_iso(Iso::new(origin, Quat::IDENTITY));
            let count = PxSceneQueryExt_sweepMultiple_mut(
                self.scene,
                geometry,
                &pose,
                &px_vec(delta / distance),
                distance,
                PxSceneQueryFlags { mBits: 0 },
                hits.as_mut_ptr(),
                hits.len() as u32,
                &mut blocking_hit,
                &filter,
                null_mut(),
                null(),
                0.0,
            );
            let hits = &hits[..(count.max(0) as usize).min(hits.len())];
            static CAMERA_DEBUGGED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
            if std::env::var_os("GB_CAMERA_DEBUG").is_some() && CAMERA_DEBUGGED.set(()).is_ok() {
                for hit in hits.iter().filter(|hit| !hit.shape.is_null()) {
                    let layer_bit = PxShape_getQueryFilterData(hit.shape).word0;
                    let layer = layer_bit.trailing_zeros() as usize;
                    let actor_id = (*hit.actor).userData as usize;
                    let owner = actor_id
                        .checked_sub(1)
                        .and_then(|index| self.actors.get(index))
                        .map(|r| format!("instance {} node {}", r.instance, r.node))
                        .unwrap_or_else(|| "unknown actor".into());
                    eprintln!(
                        "camera sphere sweep hit {:.2}m: layer {} ({}){}; {}",
                        hit.distance,
                        layer,
                        self.settings.layers.get(layer).map_or("?", String::as_str),
                        if layer_bit & obstacle_mask != 0 {
                            " [blocking]"
                        } else {
                            " [ignored]"
                        },
                        owner
                    );
                }
            }
            hits.iter()
                .filter(|hit| !hit.shape.is_null())
                .filter_map(|hit| {
                    let layer_bit = PxShape_getQueryFilterData(hit.shape).word0;
                    // A hit at the sweep origin is a volume the target already stands in (kill
                    // zones, 'Ignore All' triggers), not something between target and camera.
                    (layer_bit & obstacle_mask != 0 && hit.distance > 0.5).then_some(hit.distance)
                })
                .min_by(f32::total_cmp)
                .map_or(distance, |hit| (hit - 0.6).max(minimum_distance))
        }
    }

    /// Distance to the first STATIC surface hit by a ray (scene geometry only), or None.
    pub fn raycast_static(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<f32> {
        let d = dir.try_normalize()?;
        unsafe {
            let filter = PxSceneQueryFilterData {
                data: PxFilterData_new_1(),
                flags: PxQueryFlags { mBits: PxQueryFlag::eSTATIC as u16 },
                structgen_pad0: [0; 2],
            };
            let mut hit = PxRaycastHit_new();
            let ok = PxSceneQueryExt_raycastSingle_mut(
                self.scene,
                &px_vec(origin),
                &px_vec(d),
                max,
                PxSceneQueryFlags { mBits: PxHitFlag::ePOSITION as u16 },
                &mut hit,
                &filter,
                null_mut(),
                null(),
            );
            ok.then_some(hit.distance)
        }
    }

    pub fn new(settings: Settings) -> Result<Self, String> {
        unsafe {
            let sdk = sdk().ok_or("PhysX failed to initialize")?;
            let (physics, cooking) = (sdk.physics, sdk.cooking);
            let scale = PxPhysics_getTolerancesScale(physics);
            let mut desc = PxSceneDesc_new(scale);
            let g = Vec3::from_array(settings.gravity);
            desc.gravity = px_vec(g);
            desc.bounceThresholdVelocity = settings.bounce_threshold;
            desc.frictionType = match settings.friction_type.as_str() {
                "one_directional" => PxFrictionType::eONE_DIRECTIONAL,
                "two_directional" => PxFrictionType::eTWO_DIRECTIONAL,
                _ => PxFrictionType::ePATCH,
            };
            desc.solverType = if settings.solver_type == "tgs" {
                PxSolverType::eTGS
            } else {
                PxSolverType::ePGS
            };
            desc.broadPhaseType = match settings.broadphase.as_str() {
                "sap" => PxBroadPhaseType::eSAP,
                "mbp" => PxBroadPhaseType::eMBP,
                _ => PxBroadPhaseType::eABP,
            };
            let mut flags = PxSceneFlag::eENABLE_ACTIVE_ACTORS | PxSceneFlag::eENABLE_CCD;
            if settings.contacts_generation == "pcm" {
                flags |= PxSceneFlag::eENABLE_PCM;
            }
            if settings.adaptive_force {
                flags |= PxSceneFlag::eADAPTIVE_FORCE;
            }
            if settings.enhanced_determinism {
                flags |= PxSceneFlag::eENABLE_ENHANCED_DETERMINISM;
            }
            desc.flags = PxSceneFlags { mBits: flags };
            let dispatcher = phys_PxDefaultCpuDispatcherCreate(2, null_mut());
            desc.cpuDispatcher = dispatcher as *mut PxCpuDispatcher;
            enable_custom_filter_shader(&mut desc, layer_filter, 0);
            let scene = PxPhysics_createScene_mut(physics, &desc);
            if scene.is_null() {
                return Err("PhysX scene creation failed".into());
            }
            let mut raw: Box<Vec<RawPair>> = Box::new(Vec::new());
            let mut info = SimulationEventCallbackInfo::default();
            info.collision_callback = Some(on_contact);
            info.collision_user_data = &mut *raw as *mut Vec<RawPair> as *mut std::ffi::c_void;
            let events = create_simulation_event_callbacks(&info);
            PxScene_setSimulationEventCallback_mut(scene, events);
            Ok(Self {
                settings,
                physics,
                cooking,
                dispatcher,
                scene,
                materials: Vec::new(),
                bodies: Vec::new(),
                instances: Vec::new(),
                steps: 0,
                actors: Vec::new(),
                colliders: Vec::new(),
                contacts: Vec::new(),
                touching: HashMap::new(),
                raw,
                events,
                runtime_joints: Vec::new(),
                pre_velocity: Vec::new(),
                pending_dv: Vec::new(),
                all_joints: Vec::new(),
                joint_links: Vec::new(),
                invulnerable_joints: Vec::new(),
                drive_limits_are_forces: true,
            })
        }
    }

    /// One FixedUpdate worth of simulation.
    pub fn step(&mut self) {
        // Collision.relativeVelocity uses pre-solver velocities: current + queued forces + gravity.
        let g = Vec3::from_array(self.settings.gravity) * self.settings.fixed_timestep;
        self.pending_dv.resize(self.bodies.len(), Vec3::ZERO);
        self.pre_velocity = (0..self.bodies.len())
            .map(|b| {
                self.linear_velocity(b)
                    + self.pending_dv[b]
                    + if self.bodies[b].kinematic || self.bodies[b].removed {
                        Vec3::ZERO
                    } else {
                        g
                    }
            })
            .collect();
        self.pending_dv.iter_mut().for_each(|v| *v = Vec3::ZERO);
        unsafe {
            PxScene_simulate_mut(
                self.scene,
                self.settings.fixed_timestep,
                null_mut(),
                null_mut(),
                0,
                true,
            );
            let mut error = 0u32;
            PxScene_fetchResults_mut(self.scene, true, &mut error);
        }
        self.steps += 1;
        self.collect_contacts();
    }

    /// Fold shape-pair events into Unity's per-rigidbody-pair Enter/Stay/Exit.
    fn collect_contacts(&mut self) {
        self.contacts.clear();
        let mut merged: HashMap<(usize, usize), RawPair> = HashMap::new();
        for mut r in self.raw.drain(..) {
            let key = (r.actors[0].min(r.actors[1]), r.actors[0].max(r.actors[1]));
            if r.actors[0] != key.0 {
                // PhysX normals point toward shape0; re-express them toward key.0.
                for p in &mut r.points {
                    p.normal = -p.normal;
                }
            }
            let e = merged.entry(key).or_insert(RawPair {
                actors: [key.0, key.1],
                found: 0,
                persists: 0,
                lost: 0,
                points: vec![],
            });
            e.found += r.found;
            e.persists += r.persists;
            e.lost += r.lost;
            e.points.extend(r.points);
        }
        let mut keys: Vec<_> = merged.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let r = merged.remove(&key).unwrap();
            let before = self.touching.get(&key).copied().unwrap_or(0);
            let after = (before + r.found).saturating_sub(r.lost);
            let kind = if before == 0 && after > 0 {
                ContactKind::Enter
            } else if after > 0 {
                ContactKind::Stay
            } else if before > 0 {
                ContactKind::Exit
            } else {
                continue;
            };
            if after > 0 {
                self.touching.insert(key, after);
            } else {
                self.touching.remove(&key);
            }
            let (a, b) = (key.0 - 1, key.1 - 1);
            let vel = |i: usize| {
                self.actors[i]
                    .body
                    .and_then(|b| self.pre_velocity.get(b).copied())
                    .unwrap_or(Vec3::ZERO)
            };
            let rel = vel(b) - vel(a);
            let flipped = r
                .points
                .iter()
                .map(|p| ContactPoint {
                    normal: -p.normal,
                    ..*p
                })
                .collect();
            self.contacts.push(Contact {
                kind,
                this: a,
                other: b,
                points: r.points,
                relative_velocity: rel,
            });
            self.contacts.push(Contact {
                kind,
                this: b,
                other: a,
                points: flipped,
                relative_velocity: -rel,
            });
        }
    }

    fn register_actor(&mut self, actor: *mut PxActor, r: ActorRef) {
        self.actors.push(r);
        unsafe { (*actor).userData = self.actors.len() as *mut std::ffi::c_void };
    }

    /// Unity-space pose of a body (Rigidbody.position / rotation).
    pub fn pose(&self, body: usize) -> Iso {
        unsafe {
            from_px(PxRigidActor_getGlobalPose(
                self.bodies[body].actor as *const PxRigidActor,
            ))
        }
    }

    pub fn linear_velocity(&self, body: usize) -> Vec3 {
        if self.bodies[body].removed {
            return Vec3::ZERO;
        }
        let v =
            unsafe { PxRigidBody_getLinearVelocity(self.bodies[body].actor as *const PxRigidBody) };
        Vec3::new(v.x, v.y, v.z)
    }

    pub fn is_sleeping(&self, body: usize) -> bool {
        unsafe { PxRigidDynamic_isSleeping(self.bodies[body].actor) }
    }

    pub fn mass(&self, body: usize) -> f32 {
        unsafe { PxRigidBody_getMass(self.bodies[body].actor as *const PxRigidBody) }
    }

    pub fn inertia(&self, body: usize) -> Vec3 {
        let v = unsafe {
            PxRigidBody_getMassSpaceInertiaTensor(self.bodies[body].actor as *const PxRigidBody)
        };
        Vec3::new(v.x, v.y, v.z)
    }

    fn rb(&self, body: usize) -> *mut PxRigidBody {
        self.bodies[body].actor as *mut PxRigidBody
    }

    pub fn angular_velocity(&self, body: usize) -> Vec3 {
        let v = unsafe { PxRigidBody_getAngularVelocity(self.rb(body)) };
        Vec3::new(v.x, v.y, v.z)
    }

    /// `Rigidbody.collisionDetectionMode`: 1 = Continuous, 2 = ContinuousDynamic (PhysX
    /// `eENABLE_CCD`), 0 = Discrete. The source enables it on the punching hand/forearm and
    /// restores Discrete when the punch resets (`ArmActionPunching` / `ArmActionPunchResetting`);
    /// without it a fast hand tunnels through the target and PhysX's depenetration ejects the
    /// victim, which reads as "punches are far too strong".
    pub fn set_collision_detection(&mut self, body: usize, mode: u32) {
        if self.bodies[body].kinematic || self.bodies[body].removed {
            return;
        }
        let on = matches!(mode, 1 | 2);
        unsafe {
            PxRigidBody_setRigidBodyFlag_mut(
                self.rb(body),
                PxRigidBodyFlag::eENABLE_CCD,
                on,
            )
        };
    }

    pub fn set_linear_velocity(&mut self, body: usize, v: Vec3) {
        if self.bodies[body].kinematic || self.bodies[body].removed {
            return;
        }
        unsafe { PxRigidBody_setLinearVelocity_mut(self.rb(body), &px_vec(v), true) }
    }

    pub fn set_angular_velocity(&mut self, body: usize, v: Vec3) {
        if self.bodies[body].kinematic || self.bodies[body].removed {
            return;
        }
        unsafe { PxRigidBody_setAngularVelocity_mut(self.rb(body), &px_vec(v), true) }
    }

    /// Rigidbody.worldCenterOfMass
    pub fn center_of_mass(&self, body: usize) -> Vec3 {
        let com = unsafe { from_px(PxRigidBody_getCMassLocalPose(self.rb(body))) };
        let pose = self.pose(body);
        pose.position + pose.rotation * com.position
    }

    /// Rigidbody.AddForce / AddTorque with a Unity ForceMode (0 Force, 1 Impulse, 2 VelocityChange, 5 Acceleration).
    /// Forces on kinematic bodies are ignored, as in Unity (PhysX would report an error).
    pub fn add_force(&mut self, body: usize, force: Vec3, unity_mode: u32) {
        if !force.is_finite() || self.bodies[body].kinematic || self.bodies[body].removed {
            return;
        }
        let m = self.mass(body).max(1e-6);
        let dv = match unity_mode {
            1 => force / m,
            2 => force,
            5 => force * self.settings.fixed_timestep,
            _ => force / m * self.settings.fixed_timestep,
        };
        if self.pending_dv.len() < self.bodies.len() {
            self.pending_dv.resize(self.bodies.len(), Vec3::ZERO);
        }
        self.pending_dv[body] += dv;
        unsafe {
            PxRigidBody_addForce_mut(self.rb(body), &px_vec(force), force_mode(unity_mode), true)
        }
    }

    pub fn add_torque(&mut self, body: usize, torque: Vec3, unity_mode: u32) {
        if !torque.is_finite() || self.bodies[body].kinematic || self.bodies[body].removed {
            return;
        }
        unsafe {
            PxRigidBody_addTorque_mut(self.rb(body), &px_vec(torque), force_mode(unity_mode), true)
        }
    }

    /// Rigidbody.AddRelativeTorque: torque given in the body's local frame.
    pub fn add_relative_torque(&mut self, body: usize, torque: Vec3, unity_mode: u32) {
        let world = self.pose(body).rotation * torque;
        self.add_torque(body, world, unity_mode);
    }

    /// Rigidbody.AddForceAtPosition
    pub fn add_force_at(&mut self, body: usize, force: Vec3, position: Vec3, unity_mode: u32) {
        if !force.is_finite()
            || !position.is_finite()
            || self.bodies[body].kinematic
            || self.bodies[body].removed
        {
            return;
        }
        unsafe {
            PxRigidBodyExt_addForceAtPos_mut(
                self.rb(body),
                &px_vec(force),
                &px_vec(position),
                force_mode(unity_mode),
                true,
            )
        }
    }

    pub fn set_drag(&mut self, body: usize, drag: f32) {
        unsafe { PxRigidBody_setLinearDamping_mut(self.rb(body), drag) }
    }

    pub fn set_angular_drag(&mut self, body: usize, drag: f32) {
        unsafe { PxRigidBody_setAngularDamping_mut(self.rb(body), drag) }
    }

    pub fn set_max_angular_velocity(&mut self, body: usize, v: f32) {
        unsafe { PxRigidBody_setMaxAngularVelocity_mut(self.rb(body), v) }
    }

    pub fn set_solver_iterations(&mut self, body: usize, position: u32, velocity: u32) {
        unsafe {
            PxRigidDynamic_setSolverIterationCounts_mut(self.bodies[body].actor, position, velocity)
        }
    }

    pub fn set_use_gravity(&mut self, body: usize, on: bool) {
        unsafe {
            PxActor_setActorFlag_mut(
                self.bodies[body].actor as *mut PxActor,
                PxActorFlag::eDISABLE_GRAVITY,
                !on,
            )
        }
    }

    /// Rigidbody.inertiaTensor (mass-space diagonal)
    pub fn set_inertia(&mut self, body: usize, inertia: Vec3) {
        unsafe { PxRigidBody_setMassSpaceInertiaTensor_mut(self.rb(body), &px_vec(inertia)) }
    }

    /// Move a body and zero its velocity (respawn / teleport).
    pub fn teleport(&mut self, body: usize, pose: Iso) {
        unsafe {
            PxRigidActor_setGlobalPose_mut(
                self.bodies[body].actor as *mut PxRigidActor,
                &px_iso(pose),
                true,
            );
        }
        self.set_linear_velocity(body, Vec3::ZERO);
        self.set_angular_velocity(body, Vec3::ZERO);
    }

    /// Rigidbody.isKinematic = on (PxRigidBodyFlag::eKINEMATIC): the body stops reacting to forces but still
    /// pushes dynamic bodies and anchors joints.
    pub fn set_kinematic(&mut self, body: usize, on: bool) {
        unsafe {
            PxRigidBody_setRigidBodyFlag_mut(self.rb(body), PxRigidBodyFlag::eKINEMATIC, on);
        }
    }

    /// Rigidbody.MovePosition / MoveRotation on a kinematic body: PhysX sweeps it to `pose` over
    /// the next step, pushing whatever is in the way (the wheel axle, moving platforms).
    pub fn move_kinematic(&mut self, body: usize, pose: Iso) {
        unsafe {
            PxRigidDynamic_setKinematicTarget_mut(
                self.bodies[body].actor as *mut PxRigidDynamic,
                &px_iso(pose),
            );
        }
    }

    /// Take a body out of the simulation (a shattered glass pane, ...). It keeps its last pose;
    /// forces and velocity sets on it are ignored from now on, as for kinematic bodies.
    pub fn remove_body(&mut self, body: usize) {
        if self.bodies[body].removed {
            return;
        }
        unsafe {
            PxScene_removeActor_mut(self.scene, self.bodies[body].actor as *mut PxActor, true);
        }
        self.bodies[body].removed = true;
    }

    /// Put a removed body back into the simulation (stage reset between rounds).
    pub fn restore_body(&mut self, body: usize) {
        if !self.bodies[body].removed {
            return;
        }
        unsafe {
            PxScene_addActor_mut(self.scene, self.bodies[body].actor as *mut PxActor, null());
        }
        self.bodies[body].removed = false;
    }

    /// A free dynamic box (fracture shards). Unity-space pose; `layer` picks the collision row.
    pub fn add_box(
        &mut self,
        pose: Iso,
        half_extents: Vec3,
        mass: f32,
        layer: u32,
        velocity: Vec3,
    ) -> usize {
        let s = &self.settings;
        let (sleep, depen, max_ang, pos_iters, vel_iters, contact_offset) = (
            s.sleep_threshold,
            s.max_depenetration_velocity,
            s.max_angular_speed,
            s.solver_iterations,
            s.solver_velocity_iterations,
            s.contact_offset,
        );
        let mask = self.settings.collision_mask(layer);
        let material = self.material(&None);
        let body = self.bodies.len();
        unsafe {
            let a = PxPhysics_createRigidDynamic_mut(self.physics, &px_iso(pose));
            let rb = a as *mut PxRigidBody;
            PxRigidBody_setMaxAngularVelocity_mut(rb, max_ang);
            PxRigidBody_setMaxDepenetrationVelocity_mut(rb, depen);
            PxRigidDynamic_setSleepThreshold_mut(a, sleep);
            PxRigidDynamic_setSolverIterationCounts_mut(a, pos_iters, vel_iters);
            let geo = PxBoxGeometry_new_1(
                half_extents.x.max(0.005),
                half_extents.y.max(0.005),
                half_extents.z.max(0.005),
            );
            let shape = PxPhysics_createShape_mut(
                self.physics,
                &geo as *const _ as *const PxGeometry,
                material,
                true,
                PxShapeFlags {
                    mBits: (PxShapeFlag::eSIMULATION_SHAPE | PxShapeFlag::eSCENE_QUERY_SHAPE) as u8,
                },
            );
            PxShape_setContactOffset_mut(shape, contact_offset);
            PxShape_setRestOffset_mut(shape, 0.0);
            let filter = PxFilterData {
                word0: 1 << (layer & 31),
                word1: mask,
                word2: 0,
                word3: 0,
            };
            PxShape_setSimulationFilterData_mut(shape, &filter);
            PxShape_setQueryFilterData_mut(shape, &filter);
            PxRigidActor_attachShape_mut(a as *mut PxRigidActor, shape);
            PxShape_release_mut(shape);
            PxRigidBodyExt_setMassAndUpdateInertia_mut_1(rb, mass.max(1e-3), null(), false);
            PxRigidBody_setLinearVelocity_mut(rb, &px_vec(velocity), true);
            self.bodies.push(Body {
                instance: usize::MAX,
                node: 0,
                name: "shard".into(),
                kinematic: false,
                removed: false,
                actor: a,
            });
            self.register_actor(
                a as *mut PxActor,
                ActorRef {
                    instance: usize::MAX,
                    node: 0,
                    body: Some(body),
                },
            );
            PxScene_addActor_mut(self.scene, a as *mut PxActor, null());
        }
        body
    }

    fn material(&mut self, m: &Option<Material>) -> *mut PxMaterial {
        if let Some((_, px)) = self.materials.iter().find(|(k, _)| k == m) {
            return *px;
        }
        // Unity's built-in default material: 0.6 / 0.6 / 0, average / average.
        let (dynamic, stat, bounce, fc, bc) = match m {
            Some(m) => (
                m.dynamic_friction,
                m.static_friction,
                m.bounciness,
                unity::combine_mode(&m.friction_combine),
                unity::combine_mode(&m.bounce_combine),
            ),
            None => (0.6, 0.6, 0.0, 0, 0),
        };
        let px = unsafe {
            let px = PxPhysics_createMaterial_mut(self.physics, stat, dynamic, bounce);
            PxMaterial_setFrictionCombineMode_mut(px, fc);
            PxMaterial_setRestitutionCombineMode_mut(px, bc);
            px
        };
        self.materials.push((m.clone(), px));
        px
    }

    unsafe fn cook(
        &self,
        src: &Sidecar,
        index: usize,
        convex: bool,
    ) -> Result<*mut PxBase, String> {
        let mesh = &src.collision_meshes[index];
        if mesh.vertices.len() % 3 != 0 || mesh.triangles.len() % 3 != 0 || mesh.vertices.is_empty()
        {
            return Err(format!("collision mesh {} is malformed", mesh.name));
        }
        if mesh
            .triangles
            .iter()
            .any(|&t| t as usize >= mesh.vertices.len() / 3)
        {
            return Err(format!(
                "collision mesh {} indexes past its vertices",
                mesh.name
            ));
        }
        let insertion = PxPhysics_getPhysicsInsertionCallback_mut(self.physics);
        let points = PxBoundedData {
            stride: 12,
            structgen_pad0: [0; 4],
            data: mesh.vertices.as_ptr() as *const _,
            count: (mesh.vertices.len() / 3) as u32,
            structgen_pad1: [0; 4],
        };
        let out = if convex {
            let mut desc = PxConvexMeshDesc_new();
            desc.points = points;
            desc.flags = PxConvexFlags {
                mBits: PxConvexFlag::eCOMPUTE_CONVEX as u16,
            };
            let mut result = 0;
            PxCooking_createConvexMesh(self.cooking, &desc, insertion, &mut result) as *mut PxBase
        } else {
            let mut desc = PxTriangleMeshDesc_new();
            desc.points = points;
            desc.triangles = PxBoundedData {
                stride: 12,
                structgen_pad0: [0; 4],
                data: mesh.triangles.as_ptr() as *const _,
                count: (mesh.triangles.len() / 3) as u32,
                structgen_pad1: [0; 4],
            };
            let mut result = 0;
            PxCooking_createTriangleMesh(self.cooking, &desc, insertion, &mut result) as *mut PxBase
        };
        if out.is_null() {
            return Err(format!("cooking {} failed", mesh.name));
        }
        Ok(out)
    }

    /// Instantiate a prefab/scene sidecar with its root nodes placed at `origin` (Unity space).
    /// Returns the instance index.
    pub fn spawn(&mut self, name: &str, src: &Sidecar, origin: Pose) -> Result<usize, String> {
        let instance = self.instances.len();
        let world = src.world_poses(origin);
        let mut inst = Instance {
            name: name.into(),
            bodies: HashMap::new(),
            statics: 0,
            joints: 0,
            warnings: vec![],
        };
        let s = &self.settings;
        let (sleep, depen, max_ang) = (
            s.sleep_threshold,
            s.max_depenetration_velocity,
            s.max_angular_speed,
        );
        let (pos_iters, vel_iters, contact_offset) = (
            s.solver_iterations,
            s.solver_velocity_iterations,
            s.contact_offset,
        );

        // Rigidbodies first so colliders can find their owner.
        let mut owner: Vec<Option<usize>> = vec![None; src.nodes.len()];
        for (i, node) in src.nodes.iter().enumerate() {
            owner[i] = node.parent.and_then(|p| owner[p]);
            if !node.active_in_hierarchy {
                continue;
            }
            let Some(rb) = node.component("Rigidbody") else {
                continue;
            };
            let d = &rb.data;
            let kinematic = flag(&d["m_IsKinematic"]);
            let actor = unsafe {
                let a = PxPhysics_createRigidDynamic_mut(self.physics, &px_iso(Iso::of(&world[i])));
                let body = a as *mut PxRigidBody;
                PxRigidBody_setLinearDamping_mut(body, num(&d["m_Drag"]).max(0.0));
                PxRigidBody_setAngularDamping_mut(body, num(&d["m_AngularDrag"]).max(0.0));
                PxRigidBody_setMaxAngularVelocity_mut(body, max_ang);
                PxRigidBody_setMaxDepenetrationVelocity_mut(body, depen);
                PxRigidDynamic_setSleepThreshold_mut(a, sleep);
                PxRigidDynamic_setSolverIterationCounts_mut(a, pos_iters, vel_iters);
                // Unity RigidbodyConstraints are PxRigidDynamicLockFlags shifted up by one bit.
                let locks = (d["m_Constraints"].as_u64().unwrap_or(0) >> 1) as u16 & 0x3f;
                PxRigidDynamic_setRigidDynamicLockFlags_mut(
                    a,
                    PxRigidDynamicLockFlags { mBits: locks },
                );
                if kinematic {
                    PxRigidBody_setRigidBodyFlag_mut(body, PxRigidBodyFlag::eKINEMATIC, true);
                } else {
                    // Rigidbody.collisionDetectionMode: Continuous (1) / ContinuousDynamic (2) -> CCD,
                    // ContinuousSpeculative (3) -> speculative CCD; Discrete (0) -> none.
                    match d["m_CollisionDetection"].as_u64().unwrap_or(0) {
                        1 | 2 => PxRigidBody_setRigidBodyFlag_mut(body, PxRigidBodyFlag::eENABLE_CCD, true),
                        3 => PxRigidBody_setRigidBodyFlag_mut(body, PxRigidBodyFlag::eENABLE_SPECULATIVE_CCD, true),
                        _ => {}
                    }
                }
                if !flag(&d["m_UseGravity"]) {
                    PxActor_setActorFlag_mut(
                        a as *mut PxActor,
                        PxActorFlag::eDISABLE_GRAVITY,
                        true,
                    );
                }
                a
            };
            let body = self.bodies.len();
            self.register_actor(
                actor as *mut PxActor,
                ActorRef {
                    instance,
                    node: i,
                    body: Some(body),
                },
            );
            owner[i] = Some(body);
            inst.bodies.insert(i, body);
            self.bodies.push(Body {
                instance,
                node: i,
                name: node.name().into(),
                kinematic,
                removed: false,
                actor,
            });
        }

        // Colliders attach to the nearest Rigidbody at or above them, else to a static per node.
        let mut statics: HashMap<usize, *mut PxRigidStatic> = HashMap::new();
        let mut body_has_shapes = vec![false; self.bodies.len()];
        for (i, node) in src.nodes.iter().enumerate() {
            if !node.active_in_hierarchy {
                continue;
            }
            for c in node
                .components
                .iter()
                .filter(|c| c.kind.ends_with("Collider"))
            {
                let Some(desc) =
                    unity::collider(&c.kind, &c.data, c.collision_mesh, world[i].scale)
                else {
                    continue;
                };
                let (actor, actor_pose, dynamic) = match owner[i] {
                    Some(b) => (
                        self.bodies[b].actor as *mut PxRigidActor,
                        Iso::of(&world[self.bodies[b].node]),
                        !self.bodies[b].kinematic,
                    ),
                    None => {
                        let a = *statics.entry(i).or_insert_with(|| unsafe {
                            PxPhysics_createRigidStatic_mut(
                                self.physics,
                                &px_iso(Iso::of(&world[i])),
                            )
                        });
                        (a as *mut PxRigidActor, Iso::of(&world[i]), false)
                    }
                };
                let local = actor_pose
                    .inverse()
                    .mul(&Iso::of(&world[i]))
                    .mul(&desc.local);
                let material = self.material(&c.material);
                unsafe {
                    let sphere;
                    let capsule;
                    let cube;
                    let convex_geo;
                    let tri_geo;
                    let geometry: *const PxGeometry = match &desc.shape {
                        Shape::Sphere { radius } => {
                            sphere = PxSphereGeometry_new_1(*radius);
                            &sphere as *const _ as *const PxGeometry
                        }
                        Shape::Capsule {
                            radius,
                            half_height,
                        } => {
                            capsule = PxCapsuleGeometry_new_1(*radius, *half_height);
                            &capsule as *const _ as *const PxGeometry
                        }
                        Shape::Box { half_extents } => {
                            cube =
                                PxBoxGeometry_new_1(half_extents.x, half_extents.y, half_extents.z);
                            &cube as *const _ as *const PxGeometry
                        }
                        Shape::Mesh {
                            index,
                            convex,
                            scale,
                        } => {
                            if !convex && dynamic {
                                inst.warnings.push(format!(
                                    "{}: concave MeshCollider on a dynamic body skipped",
                                    node.path
                                ));
                                continue;
                            }
                            let mesh = match self.cook(src, *index, *convex) {
                                Ok(m) => m,
                                Err(e) => {
                                    inst.warnings.push(format!("{}: {e}", node.path));
                                    continue;
                                }
                            };
                            let mesh_scale = PxMeshScale_new_2(&px_vec(*scale));
                            if *convex {
                                convex_geo = PxConvexMeshGeometry_new_1(
                                    mesh as *mut PxConvexMesh,
                                    &mesh_scale,
                                    PxConvexMeshGeometryFlags { mBits: 0 },
                                );
                                &convex_geo as *const _ as *const PxGeometry
                            } else {
                                tri_geo = PxTriangleMeshGeometry_new_1(
                                    mesh as *mut PxTriangleMesh,
                                    &mesh_scale,
                                    PxMeshGeometryFlags { mBits: 0 },
                                );
                                &tri_geo as *const _ as *const PxGeometry
                            }
                        }
                    };
                    let flags = if desc.trigger {
                        PxShapeFlag::eTRIGGER_SHAPE | PxShapeFlag::eVISUALIZATION
                    } else {
                        PxShapeFlag::eSIMULATION_SHAPE
                            | PxShapeFlag::eSCENE_QUERY_SHAPE
                            | PxShapeFlag::eVISUALIZATION
                    };
                    let shape = PxPhysics_createShape_mut(
                        self.physics,
                        geometry,
                        material,
                        true,
                        PxShapeFlags { mBits: flags as u8 },
                    );
                    if shape.is_null() {
                        inst.warnings
                            .push(format!("{}: PhysX rejected {} geometry", node.path, c.kind));
                        continue;
                    }
                    PxShape_setLocalPose_mut(shape, &px_iso(local));
                    PxShape_setContactOffset_mut(shape, contact_offset);
                    PxShape_setRestOffset_mut(shape, 0.0);
                    let filter = PxFilterData {
                        word0: 1 << (node.layer & 31),
                        word1: self.settings.collision_mask(node.layer),
                        word2: if desc.trigger { TRIGGER } else { 0 },
                        // Actor body parts carry (instance << 5) | part id so the filter shader can
                        // apply BodyBase's IgnoreCollision pairs within one actor only.
                        word3: (instance as u32) << 5 | body_part_id(node.name()),
                    };
                    PxShape_setSimulationFilterData_mut(shape, &filter);
                    PxShape_setQueryFilterData_mut(shape, &filter);
                    PxRigidActor_attachShape_mut(actor, shape);
                    PxShape_release_mut(shape);
                    self.colliders.push(ColliderRef {
                        instance,
                        node: i,
                        body: owner[i],
                        trigger: desc.trigger,
                        convex: !matches!(&desc.shape, Shape::Mesh { convex: false, .. }),
                        shape,
                        actor,
                    });
                }
                if let Some(b) = owner[i] {
                    body_has_shapes[b] |= !desc.trigger;
                }
            }
        }

        // Mass last: Unity derives the inertia tensor and centre of mass from the simulation shapes.
        for (&node, &b) in &inst.bodies {
            let mass = num(&src.nodes[node].component("Rigidbody").unwrap().data["m_Mass"]);
            let mass = if mass.is_finite() && mass > 0.0 {
                mass
            } else {
                1.0
            };
            unsafe {
                let body = self.bodies[b].actor as *mut PxRigidBody;
                if body_has_shapes[b] {
                    PxRigidBodyExt_setMassAndUpdateInertia_mut_1(body, mass, null(), false);
                } else {
                    PxRigidBody_setMass_mut(body, mass);
                    PxRigidBody_setMassSpaceInertiaTensor_mut(body, &px_vec(Vec3::ONE));
                }
            }
        }

        // Joints: actor0 = own body, actor1 = connected body (or world).
        for (i, node) in src.nodes.iter().enumerate() {
            if !node.active_in_hierarchy {
                continue;
            }
            for c in node.components.iter().filter(|c| c.kind.ends_with("Joint")) {
                let Some(&own) = inst.bodies.get(&i) else {
                    inst.warnings.push(format!(
                        "{}: {} without an active Rigidbody",
                        node.path, c.kind
                    ));
                    continue;
                };
                let connected = match c.connected_node {
                    Some(n) => match inst.bodies.get(&n) {
                        Some(&b) => Some(b),
                        None => {
                            // Unity skips joints whose connected body is disabled.
                            inst.warnings.push(format!(
                                "{}: connected body {} inactive",
                                node.path, src.nodes[n].path
                            ));
                            continue;
                        }
                    },
                    None => None,
                };
                let own_pose = (Iso::of(&world[i]), world[i].scale);
                let conn_pose = connected.map(|b| {
                    let n = self.bodies[b].node;
                    (Iso::of(&world[n]), world[n].scale)
                });
                let desc = if c.kind == "ConfigurableJoint" {
                    Some(unity::configurable_joint(
                        &c.data, own_pose.0, own_pose.1, conn_pose,
                    ))
                } else {
                    unity::other_joint(&c.kind, &c.data, own_pose.0, own_pose.1, conn_pose)
                };
                let Some(desc) = desc else {
                    inst.warnings
                        .push(format!("{}: {} not supported", node.path, c.kind));
                    continue;
                };
                let actor1 =
                    connected.map_or(null_mut(), |b| self.bodies[b].actor as *mut PxRigidActor);
                let j = unsafe {
                    self.create_joint(self.bodies[own].actor as *mut PxRigidActor, actor1, &desc)
                };
                if node
                    .components
                    .iter()
                    .any(|c| c.script.as_deref() == Some("JointBreakSyncTrigger"))
                {
                    unsafe { PxJoint_setBreakForce_mut(j as *mut PxJoint, f32::MAX, f32::MAX) };
                    self.invulnerable_joints
                        .push((j, desc.break_force, desc.break_torque));
                }
                self.all_joints.push(j);
                self.joint_links.push((Some(own), connected));
                inst.joints += 1;
            }
        }

        unsafe {
            for b in inst.bodies.values() {
                PxScene_addActor_mut(self.scene, self.bodies[*b].actor as *mut PxActor, null());
            }
            let mut sorted: Vec<_> = statics.iter().map(|(n, a)| (*n, *a)).collect();
            sorted.sort_by_key(|(n, _)| *n);
            for (node, a) in sorted {
                self.register_actor(
                    a as *mut PxActor,
                    ActorRef {
                        instance,
                        node,
                        body: None,
                    },
                );
                PxScene_addActor_mut(self.scene, a as *mut PxActor, null());
            }
        }
        inst.statics = statics.len();
        self.instances.push(inst);
        Ok(instance)
    }

    unsafe fn create_joint(
        &self,
        actor0: *mut PxRigidActor,
        actor1: *mut PxRigidActor,
        d: &JointDesc,
    ) -> *mut PxD6Joint {
        let j = phys_PxD6JointCreate(
            self.physics,
            actor0,
            &px_iso(d.frame0),
            actor1,
            &px_iso(d.frame1),
        );
        let motion = |m: Motion| match m {
            Motion::Locked => PxD6Motion::eLOCKED,
            Motion::Limited => PxD6Motion::eLIMITED,
            Motion::Free => PxD6Motion::eFREE,
        };
        let axes = [
            PxD6Axis::eX,
            PxD6Axis::eY,
            PxD6Axis::eZ,
            PxD6Axis::eTWIST,
            PxD6Axis::eSWING1,
            PxD6Axis::eSWING2,
        ];
        for (k, m) in d.linear.iter().chain(d.angular.iter()).enumerate() {
            PxD6Joint_setMotion_mut(j, axes[k], motion(*m));
        }
        let spring = |l: &Limit| PxSpring_new(l.spring.stiffness, l.spring.damping);
        let soft = |l: &Limit| l.spring.stiffness > 0.0;
        if d.angular[0] == Motion::Limited {
            let (lo, hi) = (d.twist.0, d.twist.1.max(d.twist.0 + 1e-4));
            let mut lim = if soft(&d.twist_limit) {
                PxJointAngularLimitPair_new_1(lo, hi, &spring(&d.twist_limit))
            } else {
                PxJointAngularLimitPair_new(lo, hi, d.twist_limit.contact_distance.unwrap_or(-1.0))
            };
            lim.restitution = d.twist_limit.restitution;
            PxD6Joint_setTwistLimit_mut(j, &lim);
        }
        if d.angular[1] == Motion::Limited || d.angular[2] == Motion::Limited {
            // PhysX cones need 0 < angle < pi; a zero Unity limit on a limited axis is effectively locked.
            let clamp = |a: f32| a.abs().clamp(1e-3, std::f32::consts::PI - 1e-3);
            let (y, z) = (clamp(d.swing.0), clamp(d.swing.1));
            let mut lim = if soft(&d.swing_limit) {
                PxJointLimitCone_new_1(y, z, &spring(&d.swing_limit))
            } else {
                PxJointLimitCone_new(y, z, d.swing_limit.contact_distance.unwrap_or(-1.0))
            };
            lim.restitution = d.swing_limit.restitution;
            PxD6Joint_setSwingLimit_mut(j, &lim);
        }
        if d.linear.contains(&Motion::Limited) {
            let extent = d.distance.max(1e-4);
            let mut lim = if soft(&d.distance_limit) {
                PxJointLinearLimit_new_1(extent, &spring(&d.distance_limit))
            } else {
                PxJointLinearLimit_new(
                    PxPhysics_getTolerancesScale(self.physics),
                    extent,
                    d.distance_limit.contact_distance.unwrap_or(-1.0),
                )
            };
            lim.restitution = d.distance_limit.restitution;
            PxD6Joint_setDistanceLimit_mut(j, &lim);
        }
        for (axis, drive) in &d.drives {
            let index = match axis {
                DriveAxis::X => PxD6Drive::eX,
                DriveAxis::Y => PxD6Drive::eY,
                DriveAxis::Z => PxD6Drive::eZ,
                DriveAxis::Swing => PxD6Drive::eSWING,
                DriveAxis::Twist => PxD6Drive::eTWIST,
                DriveAxis::Slerp => PxD6Drive::eSLERP,
            };
            let px = PxD6JointDrive_new_1(
                drive.stiffness.max(0.0),
                drive.damping.max(0.0),
                drive.force_limit.clamp(0.0, f32::MAX),
                false,
            );
            PxD6Joint_setDrive_mut(j, index, &px);
        }
        PxD6Joint_setDrivePosition_mut(j, &px_iso(d.target), false);
        PxD6Joint_setDriveVelocity_mut(
            j,
            &px_vec(d.target_velocity),
            &px_vec(d.target_angular_velocity),
            false,
        );
        let joint = j as *mut PxJoint;
        let mut flags = if self.drive_limits_are_forces {
            PxConstraintFlag::eDRIVE_LIMITS_ARE_FORCES
        } else {
            0
        };
        if d.collision {
            flags |= PxConstraintFlag::eCOLLISION_ENABLED;
        }
        if !d.preprocessing {
            flags |= PxConstraintFlag::eDISABLE_PREPROCESSING;
        }
        if let Some((distance, angle)) = d.projection {
            flags |= PxConstraintFlag::ePROJECTION;
            PxD6Joint_setProjectionLinearTolerance_mut(j, distance);
            PxD6Joint_setProjectionAngularTolerance_mut(j, angle);
        }
        PxJoint_setConstraintFlags_mut(
            joint,
            PxConstraintFlags {
                mBits: flags as u16,
            },
        );
        PxJoint_setBreakForce_mut(
            joint,
            d.break_force.min(f32::MAX),
            d.break_torque.min(f32::MAX),
        );
        if d.mass_scale != 1.0 && d.mass_scale.is_finite() {
            PxJoint_setInvMassScale0_mut(joint, d.mass_scale);
            PxJoint_setInvInertiaScale0_mut(joint, d.mass_scale);
        }
        if d.connected_mass_scale != 1.0 && d.connected_mass_scale.is_finite() {
            PxJoint_setInvMassScale1_mut(joint, d.connected_mass_scale);
            PxJoint_setInvInertiaScale1_mut(joint, d.connected_mass_scale);
        }
        j
    }

    /// `AddComponent<ConfigurableJoint>()` on `body` configured by `data` (a ConfigurableJoint
    /// typetree; missing fields take Unity's defaults), connected to `connected` or the world.
    /// Returns a handle for `remove_joint` / `joint_broken`.
    /// Whether two bodies are directly joint-connected (either direction). Used by the ragdoll
    /// debug overlay: a penetrating pair that is not joint-connected is a snag candidate.
    pub fn joint_connected(&self, a: usize, b: usize) -> bool {
        self.joint_links.iter().any(|(x, y)| {
            (x.is_some_and(|x| x == a) && y.is_some_and(|y| y == b))
                || (x.is_some_and(|x| x == b) && y.is_some_and(|y| y == a))
        })
    }

    pub fn add_joint(
        &mut self,
        body: usize,
        connected: Option<usize>,
        data: &serde_json::Value,
    ) -> usize {
        let pose = |b: usize| unsafe {
            from_px(PxRigidActor_getGlobalPose(
                self.bodies[b].actor as *const PxRigidActor,
            ))
        };
        let own = pose(body);
        let desc = unity::configurable_joint(
            data,
            own,
            Vec3::ONE,
            connected.map(|c| (pose(c), Vec3::ONE)),
        );
        let actor1 = connected.map_or(null_mut(), |c| self.bodies[c].actor as *mut PxRigidActor);
        let j = unsafe {
            self.create_joint(self.bodies[body].actor as *mut PxRigidActor, actor1, &desc)
        };
        self.joint_links.push((Some(body), connected));
        self.all_joints.push(j);
        self.runtime_joints.push(Some(j));
        self.runtime_joints.len() - 1
    }

    /// Switch every joint between force and impulse drive limits (see `drive_limits_are_forces`).
    pub fn set_drive_limits_are_forces(&mut self, on: bool) {
        self.drive_limits_are_forces = on;
        for j in &self.all_joints {
            unsafe {
                PxJoint_setConstraintFlag_mut(
                    *j as *mut PxJoint,
                    PxConstraintFlag::eDRIVE_LIMITS_ARE_FORCES,
                    on,
                )
            };
        }
    }

    /// Object.Destroy(joint)
    /// Release every joint attached to `body` (a snapped elevator cable, a cut rope). Returns how many.
    pub fn release_joints_of(&mut self, body: usize) -> usize {
        let mut released = 0;
        let mut k = 0;
        while k < self.all_joints.len() {
            let (a, b) = self.joint_links.get(k).copied().unwrap_or((None, None));
            if a == Some(body) || b == Some(body) {
                let j = self.all_joints.remove(k);
                self.joint_links.remove(k);
                self.invulnerable_joints.retain(|(x, _, _)| *x != j);
                for slot in self.runtime_joints.iter_mut() {
                    if *slot == Some(j) {
                        *slot = None;
                    }
                }
                unsafe { PxJoint_release_mut(j as *mut PxJoint) };
                released += 1;
            } else {
                k += 1;
            }
        }
        released
    }

    pub fn remove_joint(&mut self, handle: usize) {
        if let Some(Some(j)) = self.runtime_joints.get(handle).copied() {
            self.all_joints.retain(|x| *x != j);
            unsafe { PxJoint_release_mut(j as *mut PxJoint) };
            self.runtime_joints[handle] = None;
        }
    }

    /// A broken joint is destroyed by Unity, so scripts then see it as missing.
    /// JointBreakSyncTrigger.SetJointBreakValues once GAME_STATE allows joints to break (round
    /// start): restore the cached break force/torque of every invulnerable scene joint.
    pub fn make_joints_breakable(&mut self) {
        for (j, force, torque) in self.invulnerable_joints.drain(..) {
            unsafe {
                PxJoint_setBreakForce_mut(
                    j as *mut PxJoint,
                    force.min(f32::MAX),
                    torque.min(f32::MAX),
                )
            };
        }
    }

    pub fn joint_broken(&self, handle: usize) -> bool {
        match self.runtime_joints.get(handle).copied().flatten() {
            Some(j) => unsafe {
                PxJoint_getConstraintFlags(j as *const PxJoint).mBits as u32
                    & PxConstraintFlag::eBROKEN
                    != 0
            },
            None => true,
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        unsafe {
            PxScene_release_mut(self.scene);
            destroy_simulation_event_callbacks(self.events);
            PxDefaultCpuDispatcher_release_mut(self.dispatcher);
        }
    }
}
