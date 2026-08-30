//! VMT → the handful of material parameters a viewer actually needs.
//!
//! Modelled on `vbspview`'s `material.rs`, which is the better of the two
//! references here: `vbsp-to-gltf` loads `$bumpmap` into its material struct and
//! then never reads the field, so it pays to decode a normal map it throws away.

use crate::vfs::Vfs;
use vmt_parser::material::{Material, WaterMaterial};
use vmt_parser::{TextureTransform, VdfError, from_str};

/// `Material::resolve` needs one error type that covers both the include lookup
/// and the parse of whatever that lookup returns.
#[derive(Debug)]
enum VmtError {
    Missing,
    NotUtf8,
    Parse(VdfError),
}

impl From<VdfError> for VmtError {
    fn from(e: VdfError) -> Self {
        VmtError::Parse(e)
    }
}

impl std::fmt::Display for VmtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmtError::Missing => write!(f, "include not found"),
            VmtError::NotUtf8 => write!(f, "include is not utf-8"),
            VmtError::Parse(e) => write!(f, "{e}"),
        }
    }
}

/// Note the absence of `$bumpmap`. It is tempting to collect it here and worry
/// about wiring it up later, but a normal map needs per-vertex tangents and the
/// BSP gives us none — so a `bump_map` field would be decoded, stored, and never
/// read. That is precisely what `vbsp-to-gltf` does. It belongs here once
/// tangent generation does.
#[derive(Default)]
pub struct MaterialDef {
    /// Path relative to `materials/`, without extension.
    pub base_texture: Option<String>,
    pub translucent: bool,
    pub alpha_test: Option<f32>,
    pub no_cull: bool,
    pub transform: Option<TextureTransform>,
    /// Set for water with no `$basetexture`, which has no sensible texture to
    /// show. Rendered as flat blue rather than as an error.
    pub is_water: bool,
}

pub fn load(vfs: &Vfs, name: &str) -> Result<MaterialDef, String> {
    let path = format!(
        "materials/{}.vmt",
        name.trim_end_matches(".vmt").trim_start_matches('/')
    );
    let raw = vfs.load(&path).ok_or_else(|| format!("no {path}"))?;
    let text = String::from_utf8(raw).map_err(|e| e.to_string())?;

    let material = from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    // A VMT may be a `patch` that includes another and overrides a few keys, so
    // resolution needs the search path too. `resolve` reports include-parse
    // failures through the same error type as the loader closure, so that type
    // has to absorb `VdfError` as well.
    let material = material
        .resolve(|include| {
            let data = vfs.load(include).ok_or(VmtError::Missing)?;
            String::from_utf8(data).map_err(|_| VmtError::NotUtf8)
        })
        .map_err(|e| format!("{path}: {e}"))?;

    if let Material::Water(WaterMaterial {
        base_texture: None, ..
    }) = &material
    {
        return Ok(MaterialDef {
            is_water: true,
            translucent: true,
            ..MaterialDef::default()
        });
    }

    Ok(MaterialDef {
        base_texture: material.base_texture().map(str::to_string),
        // `$surfaceprop glass` is translucent in practice even when the shader
        // does not say so.
        translucent: material.translucent() || material.surface_prop() == Some("glass"),
        alpha_test: material.alpha_test(),
        no_cull: material.no_cull(),
        transform: material
            .base_texture_transform()
            .filter(|t| **t != TextureTransform::default())
            .cloned(),
        is_water: false,
    })
}
