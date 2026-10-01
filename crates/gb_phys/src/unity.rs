//! Unity component semantics -> PhysX parameters, free of FFI so it can be unit tested.
//!
//! Mirrors how Unity 2021.3 configures PhysX 4.1: every joint is a D6 whose actor0 is the
//! joint's own body and actor1 the connected body (or the world). PhysX therefore measures the
//! connected body relative to the owner, which is why Unity's angular X limits flip sign.
use crate::source::{flag, num, num_or, quat, vec3, Pose};
use glam::{Mat3, Quat, Vec3};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Iso {
    pub position: Vec3,
    pub rotation: Quat,
}

impl Iso {
    pub const IDENTITY: Self = Self {
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
    };
    pub fn new(position: Vec3, rotation: Quat) -> Self {
        Self { position, rotation }
    }
    pub fn of(p: &Pose) -> Self {
        Self::new(p.position, p.rotation)
    }
    pub fn inverse(&self) -> Self {
        let r = self.rotation.inverse();
        Self::new(r * -self.position, r)
    }
    pub fn mul(&self, o: &Iso) -> Iso {
        Iso::new(
            self.position + self.rotation * o.position,
            (self.rotation * o.rotation).normalize(),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Sphere {
        radius: f32,
    },
    /// PhysX capsules run along local X; the collider pose rotates it onto Unity's direction.
    Capsule {
        radius: f32,
        half_height: f32,
    },
    Box {
        half_extents: Vec3,
    },
    Mesh {
        index: usize,
        convex: bool,
        scale: Vec3,
    },
}

/// A collider's geometry and its pose in the collider GameObject's (unscaled) frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeDesc {
    pub shape: Shape,
    pub local: Iso,
    pub trigger: bool,
}

pub fn collider(
    kind: &str,
    data: &Value,
    collision_mesh: Option<usize>,
    scale: Vec3,
) -> Option<ShapeDesc> {
    if !flag(&data["m_Enabled"]) {
        return None;
    }
    let s = scale.abs();
    let center = vec3(&data["m_Center"]) * scale;
    let trigger = flag(&data["m_IsTrigger"]);
    let (shape, rotation) = match kind {
        "SphereCollider" => (
            Shape::Sphere {
                radius: num(&data["m_Radius"]).abs() * s.max_element(),
            },
            Quat::IDENTITY,
        ),
        "BoxCollider" => (
            Shape::Box {
                half_extents: (vec3(&data["m_Size"]) * s * 0.5).abs(),
            },
            Quat::IDENTITY,
        ),
        "CapsuleCollider" => {
            let dir = data["m_Direction"].as_u64().unwrap_or(1) as usize;
            let (along, across, rotation) = match dir {
                0 => (s.x, s.y.max(s.z), Quat::IDENTITY),
                2 => (
                    s.z,
                    s.x.max(s.y),
                    Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
                ),
                _ => (
                    s.y,
                    s.x.max(s.z),
                    Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                ),
            };
            let radius = num(&data["m_Radius"]).abs() * across;
            let half_height = (num(&data["m_Height"]).abs() * along * 0.5 - radius).max(0.0);
            (
                Shape::Capsule {
                    radius,
                    half_height,
                },
                rotation,
            )
        }
        "MeshCollider" => {
            let convex = flag(&data["m_Convex"]);
            (
                Shape::Mesh {
                    index: collision_mesh?,
                    convex,
                    scale,
                },
                Quat::IDENTITY,
            )
        }
        _ => return None,
    };
    let degenerate = match &shape {
        Shape::Sphere { radius } | Shape::Capsule { radius, .. } => !(*radius > 0.0),
        Shape::Box { half_extents } => !(half_extents.min_element() > 0.0),
        Shape::Mesh { scale, .. } => !(scale.abs().min_element() > 0.0),
    };
    if degenerate {
        return None;
    }
    Some(ShapeDesc {
        shape,
        local: Iso::new(center, rotation),
        trigger,
    })
}

/// Unity combine enum order matches PxCombineMode (average, minimum, multiply, maximum).
pub fn combine_mode(name: &str) -> u32 {
    match name {
        "minimum" => 1,
        "multiply" => 2,
        "maximum" => 3,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Locked,
    Limited,
    Free,
}

impl Motion {
    fn from(v: &Value) -> Self {
        match v.as_u64() {
            Some(1) => Motion::Limited,
            Some(2) => Motion::Free,
            _ => Motion::Locked,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Spring {
    pub stiffness: f32,
    pub damping: f32,
}

/// One side of a joint limit. `spring.stiffness > 0` makes it soft.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Limit {
    pub restitution: f32,
    /// None = PhysX default contact distance (Unity's 0).
    pub contact_distance: Option<f32>,
    pub spring: Spring,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drive {
    pub stiffness: f32,
    pub damping: f32,
    pub force_limit: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveAxis {
    X,
    Y,
    Z,
    Swing,
    Twist,
    Slerp,
}

#[derive(Clone, Debug, PartialEq)]
pub struct JointDesc {
    /// Joint frame on actor0 (own body) and actor1 (connected body, or world when None).
    pub frame0: Iso,
    pub frame1: Iso,
    pub linear: [Motion; 3],
    pub angular: [Motion; 3],
    /// PhysX twist range in radians (already negated/swapped from Unity).
    pub twist: (f32, f32),
    pub twist_limit: Limit,
    /// Swing cone half-angles about joint Y and Z in radians.
    pub swing: (f32, f32),
    pub swing_limit: Limit,
    pub distance: f32,
    pub distance_limit: Limit,
    pub drives: Vec<(DriveAxis, Drive)>,
    pub target: Iso,
    pub target_velocity: Vec3,
    pub target_angular_velocity: Vec3,
    pub projection: Option<(f32, f32)>,
    pub collision: bool,
    pub preprocessing: bool,
    pub mass_scale: f32,
    pub connected_mass_scale: f32,
    pub break_force: f32,
    pub break_torque: f32,
}

/// Unity's joint-space basis: X = axis, Y = secondaryAxis orthogonalized, Z = X × Y.
pub fn joint_basis(axis: Vec3, secondary: Vec3) -> Quat {
    let x = axis.try_normalize().unwrap_or(Vec3::X);
    let mut z = x.cross(secondary);
    if z.length_squared() < 1e-12 {
        z = x.cross(if x.y.abs() < 0.9 { Vec3::Y } else { Vec3::Z });
    }
    let z = z.normalize();
    let y = z.cross(x);
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

fn deg(v: &Value) -> f32 {
    num_or(v, 0.0).to_radians()
}

fn limit(bounds: &Value, spring: &Value) -> Limit {
    let cd = num_or(&bounds["contactDistance"], 0.0);
    Limit {
        restitution: num_or(&bounds["bounciness"], 0.0),
        contact_distance: (cd > 0.0 && cd.is_finite()).then_some(cd),
        spring: Spring {
            stiffness: num_or(&spring["spring"], 0.0),
            damping: num_or(&spring["damper"], 0.0),
        },
    }
}

fn drive(v: &Value) -> Drive {
    // Absent drives (Fixed/Hinge joints) are off; a missing force limit is unlimited.
    Drive {
        stiffness: num_or(&v["positionSpring"], 0.0),
        damping: num_or(&v["positionDamper"], 0.0),
        force_limit: num(&v["maximumForce"]),
    }
}

/// ConfigurableJoint on a body at `own` (with lossy `own_scale`), connected to `connected`
/// (None = world). Anchors are recomputed like Unity when autoConfigureConnectedAnchor is set.
pub fn configurable_joint(
    data: &Value,
    own: Iso,
    own_scale: Vec3,
    connected: Option<(Iso, Vec3)>,
) -> JointDesc {
    let basis = joint_basis(vec3(&data["m_Axis"]), vec3(&data["m_SecondaryAxis"]));
    let anchor = vec3(&data["m_Anchor"]) * own_scale;
    let frame0 = Iso::new(anchor, basis);
    let world_frame = own.mul(&frame0);
    let frame1 = match connected {
        Some((body, scale)) => {
            let rel = body.inverse().mul(&world_frame);
            let position = if flag(&data["m_AutoConfigureConnectedAnchor"]) {
                rel.position
            } else {
                vec3(&data["m_ConnectedAnchor"]) * scale
            };
            Iso::new(position, rel.rotation)
        }
        None => {
            let position = if flag(&data["m_AutoConfigureConnectedAnchor"]) {
                world_frame.position
            } else {
                vec3(&data["m_ConnectedAnchor"])
            };
            Iso::new(position, world_frame.rotation)
        }
    };
    let (low, high) = (
        deg(&data["m_LowAngularXLimit"]["limit"]),
        deg(&data["m_HighAngularXLimit"]["limit"]),
    );
    let mut twist_limit = limit(&data["m_LowAngularXLimit"], &data["m_AngularXLimitSpring"]);
    twist_limit.restitution = twist_limit
        .restitution
        .max(num_or(&data["m_HighAngularXLimit"]["bounciness"], 0.0));
    let mut drives = vec![
        (DriveAxis::X, drive(&data["m_XDrive"])),
        (DriveAxis::Y, drive(&data["m_YDrive"])),
        (DriveAxis::Z, drive(&data["m_ZDrive"])),
    ];
    if data["m_RotationDriveMode"].as_u64() == Some(1) {
        drives.push((DriveAxis::Slerp, drive(&data["m_SlerpDrive"])));
    } else {
        drives.push((DriveAxis::Twist, drive(&data["m_AngularXDrive"])));
        drives.push((DriveAxis::Swing, drive(&data["m_AngularYZDrive"])));
    }
    let projection = (data["m_ProjectionMode"].as_u64() == Some(1)).then(|| {
        (
            num(&data["m_ProjectionDistance"]),
            deg(&data["m_ProjectionAngle"]),
        )
    });
    JointDesc {
        frame0,
        frame1,
        linear: [
            Motion::from(&data["m_XMotion"]),
            Motion::from(&data["m_YMotion"]),
            Motion::from(&data["m_ZMotion"]),
        ],
        angular: [
            Motion::from(&data["m_AngularXMotion"]),
            Motion::from(&data["m_AngularYMotion"]),
            Motion::from(&data["m_AngularZMotion"]),
        ],
        twist: (-high, -low),
        twist_limit,
        swing: (
            deg(&data["m_AngularYLimit"]["limit"]),
            deg(&data["m_AngularZLimit"]["limit"]),
        ),
        swing_limit: limit(&data["m_AngularYLimit"], &data["m_AngularYZLimitSpring"]),
        distance: num_or(&data["m_LinearLimit"]["limit"], 0.0),
        distance_limit: limit(&data["m_LinearLimit"], &data["m_LinearLimitSpring"]),
        drives,
        target: Iso::new(
            vec3(&data["m_TargetPosition"]),
            quat(&data["m_TargetRotation"]),
        ),
        target_velocity: vec3(&data["m_TargetVelocity"]),
        target_angular_velocity: vec3(&data["m_TargetAngularVelocity"]),
        projection,
        collision: flag(&data["m_EnableCollision"]),
        preprocessing: flag(&data["m_EnablePreprocessing"]),
        mass_scale: num_or(&data["m_MassScale"], 1.0),
        connected_mass_scale: num_or(&data["m_ConnectedMassScale"], 1.0),
        break_force: num(&data["m_BreakForce"]),
        break_torque: num(&data["m_BreakTorque"]),
    }
}

/// FixedJoint / HingeJoint expressed as the ConfigurableJoint Unity would build for them.
/// Hinge springs and motors are not modelled yet (no ragdoll uses them).
pub fn other_joint(
    kind: &str,
    data: &Value,
    own: Iso,
    own_scale: Vec3,
    connected: Option<(Iso, Vec3)>,
) -> Option<JointDesc> {
    let mut d = serde_json::json!({
        "m_Axis": data.get("m_Axis").cloned().unwrap_or(serde_json::json!({"x":1.0,"y":0.0,"z":0.0})),
        "m_SecondaryAxis": {"x":0.0,"y":1.0,"z":0.0},
        "m_Anchor": data.get("m_Anchor").cloned().unwrap_or(Value::Null),
        "m_ConnectedAnchor": data.get("m_ConnectedAnchor").cloned().unwrap_or(Value::Null),
        "m_AutoConfigureConnectedAnchor": data.get("m_AutoConfigureConnectedAnchor").cloned().unwrap_or(Value::Bool(true)),
        "m_EnableCollision": data["m_EnableCollision"].clone(),
        "m_EnablePreprocessing": data["m_EnablePreprocessing"].clone(),
        "m_MassScale": data["m_MassScale"].clone(),
        "m_ConnectedMassScale": data["m_ConnectedMassScale"].clone(),
        "m_BreakForce": data["m_BreakForce"].clone(),
        "m_BreakTorque": data["m_BreakTorque"].clone(),
        "m_RotationDriveMode": 0,
    });
    match kind {
        "FixedJoint" => {}
        "HingeJoint" => {
            let limited = flag(&data["m_UseLimits"]);
            d["m_AngularXMotion"] = (if limited { 1 } else { 2 }).into();
            d["m_LowAngularXLimit"] = serde_json::json!({"limit": data["m_Limits"]["min"], "bounciness": data["m_Limits"]["bounciness"]});
            d["m_HighAngularXLimit"] = serde_json::json!({"limit": data["m_Limits"]["max"], "bounciness": data["m_Limits"]["bounciness"]});
        }
        _ => return None,
    }
    Some(configurable_joint(&d, own, own_scale, connected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_basis_is_identity_and_right_handed_formula() {
        assert!(joint_basis(Vec3::X, Vec3::Y).abs_diff_eq(Quat::IDENTITY, 1e-6));
        let q = joint_basis(Vec3::new(0.0, 0.5, 1.0), Vec3::Y);
        let x = q * Vec3::X;
        assert!(x.abs_diff_eq(Vec3::new(0.0, 0.5, 1.0).normalize(), 1e-6));
        assert!((q * Vec3::Z).abs_diff_eq(x.cross(Vec3::Y).normalize(), 1e-6));
    }

    #[test]
    fn capsule_follows_unity_direction_and_scale() {
        let d = json!({"m_Enabled": true, "m_Radius": 0.2, "m_Height": 1.0, "m_Direction": 1, "m_Center": {"x":0,"y":0.5,"z":0}});
        let c = collider("CapsuleCollider", &d, None, Vec3::new(2.0, 1.0, 1.0)).unwrap();
        let Shape::Capsule {
            radius,
            half_height,
        } = c.shape
        else {
            panic!("not a capsule")
        };
        assert!((radius - 0.4).abs() < 1e-6 && (half_height - 0.1).abs() < 1e-6);
        assert!((c.local.rotation * Vec3::X).abs_diff_eq(Vec3::Y, 1e-6));
        assert_eq!(c.local.position, Vec3::new(0.0, 0.5, 0.0));
        let z = collider(
            "CapsuleCollider",
            &json!({"m_Enabled": true, "m_Radius": 0.1, "m_Height": 1.0, "m_Direction": 2}),
            None,
            Vec3::ONE,
        )
        .unwrap();
        assert!((z.local.rotation * Vec3::X).abs_diff_eq(Vec3::Z, 1e-6));
        assert!(collider(
            "SphereCollider",
            &json!({"m_Enabled": false, "m_Radius": 1.0}),
            None,
            Vec3::ONE
        )
        .is_none());
    }

    #[test]
    fn joint_frames_start_at_rest_and_twist_limits_flip() {
        let d = json!({
            "m_Axis": {"x":1,"y":0,"z":0}, "m_SecondaryAxis": {"x":0,"y":1,"z":0},
            "m_Anchor": {"x":0,"y":0.1,"z":0}, "m_AutoConfigureConnectedAnchor": true,
            "m_AngularXMotion": 1, "m_LowAngularXLimit": {"limit": -90.0}, "m_HighAngularXLimit": {"limit": 10.0},
            "m_RotationDriveMode": 1, "m_SlerpDrive": {"positionSpring": 10.0, "positionDamper": 1000.0, "maximumForce": 4.0},
            "m_BreakForce": null
        });
        let own = Iso::new(Vec3::new(1.0, 2.0, 3.0), Quat::from_rotation_y(0.7));
        let other = Iso::new(Vec3::new(0.0, 1.0, 0.0), Quat::from_rotation_x(-0.4));
        let j = configurable_joint(&d, own, Vec3::ONE, Some((other, Vec3::ONE)));
        let a = own.mul(&j.frame0);
        let b = other.mul(&j.frame1);
        assert!(a.position.abs_diff_eq(b.position, 1e-5));
        assert!(
            a.rotation.abs_diff_eq(b.rotation, 1e-5) || a.rotation.abs_diff_eq(-b.rotation, 1e-5)
        );
        assert!(
            (j.twist.0 + 10f32.to_radians()).abs() < 1e-6
                && (j.twist.1 - 90f32.to_radians()).abs() < 1e-6
        );
        assert_eq!(j.angular, [Motion::Limited, Motion::Locked, Motion::Locked]);
        assert!(j.drives.contains(&(
            DriveAxis::Slerp,
            Drive {
                stiffness: 10.0,
                damping: 1000.0,
                force_limit: 4.0
            }
        )));
        assert!(j.break_force.is_infinite());
    }
}
