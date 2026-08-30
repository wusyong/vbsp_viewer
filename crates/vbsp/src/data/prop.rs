use crate::{Handle, StaticPropLumpEntry};
use vbsp_common::{AsPropPlacement, PropPlacement};

impl<'a> AsPropPlacement<'a> for Handle<'a, StaticPropLumpEntry> {
    fn as_prop_placement(&self) -> PropPlacement<'a> {
        PropPlacement {
            model: self.model(),
            rotation: self.angles.as_quaternion(),
            scale: 1.0,
            origin: self.origin,
            skin: self.skin,
        }
    }
}
