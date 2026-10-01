//! Gameplay logic ported from the Femur / GB.Game namespaces of GameAssembly.dll.
pub mod actor;
pub mod beast;
pub mod input;
pub mod movement;

pub use actor::Actor;
pub use beast::{Beast, Part};
pub use input::InputState;
