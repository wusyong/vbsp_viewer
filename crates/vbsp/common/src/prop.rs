use glam::{Quat, Vec3};

#[derive(Debug, Clone)]
pub struct PropPlacement<'a> {
    pub model: &'a str,
    pub rotation: Quat,
    pub scale: f32,
    pub origin: Vec3,
    pub skin: i32,
}

/// Abstraction for various ways props are placed in a bsp
pub trait AsPropPlacement<'a> {
    fn as_prop_placement(&self) -> PropPlacement<'a>;
}
