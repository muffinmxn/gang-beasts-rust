//! Femur.BodyHandeler: named body parts of a spawned beast and their runtime rigidbody setup.
use gb_phys::{Sidecar, World};

/// Femur.BodyHandeler part slots that carry a Rigidbody (BodyHandeler_HumanoidMediumEctomorphv2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Head,
    Chest,
    Waist,
    Stomach,
    Hips,
    Crotch,
    LeftArm,
    LeftForarm,
    LeftHand,
    LeftThigh,
    LeftLeg,
    LeftFoot,
    RightArm,
    RightForarm,
    RightHand,
    RightThigh,
    RightLeg,
    RightFoot,
    Ball,
    Spring,
    BallProxy,
}

impl Part {
    pub const ALL: [Part; 21] = [
        Part::Head,
        Part::Chest,
        Part::Waist,
        Part::Stomach,
        Part::Hips,
        Part::Crotch,
        Part::LeftArm,
        Part::LeftForarm,
        Part::LeftHand,
        Part::LeftThigh,
        Part::LeftLeg,
        Part::LeftFoot,
        Part::RightArm,
        Part::RightForarm,
        Part::RightHand,
        Part::RightThigh,
        Part::RightLeg,
        Part::RightFoot,
        Part::Ball,
        Part::Spring,
        Part::BallProxy,
    ];

    /// GameObject names BodyHandeler.SetupTransforms looks up.
    pub fn node_name(self) -> &'static str {
        match self {
            Part::Head => "actor_head_collider",
            Part::Chest => "actor_chest_collider",
            Part::Waist => "actor_waist_collider",
            Part::Stomach => "actor_stomach_collider",
            Part::Hips => "actor_hips_collider",
            Part::Crotch => "actor_crotch_collider",
            Part::LeftArm => "actor_leftArm_collider",
            Part::LeftForarm => "actor_leftForarm_collider",
            Part::LeftHand => "actor_leftHand_collider",
            Part::LeftThigh => "actor_leftThigh_collider",
            Part::LeftLeg => "actor_leftLeg_collider",
            Part::LeftFoot => "actor_leftFoot_helper",
            Part::RightArm => "actor_rightArm_collider",
            Part::RightForarm => "actor_rightForarm_collider",
            Part::RightHand => "actor_rightHand_collider",
            Part::RightThigh => "actor_rightThigh_collider",
            Part::RightLeg => "actor_rightLeg_collider",
            Part::RightFoot => "actor_rightFoot_helper",
            Part::Ball => "actor_ball_collider",
            Part::Spring => "actor_spring_helper",
            Part::BallProxy => "actor_ball_proxy",
        }
    }
}

/// BodyHandeler.SolverIterations / SolverVelocityIterations (static, set in .cctor).
pub const SOLVER_ITERATIONS: u32 = 8;
pub const SOLVER_VELOCITY_ITERATIONS: u32 = 10;

/// CollisionHandeler settings of one part (from the prefab).
#[derive(Clone, Copy, Debug, Default)]
pub struct PartSettings {
    pub ground_check: bool,
    pub damage_check: bool,
    /// CollisionHandeler.damageModifier: how much damage this part takes.
    pub damage_modifier: f32,
    pub damage_minimum_velocity: f32,
}

pub struct Beast {
    pub instance: usize,
    bodies: [usize; 21],
    pub settings: [PartSettings; 21],
}

impl Beast {
    pub fn new(world: &World, instance: usize, src: &Sidecar) -> Result<Self, String> {
        let inst = &world.instances[instance];
        let mut bodies = [0; 21];
        let mut settings = [PartSettings::default(); 21];
        for (k, part) in Part::ALL.iter().enumerate() {
            let node = src
                .nodes
                .iter()
                .position(|n| n.name() == part.node_name())
                .ok_or_else(|| format!("beast is missing {}", part.node_name()))?;
            bodies[k] = *inst
                .bodies
                .get(&node)
                .ok_or_else(|| format!("{} has no active Rigidbody", part.node_name()))?;
            if let Some(c) = src.nodes[node]
                .components
                .iter()
                .find(|c| c.script.as_deref() == Some("CollisionHandeler"))
            {
                let d = &c.data;
                let flag = |k: &str| d[k].as_i64().unwrap_or(0) != 0;
                let num = |k: &str, def: f64| d[k].as_f64().unwrap_or(def) as f32;
                settings[k] = PartSettings {
                    ground_check: flag("groundCheck"),
                    damage_check: flag("damageCheck"),
                    damage_modifier: num("damageModifier", 1.0),
                    damage_minimum_velocity: num("damageMinimumVelocity", 1.0),
                };
            }
        }
        Ok(Self {
            instance,
            bodies,
            settings,
        })
    }

    pub fn body(&self, part: Part) -> usize {
        self.bodies[Part::ALL.iter().position(|p| *p == part).unwrap()]
    }

    /// BodyHandeler_HumanoidMediumEctomorphv2.SetupRigidbodys: RigidbodySettings(part, useGravity: true)
    /// on every part except the ball proxy.
    pub fn setup_rigidbodies(&self, world: &mut World) {
        for part in Part::ALL.iter().filter(|p| **p != Part::BallProxy) {
            let b = self.body(*part);
            world.set_drag(b, 1.6);
            world.set_angular_drag(b, 1.6);
            world.set_max_angular_velocity(b, 20.0);
            world.set_solver_iterations(b, SOLVER_ITERATIONS, SOLVER_VELOCITY_ITERATIONS);
            world.set_use_gravity(b, true);
            let inertia = world.inertia(b);
            world.set_inertia(b, inertia * 10.0);
        }
    }
}

/// Loads the beast prefab sidecar with its Start-time component setup applied:
/// ServerEnableProjectionMode.RunProjectionEnable (0x6AD4B0) sets every ConfigurableJoint under
/// the beast to projectionMode PositionAndRotation (distance 0.1, angle 180 from the prefab).
pub fn load_source(root: &std::path::Path) -> Result<Sidecar, String> {
    let mut src = Sidecar::load(root, "beast")?;
    for n in &mut src.nodes {
        for c in &mut n.components {
            if c.kind == "ConfigurableJoint" {
                c.data["m_ProjectionMode"] = 1.into();
            }
        }
    }
    Ok(src)
}
