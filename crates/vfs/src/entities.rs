//! The BSP `ENTITIES` lump — KeyValues, so it lives here rather than in `bsp`.
//!
//! The lump is a bare sequence of blocks with no keys and no root:
//!
//! ```text
//! {
//! "classname" "worldspawn"
//! "skyname" "sky_tf2_04"
//! }
//! {
//! "classname" "func_door"
//! "model" "*12"
//! "origin" "-1024 512 64"
//! }
//! ```
//!
//! This module exists to unblock **brush entities**, deferred from M2 for want
//! of a KeyValues parser. An entity whose `model` is `*N` draws BSP model `N`:
//! doors, gates, moving platforms, and the func_brush detail that makes up a
//! surprising amount of a TF2 map. Without them, a map has holes where its
//! doors should be.
//!
//! `bsp` deliberately does not depend on this crate — it hands over the lump
//! text and stays free of any text format.

use crate::keyvalues::{KeyValues, Result as KvResult};

/// One entity: its classname plus every key it declared, in file order.
#[derive(Clone, Debug)]
pub struct Entity {
    pub classname: String,
    pub keys: KeyValues,
}

impl Entity {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys.string(key)
    }

    /// `model "*N"` → `Some(N)`: this entity draws BSP model `N`.
    ///
    /// A `model` naming a `.mdl` file is a static prop, which phase 1 does not
    /// draw, and yields `None`.
    pub fn brush_model(&self) -> Option<usize> {
        self.get("model")?.trim().strip_prefix('*')?.parse().ok()
    }

    /// `origin`, in Source units. Absent means the origin, which is what the
    /// engine assumes.
    pub fn origin(&self) -> [f32; 3] {
        self.vector("origin").unwrap_or([0.0; 3])
    }

    /// `angles` as pitch/yaw/roll in degrees, Source's order.
    pub fn angles(&self) -> [f32; 3] {
        self.vector("angles").unwrap_or([0.0; 3])
    }

    /// A whitespace-separated triple, e.g. `origin "-1024 512 64"`.
    pub fn vector(&self, key: &str) -> Option<[f32; 3]> {
        let text = self.get(key)?;
        let mut out = [0.0f32; 3];
        let mut parts = text.split_whitespace();
        for slot in &mut out {
            *slot = parts.next()?.parse().ok()?;
        }
        Some(out)
    }

    /// `targetname`, the name other entities refer to this one by.
    pub fn targetname(&self) -> Option<&str> {
        self.get("targetname")
    }
}

/// Parse the whole lump.
///
/// The text is NUL-terminated in the file; the terminator and anything after it
/// are ignored so callers can pass the raw lump bytes as a string.
pub fn parse(text: &str) -> KvResult<Vec<Entity>> {
    let text = text.split('\0').next().unwrap_or(text);
    Ok(KeyValues::parse_blocks(text)?
        .into_iter()
        .map(|keys| Entity {
            classname: keys.string("classname").unwrap_or_default().to_string(),
            keys,
        })
        .collect())
}

/// The `worldspawn` entity, which carries map-wide settings (`skyname`,
/// `detailmaterial`). It is always the first block, but find it by classname
/// rather than by position.
pub fn worldspawn(entities: &[Entity]) -> Option<&Entity> {
    entities
        .iter()
        .find(|e| e.classname.eq_ignore_ascii_case("worldspawn"))
}

/// Every entity that draws a BSP model, paired with its model index.
///
/// Excludes model 0: that is worldspawn itself, already drawn as the world.
pub fn brush_entities(entities: &[Entity]) -> Vec<(usize, &Entity)> {
    entities
        .iter()
        .filter_map(|e| e.brush_model().map(|m| (m, e)))
        .filter(|(model, _)| *model != 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped exactly like a real lump: no root, one key per line, and a
    /// trailing NUL.
    const LUMP: &str = "{\n\
        \"world_maxs\" \"4088 4600 1712\"\n\
        \"classname\" \"worldspawn\"\n\
        \"skyname\" \"sky_badlands_01\"\n\
        }\n\
        {\n\
        \"model\" \"*1\"\n\
        \"origin\" \"-1024 512 64\"\n\
        \"angles\" \"0 90 0\"\n\
        \"classname\" \"func_door\"\n\
        \"targetname\" \"door_blue\"\n\
        }\n\
        {\n\
        \"model\" \"models/props_gameplay/cap_point_base.mdl\"\n\
        \"classname\" \"prop_dynamic\"\n\
        }\n\
        {\n\
        \"model\" \"*0\"\n\
        \"classname\" \"func_brush\"\n\
        }\n\0";

    #[test]
    fn parses_the_lump_shape() {
        let entities = parse(LUMP).expect("parse");
        assert_eq!(entities.len(), 4, "{entities:#?}");
        assert_eq!(entities[0].classname, "worldspawn");
        assert_eq!(
            worldspawn(&entities).and_then(|w| w.get("skyname")),
            Some("sky_badlands_01")
        );
    }

    #[test]
    fn a_trailing_nul_does_not_become_part_of_the_last_key() {
        // The lump is NUL-terminated in the file; passing the bytes straight
        // through as a string would otherwise leave it in the parse.
        let entities = parse(LUMP).expect("parse");
        assert_eq!(entities.last().expect("last").classname, "func_brush");
    }

    #[test]
    fn brush_models_are_distinguished_from_studio_models() {
        let entities = parse(LUMP).expect("parse");
        assert_eq!(entities[1].brush_model(), Some(1));
        // A `.mdl` is a static prop, out of scope for phase 1.
        assert_eq!(entities[2].brush_model(), None, "mdl treated as a brush");
    }

    #[test]
    fn model_zero_is_excluded_because_it_is_worldspawn() {
        // A func_brush pointing at model 0 would draw the whole world a second
        // time, z-fighting with itself.
        let entities = parse(LUMP).expect("parse");
        let brushes = brush_entities(&entities);
        assert_eq!(brushes.len(), 1, "{brushes:?}");
        assert_eq!(brushes[0].0, 1);
        assert_eq!(brushes[0].1.targetname(), Some("door_blue"));
    }

    #[test]
    fn vectors_parse_and_default_to_zero() {
        let entities = parse(LUMP).expect("parse");
        assert_eq!(entities[1].origin(), [-1024.0, 512.0, 64.0]);
        assert_eq!(entities[1].angles(), [0.0, 90.0, 0.0]);
        // An entity with no origin sits at the world origin.
        assert_eq!(entities[3].origin(), [0.0; 3]);
        // A malformed vector is None rather than a partial read.
        assert_eq!(entities[0].vector("skyname"), None);
        assert_eq!(
            entities[0].vector("world_maxs"),
            Some([4088.0, 4600.0, 1712.0])
        );
    }

    #[test]
    fn an_empty_lump_is_no_entities() {
        assert!(parse("").expect("parse").is_empty());
        assert!(parse("\0").expect("parse").is_empty());
    }
}
