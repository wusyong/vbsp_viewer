//! VMT materials — the file that says which VTF to load and how to shade it.
//!
//! A `.vmt` is a single KeyValues block whose *name* is the shader:
//!
//! ```text
//! "LightmappedGeneric"
//! {
//!     "$basetexture" "concrete/concretewall001"
//!     "$surfaceprop" "concrete"
//! }
//! ```
//!
//! # Patch
//!
//! TF2 uses `Patch` heavily — hundreds of materials are a two-line file that
//! inherits another and overrides one parameter:
//!
//! ```text
//! "Patch"
//! {
//!     include "materials/models/player/shared/gold_player.vmt"
//!     replace { "$basetexture" "models/player/heavy/heavy_gold" }
//!     insert  { "$phong" "1" }
//! }
//! ```
//!
//! `replace` overwrites a key whether or not it exists; `insert` only adds what
//! is missing. Patches chain, so resolution follows `include` recursively with a
//! depth limit — a mod with a cyclic include should produce an error, not a
//! stack overflow.
//!
//! # Texture paths
//!
//! `$basetexture` is a path **relative to `materials/` and without an
//! extension**: `concrete/concretewall001` means
//! `materials/concrete/concretewall001.vtf`. [`Material::texture_path`] does
//! that conversion, and tolerates the authored paths that already include one
//! or both.

use crate::keyvalues::{KeyValues, KvError};
use crate::{normalise_path, Vfs, VfsError};

/// How deep a `Patch` chain may go before it is treated as a cycle.
const MAX_PATCH_DEPTH: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum VmtError {
    #[error("{path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: KvError,
    },

    #[error("{path}: empty material — no shader block")]
    NoShader { path: String },

    #[error("{path}: Patch chain deeper than {MAX_PATCH_DEPTH}, probably a cycle")]
    PatchCycle { path: String },

    #[error("{path}: Patch has no `include`")]
    PatchWithoutInclude { path: String },

    #[error(transparent)]
    Vfs(#[from] VfsError),
}

pub type Result<T> = std::result::Result<T, VmtError>;

/// A resolved material: shader name plus flattened parameters.
#[derive(Clone, Debug)]
pub struct Material {
    /// Shader name as written, e.g. `LightmappedGeneric`, `WorldVertexTransition`.
    pub shader: String,
    /// The shader block's parameters, after any `Patch` has been applied.
    pub params: KeyValues,
    /// The VMT this was loaded from, normalised.
    pub path: String,
}

impl Material {
    /// Parse one `.vmt`'s text. `Patch` is *not* resolved here — see [`load`].
    pub fn parse(path: &str, text: &str) -> Result<Material> {
        let kv = KeyValues::parse(text).map_err(|source| VmtError::Parse {
            path: path.to_string(),
            source,
        })?;
        // The first block is the shader; a VMT has exactly one.
        let (shader, params) = kv
            .entries()
            .iter()
            .find_map(|(name, value)| value.as_block().map(|b| (name.clone(), b.clone())))
            .ok_or_else(|| VmtError::NoShader {
                path: path.to_string(),
            })?;

        Ok(Material {
            shader,
            params,
            path: normalise_path(path),
        })
    }

    pub fn is_patch(&self) -> bool {
        self.shader.eq_ignore_ascii_case("patch")
    }

    /// `$basetexture`, as a VTF path ready for the VFS.
    pub fn base_texture(&self) -> Option<String> {
        self.params.string("$basetexture").map(texture_path)
    }

    /// `$basetexture2` — the second layer of a `WorldVertexTransition` blend,
    /// which is how displacement dirt-to-grass transitions are authored.
    pub fn base_texture2(&self) -> Option<String> {
        self.params.string("$basetexture2").map(texture_path)
    }

    pub fn bump_map(&self) -> Option<String> {
        self.params.string("$bumpmap").map(texture_path)
    }

    pub fn translucent(&self) -> bool {
        self.params.bool("$translucent").unwrap_or(false)
    }

    pub fn alpha_test(&self) -> bool {
        self.params.bool("$alphatest").unwrap_or(false)
    }

    /// Source's default cutoff when `$alphatestreference` is absent.
    pub fn alpha_test_reference(&self) -> f32 {
        self.params.float("$alphatestreference").unwrap_or(0.5)
    }

    pub fn no_cull(&self) -> bool {
        self.params.bool("$nocull").unwrap_or(false)
    }

    pub fn surface_prop(&self) -> Option<&str> {
        self.params.string("$surfaceprop")
    }

    /// Whether the shader blends two base textures by vertex alpha.
    pub fn is_vertex_blend(&self) -> bool {
        self.shader.eq_ignore_ascii_case("WorldVertexTransition")
    }

    pub fn is_water(&self) -> bool {
        self.shader.to_ascii_lowercase().contains("water")
    }

    pub fn is_sky(&self) -> bool {
        self.shader.eq_ignore_ascii_case("Sky") || self.shader.eq_ignore_ascii_case("SkyBox")
    }

    /// Whether the shader ignores lighting entirely.
    pub fn is_unlit(&self) -> bool {
        self.shader.eq_ignore_ascii_case("UnlitGeneric")
            || self.shader.eq_ignore_ascii_case("UnlitTwoTexture")
    }
}

/// Turn a `$basetexture` value into a VFS path.
///
/// Authored values are normally bare (`concrete/wall001`) but sometimes carry
/// `materials/` or `.vtf` already, so both are handled rather than producing
/// `materials/materials/wall001.vtf.vtf`.
pub fn texture_path(value: &str) -> String {
    let value = normalise_path(value);
    let value = value.strip_suffix(".vtf").unwrap_or(&value);
    let value = value.strip_prefix("materials/").unwrap_or(value);
    format!("materials/{value}.vtf")
}

/// Turn a BSP texdata name into a VMT path.
///
/// BSP names arrive as `CONCRETE/CONCRETEWALL001` — no `materials/` prefix, no
/// extension, uppercase.
pub fn material_path(name: &str) -> String {
    let name = normalise_path(name);
    let name = name.strip_suffix(".vmt").unwrap_or(&name);
    let name = name.strip_prefix("materials/").unwrap_or(name);
    format!("materials/{name}.vmt")
}

/// Load a material through the VFS, resolving any `Patch` chain.
///
/// `path` may be a bare BSP material name or a full `materials/....vmt` path.
pub fn load(vfs: &Vfs, path: &str) -> Result<Material> {
    load_depth(vfs, &material_path(path), 0)
}

fn load_depth(vfs: &Vfs, path: &str, depth: usize) -> Result<Material> {
    if depth > MAX_PATCH_DEPTH {
        return Err(VmtError::PatchCycle {
            path: path.to_string(),
        });
    }
    let text = vfs.read_to_string(path)?;
    let material = Material::parse(path, &text)?;
    if !material.is_patch() {
        return Ok(material);
    }

    let include = material
        .params
        .string("include")
        .ok_or_else(|| VmtError::PatchWithoutInclude {
            path: path.to_string(),
        })?;
    let mut base = load_depth(vfs, &material_path(include), depth + 1)?;

    // `replace` overwrites, `insert` fills gaps. Blocks (a `proxies` section,
    // say) come through as blocks, so both are handled by value, not by string.
    if let Some(replace) = material.params.block("replace") {
        for (key, value) in replace.entries() {
            base.params.set(key, value.clone());
        }
    }
    if let Some(insert) = material.params.block("insert") {
        for (key, value) in insert.entries() {
            base.params.insert_if_absent(key, value.clone());
        }
    }
    // The patch's own identity is the file that was asked for; the shader stays
    // the base's, since that is what actually gets rendered.
    base.path = normalise_path(path);
    Ok(base)
}

/// Resolve a material and note whether the `Patch` indirection was used, for
/// the acceptance sweep's statistics.
pub fn load_reporting_patch(vfs: &Vfs, path: &str) -> Result<(Material, bool)> {
    let full = material_path(path);
    let text = vfs.read_to_string(&full)?;
    let was_patch = Material::parse(&full, &text)?.is_patch();
    Ok((load_depth(vfs, &full, 0)?, was_patch))
}

/// Materials referenced by a value that is itself a texture, for
/// [`Material`] consumers that want every VTF a material needs.
impl Material {
    pub fn textures(&self) -> Vec<String> {
        [self.base_texture(), self.base_texture2(), self.bump_map()]
            .into_iter()
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_material() {
        let text = r#"
"LightmappedGeneric"
{
	"$basetexture" "concrete/concretewall001"
	"$surfaceprop" "concrete"
	"%keywords" "tf"
}
"#;
        let m = Material::parse("materials/concrete/concretewall001.vmt", text).expect("parse");
        assert_eq!(m.shader, "LightmappedGeneric");
        assert_eq!(
            m.base_texture().as_deref(),
            Some("materials/concrete/concretewall001.vtf")
        );
        assert_eq!(m.surface_prop(), Some("concrete"));
        assert!(!m.translucent() && !m.alpha_test());
        assert_eq!(m.alpha_test_reference(), 0.5, "Source's default cutoff");
    }

    #[test]
    fn texture_paths_tolerate_already_qualified_values() {
        assert_eq!(texture_path("concrete/wall"), "materials/concrete/wall.vtf");
        // Both forms appear in shipped materials.
        assert_eq!(
            texture_path("materials/concrete/wall.vtf"),
            "materials/concrete/wall.vtf"
        );
        assert_eq!(texture_path("Concrete\\Wall.VTF"), "materials/concrete/wall.vtf");
    }

    #[test]
    fn bsp_material_names_become_vmt_paths() {
        // The exact shape a texdata name arrives in.
        assert_eq!(
            material_path("CONCRETE/CONCRETEWALL001"),
            "materials/concrete/concretewall001.vmt"
        );
        assert_eq!(
            material_path("materials/concrete/concretewall001.vmt"),
            "materials/concrete/concretewall001.vmt"
        );
    }

    #[test]
    fn vertex_blend_and_water_shaders_are_recognised() {
        let blend = Material::parse("x.vmt", r#""WorldVertexTransition" { "$basetexture" "a" "$basetexture2" "b" }"#)
            .expect("parse");
        assert!(blend.is_vertex_blend());
        assert_eq!(blend.base_texture2().as_deref(), Some("materials/b.vtf"));
        assert_eq!(blend.textures().len(), 2);

        let water = Material::parse("x.vmt", r#""Water" { "$basetexture" "a" }"#).expect("parse");
        assert!(water.is_water());
        // `WaterCheap` and the LDR/HDR variants must count too.
        let cheap = Material::parse("x.vmt", r#""UnlitGeneric" { }"#).expect("parse");
        assert!(cheap.is_unlit() && !cheap.is_water());
    }

    #[test]
    fn an_empty_or_shaderless_file_is_an_error() {
        assert!(matches!(
            Material::parse("x.vmt", ""),
            Err(VmtError::NoShader { .. })
        ));
        // A file that is only parameters with no shader block.
        assert!(matches!(
            Material::parse("x.vmt", r#""$basetexture" "a""#),
            Err(VmtError::NoShader { .. })
        ));
    }

    #[test]
    fn flags_read_source_truthiness() {
        let m = Material::parse(
            "x.vmt",
            r#""LightmappedGeneric" {
                "$translucent" "1"
                "$alphatest" "0"
                "$alphatestreference" "0.7"
                "$nocull" "1"
            }"#,
        )
        .expect("parse");
        assert!(m.translucent());
        assert!(!m.alpha_test(), "0 is false");
        assert_eq!(m.alpha_test_reference(), 0.7);
        assert!(m.no_cull());
    }

    #[test]
    fn patch_detection_is_case_insensitive() {
        // Shipped materials write `patch`, `Patch` and `PATCH`.
        for name in ["Patch", "patch", "PATCH"] {
            let m = Material::parse("x.vmt", &format!(r#""{name}" {{ include "y.vmt" }}"#))
                .expect("parse");
            assert!(m.is_patch(), "{name}");
        }
    }
}
