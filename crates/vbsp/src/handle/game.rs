use super::Handle;
use crate::data::*;

impl<'a> Handle<'a, StaticPropLumpEntry> {
    pub fn model(&self) -> &'a str {
        self.bsp.static_props.dict.name[self.prop_type as usize].as_str()
    }
}
