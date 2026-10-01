//! Femur.MovementHandeler, ported from GameAssembly.dll.
use gb_phys::World;
use glam::{Quat, Vec3};

/// MovementHandeler.AlignToVector (RVA 0x46A2C0): torque `part` so that `alignment` (a world
/// direction on the part) turns toward `target`, predicting ahead by the current spin.
pub fn align_to_vector(
    world: &mut World,
    part: usize,
    alignment: Vec3,
    target: Vec3,
    stability: f32,
    speed: f32,
) {
    let w = world.angular_velocity(part);
    let angle = (w.length() * 57.29578 * stability / speed).to_radians();
    // Quaternion.AngleAxis normalizes the axis and yields identity for a zero axis.
    let predict = match w.try_normalize() {
        Some(axis) => Quat::from_axis_angle(axis, angle),
        None => Quat::IDENTITY,
    };
    let torque = (predict * alignment).cross(target * 10.0);
    if !torque.is_finite() {
        return;
    }
    world.add_torque(part, torque * speed * speed, 0);
}
