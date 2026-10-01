//! Physics for the Gang Beasts reimplementation: the original's PhysX 4.1 setup rebuilt from
//! exported Unity data (project settings, Rigidbodies, Colliders, ConfigurableJoints).
pub mod source;
pub mod unity;
mod world;

pub use source::{Pose, Settings, Sidecar};
pub use unity::Iso;
pub use world::{
    ignore_pair as world_ignore_pair, ActorRef, Body, ColliderRef, Contact, ContactKind,
    ContactPoint, Instance, World,
};
