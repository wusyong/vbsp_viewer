//! Turn Source materials into Bevy ones: VMT to `StandardMaterial`, VTF to
//! `Image`.
//!
//! # Textures go to the GPU compressed
//!
//! **99.7% of TF2's textures are DXT1, DXT3 or DXT5**, which map exactly onto
//! wgpu's BC1/BC2/BC3. Those are handed over as blocks with their whole mip
//! chain, so VRAM stays at parity with the real game and no CPU decode happens
//! at all — a 1024x1024 DXT1 costs 700 KiB rather than the 5.6 MiB it would as
//! RGBA8. Only the rare uncompressed formats (`BGR888`, `ABGR8888`, `UV88`,
//! `RGBA16161616F`, …) take the CPU path.
//!
//! Two details make the passthrough work:
//!
//! - **VTF stores mips smallest-first; wgpu wants largest-first.** The chain is
//!   reversed on upload.
//! - **A BC texture whose base size is not a multiple of 4 cannot carry a full
//!   mip chain** on wgpu, and TF2 has none — every shipped VTF is a power of
//!   two — but a packed community texture might, so the chain is truncated at
//!   the last mip that is still block-aligned rather than failing.
//!
//! # Shaders
//!
//! Source's shader census across all 233 maps: 43 772 `LightmappedGeneric`,
//! 2 073 `WorldVertexTransition`, 1 169 `UnlitGeneric`, 547 `Water`, then a
//! thin tail. The first covers everything Bevy's `StandardMaterial` already
//! does; only the second needs a shader, which is what
//! [`SourceExtension`] adds. See `source_material.wgsl`.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use std::collections::HashMap;
use vfs::{vmt, Vfs};
use vtf::{ImageFormat, Vtf};

/// Where the extension's WGSL lives once embedded in the binary.
const SHADER_PATH: &str = "embedded://bevy_bsp/source_material.wgsl";

/// Source's `OVERBRIGHT`: `LightmappedGeneric` multiplies the lightmap by 2
/// before modulating albedo. With a real texture in place — Source albedos
/// average well under 0.5 — this is what makes a lit surface read correctly.
pub const SOURCE_OVERBRIGHT: f32 = 2.0;

/// The material every map surface uses.
pub type SourceMaterial = ExtendedMaterial<StandardMaterial, SourceExtension>;

/// Uniform block for the extension. 16-byte aligned for WebGL2 compatibility.
#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct SourceParams {
    /// 1 when `$basetexture2` should be blended in by vertex alpha.
    pub blend: u32,
    pub _padding: UVec3,
}

/// `WorldVertexTransition`'s second albedo layer, and the flag that enables it.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct SourceExtension {
    /// `$basetexture2`. `None` binds Bevy's fallback image, which the shader
    /// then ignores because `blend` is 0.
    #[texture(100)]
    #[sampler(101)]
    pub base_texture2: Option<Handle<Image>>,
    #[uniform(102)]
    pub params: SourceParams,
}

impl MaterialExtension for SourceExtension {
    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }
}

/// Registers the material and embeds its shader.
pub struct SourceMaterialPlugin;

impl Plugin for SourceMaterialPlugin {
    fn build(&self, app: &mut App) {
        // Embedded rather than loaded from an `assets/` directory: the viewer
        // is a single binary and the shader is part of this crate.
        bevy::asset::embedded_asset!(app, "source_material.wgsl");
        app.add_plugins(MaterialPlugin::<SourceMaterial>::default());
    }
}

/// Counters for the load, for the HUD and the acceptance sweep.
#[derive(Clone, Debug, Default)]
pub struct MaterialStats {
    /// Materials asked for, and how many resolved to a VMT.
    pub requested: usize,
    pub resolved: usize,
    /// Material names with no `.vmt` anywhere — drawn with the missing-material
    /// colour, as the engine draws its checkerboard.
    pub missing: Vec<String>,
    /// Distinct textures uploaded, and how they got there.
    pub textures: usize,
    pub bcn_passthrough: usize,
    pub cpu_decoded: usize,
    /// `$basetexture` values that resolved to no VTF.
    pub missing_textures: Vec<String>,
    /// Bytes of texture data uploaded.
    pub texture_bytes: u64,
    pub translucent: usize,
    pub alpha_tested: usize,
    pub vertex_blend: usize,
    pub unlit: usize,
}

impl MaterialStats {
    pub fn textured(&self) -> usize {
        self.resolved - self.missing_textures.len().min(self.resolved)
    }
}

/// The material for each TEXDATA index, plus how the load went.
pub struct MapMaterials {
    pub by_texdata: HashMap<i32, Handle<SourceMaterial>>,
    pub stats: MaterialStats,
}

impl MapMaterials {
    pub fn get(&self, texdata: i32) -> Option<&Handle<SourceMaterial>> {
        self.by_texdata.get(&texdata)
    }
}

/// What a material load needs from the world.
pub struct MaterialContext<'a> {
    /// The search path stack. `None` — no game directory, or a broken one —
    /// forces the debug palette rather than failing the map.
    pub vfs: Option<&'a Vfs>,
    /// From [`bsp::Bsp::texture_names`], indexed by TEXDATA index.
    pub material_names: &'a [&'a str],
    /// `StandardMaterial::lightmap_exposure`; zero when lightmaps are off.
    pub lightmap_exposure: f32,
    /// Ignore `$basetexture` and paint each material a distinct flat colour.
    pub debug_palette: bool,
}

impl MaterialContext<'_> {
    fn palette_only(&self) -> bool {
        self.debug_palette || self.vfs.is_none()
    }
}

/// Resolve and upload the materials a map's geometry actually references.
///
/// One `SourceMaterial` per TEXDATA index, which is also one draw call per
/// material: `cp_badlands` has 151 texdatas against 13 845 faces.
///
/// `used_texdata` is deliberately required rather than defaulting to every
/// entry in the lump. **A map's TEXDATA list includes materials no visible face
/// uses** — `TOOLS/TOOLSTRIGGER`, `TOOLSHINT`, `TOOLSAREAPORTAL` and friends,
/// whose faces the geometry builder already drops via `SURF_SKIP_RENDER`. On
/// `cp_badlands` that is 37 of 151 materials, each of which would otherwise
/// upload a texture nothing draws.
pub fn load_materials(
    context: &MaterialContext,
    used_texdata: impl IntoIterator<Item = i32>,
    materials: &mut Assets<SourceMaterial>,
    images: &mut Assets<Image>,
) -> MapMaterials {
    let mut loader = Loader {
        context,
        cache: HashMap::new(),
        stats: MaterialStats::default(),
    };
    let mut by_texdata: HashMap<i32, Handle<SourceMaterial>> = HashMap::new();

    for texdata in used_texdata {
        if by_texdata.contains_key(&texdata) {
            continue;
        }
        let name = usize::try_from(texdata)
            .ok()
            .and_then(|i| context.material_names.get(i).copied())
            .unwrap_or("<unknown>");
        let handle = loader.build(materials, images, name);
        by_texdata.insert(texdata, handle);
    }

    MapMaterials {
        by_texdata,
        stats: loader.stats,
    }
}

/// The colour of a material that could not be found, standing in for the
/// engine's missing-material checkerboard.
pub const MISSING_MATERIAL_COLOR: Color = Color::srgb(1.0, 0.0, 1.0);

struct Loader<'a> {
    context: &'a MaterialContext<'a>,
    /// Texture path to uploaded image, so a texture shared by twenty materials
    /// is uploaded once.
    cache: HashMap<String, Option<Handle<Image>>>,
    stats: MaterialStats,
}

impl Loader<'_> {
    fn build(
        &mut self,
        materials: &mut Assets<SourceMaterial>,
        images: &mut Assets<Image>,
        name: &str,
    ) -> Handle<SourceMaterial> {
        self.stats.requested += 1;

        // No VFS at all: every material is the palette, and none is "missing",
        // because nothing was ever looked up.
        if self.context.vfs.is_none() {
            return self.palette(materials, name);
        }
        let vfs = self.context.vfs.expect("checked above");

        let material = match vmt::load(vfs, name) {
            Ok(material) => material,
            Err(_) => {
                self.stats.missing.push(name.to_string());
                return materials.add(SourceMaterial {
                    base: StandardMaterial {
                        base_color: MISSING_MATERIAL_COLOR,
                        unlit: true,
                        ..default()
                    },
                    extension: SourceExtension::default(),
                });
            }
        };
        self.stats.resolved += 1;

        if self.context.palette_only() {
            return self.palette(materials, name);
        }

        // Albedo is sRGB; a normal map or SSBUMP is linear data and must not be
        // gamma-decoded.
        let base = material
            .base_texture()
            .and_then(|path| self.texture(images, &path, TextureUse::Colour));
        let blend = material.is_vertex_blend();
        let second = if blend {
            material
                .base_texture2()
                .and_then(|path| self.texture(images, &path, TextureUse::Colour))
        } else {
            None
        };
        if blend {
            self.stats.vertex_blend += 1;
        }

        let (alpha_mode, base_color) = self.surface_appearance(&material, base.is_some());
        let unlit = material.is_unlit();
        if unlit {
            self.stats.unlit += 1;
        }

        materials.add(SourceMaterial {
            base: StandardMaterial {
                base_color,
                base_color_texture: base,
                perceptual_roughness: 0.9,
                // Source's world materials have no specular workflow worth
                // emulating in phase 1; a fully rough dielectric keeps the
                // lightmap doing all the work.
                metallic: 0.0,
                reflectance: 0.0,
                alpha_mode,
                unlit,
                // `$nocull` exists for foliage and decals that are meant to be
                // seen from behind.
                double_sided: material.no_cull(),
                cull_mode: if material.no_cull() {
                    None
                } else {
                    Some(bevy::render::render_resource::Face::Back)
                },
                lightmap_exposure: if unlit {
                    0.0
                } else {
                    self.context.lightmap_exposure
                },
                ..default()
            },
            extension: SourceExtension {
                base_texture2: second.clone(),
                params: SourceParams {
                    blend: u32::from(blend && second.is_some()),
                    ..default()
                },
            },
        })
    }

    /// A flat, stable colour per material name, for judging geometry and
    /// lighting without textures in the way.
    fn palette(
        &self,
        materials: &mut Assets<SourceMaterial>,
        name: &str,
    ) -> Handle<SourceMaterial> {
        materials.add(SourceMaterial {
            base: StandardMaterial {
                base_color: super::debug_material_color(name),
                perceptual_roughness: 0.9,
                lightmap_exposure: self.context.lightmap_exposure,
                ..default()
            },
            extension: SourceExtension::default(),
        })
    }

    /// Alpha handling and the fallback colour when no texture resolved.
    fn surface_appearance(&mut self, material: &vmt::Material, has_texture: bool) -> (AlphaMode, Color) {
        let mut colour = if has_texture {
            Color::WHITE
        } else {
            // No `$basetexture`: a flat mid-grey reads as untextured rather
            // than as black geometry.
            Color::srgb(0.5, 0.5, 0.5)
        };

        if material.is_water() {
            // Water's colour comes from `$refracttexture` and a fresnel term,
            // neither of which phase 1 has. A translucent teal is closer than
            // grey and obviously provisional.
            self.stats.translucent += 1;
            return (AlphaMode::Blend, Color::srgba(0.18, 0.32, 0.35, 0.6));
        }
        if material.alpha_test() {
            self.stats.alpha_tested += 1;
            return (AlphaMode::Mask(material.alpha_test_reference()), colour);
        }
        if material.translucent() {
            self.stats.translucent += 1;
            if !has_texture {
                colour = Color::srgba(0.5, 0.5, 0.5, 0.5);
            }
            return (AlphaMode::Blend, colour);
        }
        (AlphaMode::Opaque, colour)
    }

    /// Upload a texture, or return a cached handle.
    fn texture(
        &mut self,
        images: &mut Assets<Image>,
        path: &str,
        use_: TextureUse,
    ) -> Option<Handle<Image>> {
        if let Some(cached) = self.cache.get(path) {
            return cached.clone();
        }

        let handle = self.upload(images, path, use_);
        self.cache.insert(path.to_string(), handle.clone());
        handle
    }

    fn upload(
        &mut self,
        images: &mut Assets<Image>,
        path: &str,
        use_: TextureUse,
    ) -> Option<Handle<Image>> {
        let bytes = match self.context.vfs?.read(path) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.stats.missing_textures.push(path.to_string());
                return None;
            }
        };
        let vtf = match Vtf::parse(&bytes) {
            Ok(vtf) => vtf,
            Err(e) => {
                warn!("{path}: {e}");
                self.stats.missing_textures.push(path.to_string());
                return None;
            }
        };
        // A normal map's data is not colour, whatever the material says.
        let srgb = use_ == TextureUse::Colour && !vtf.is_normal_map();

        match vtf_image(&vtf, srgb) {
            Ok(image) => {
                self.stats.textures += 1;
                self.stats.texture_bytes += image.data.as_ref().map_or(0, |d| d.len() as u64);
                if vtf.format.is_block_compressed() {
                    self.stats.bcn_passthrough += 1;
                } else {
                    self.stats.cpu_decoded += 1;
                }
                Some(images.add(image))
            }
            Err(e) => {
                warn!("{path}: {e}");
                self.stats.missing_textures.push(path.to_string());
                None
            }
        }
    }
}

/// Why a VTF could not become an `Image`.
///
/// Separate from [`vtf::VtfError`] because a size mismatch is not a parse
/// failure: the VTF read fine and the *result* is inconsistent with the
/// descriptor built from it.
#[derive(Debug, thiserror::Error)]
pub enum TextureError {
    #[error(transparent)]
    Vtf(#[from] vtf::VtfError),

    /// The concatenated pixel data is not the size the texture descriptor
    /// claims. See [`gpu_bytes`] for why this has to be checked by hand.
    #[error(
        "{format:?} {width}x{height} with {mips} mip level(s) needs {expected} bytes, got {actual}"
    )]
    SizeMismatch {
        format: TextureFormat,
        width: u32,
        height: u32,
        mips: u32,
        expected: u64,
        actual: u64,
    },

    /// wgpu reports no fixed block size for the format — depth and stencil
    /// formats, none of which a VTF can hold. Failing closed rather than
    /// skipping the check.
    #[error("wgpu reports no block size for {0:?}, so its size cannot be checked")]
    UnknownBlockSize(TextureFormat),
}

/// Exactly how many bytes wgpu will read for a 2D texture of this format, size
/// and mip count.
///
/// # Why this is computed by hand
///
/// [`Image::new`] checks that `data.len()` matches its descriptor and is
/// therefore unusable here: its check assumes a **single** mip level, so handing
/// it a whole chain fails. [`Image::new_uninit`] skips the check entirely — it is
/// a plain struct literal with `data: None` — which is what makes it the right
/// constructor for a chain and also what leaves nothing validating the result.
///
/// Without this, a wrong mip offset or a miscounted level produces a
/// **plausible-looking texture** rather than an error, on the path 99.7% of TF2's
/// textures take. That is the worst failure mode there is: silently wrong output
/// that no sweep reports.
///
/// Block-compressed formats count in whole blocks, so each level is
/// `ceil(w / block_w) * ceil(h / block_h)` blocks — which is also how
/// [`collect_mips`] can stop early without the arithmetic disagreeing.
pub fn gpu_bytes(
    format: TextureFormat,
    width: u32,
    height: u32,
    mips: u32,
) -> Result<u64, TextureError> {
    let block_bytes = format
        .block_copy_size(None)
        .ok_or(TextureError::UnknownBlockSize(format))? as u64;
    let (block_w, block_h) = format.block_dimensions();

    let mut total = 0u64;
    for level in 0..mips {
        // wgpu's mip sizing: halve and clamp at 1, per level.
        let w = (width >> level).max(1);
        let h = (height >> level).max(1);
        total += u64::from(w.div_ceil(block_w)) * u64::from(h.div_ceil(block_h)) * block_bytes;
    }
    Ok(total)
}

/// Whether a texture holds colour (sRGB) or data (linear).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextureUse {
    Colour,
    Data,
}

/// wgpu format for a VTF format, when it can be uploaded without decoding.
///
/// `BGRX`/`UVLX` are deliberately absent: their fourth channel is undefined
/// padding, and uploading it as alpha would make the surface randomly
/// transparent. They take the CPU path, which forces alpha to 255.
pub fn passthrough_format(format: ImageFormat, srgb: bool) -> Option<TextureFormat> {
    Some(match (format, srgb) {
        // DXT1 with a one-bit alpha mode is still BC1: the mode is per block.
        (ImageFormat::Dxt1 | ImageFormat::Dxt1OneBitAlpha, true) => TextureFormat::Bc1RgbaUnormSrgb,
        (ImageFormat::Dxt1 | ImageFormat::Dxt1OneBitAlpha, false) => TextureFormat::Bc1RgbaUnorm,
        (ImageFormat::Dxt3, true) => TextureFormat::Bc2RgbaUnormSrgb,
        (ImageFormat::Dxt3, false) => TextureFormat::Bc2RgbaUnorm,
        (ImageFormat::Dxt5, true) => TextureFormat::Bc3RgbaUnormSrgb,
        (ImageFormat::Dxt5, false) => TextureFormat::Bc3RgbaUnorm,
        (ImageFormat::Rgba8888, true) => TextureFormat::Rgba8UnormSrgb,
        (ImageFormat::Rgba8888, false) => TextureFormat::Rgba8Unorm,
        (ImageFormat::Bgra8888, true) => TextureFormat::Bgra8UnormSrgb,
        (ImageFormat::Bgra8888, false) => TextureFormat::Bgra8Unorm,
        _ => return None,
    })
}

/// Build a Bevy `Image` from a VTF, preferring block passthrough.
///
/// Uses frame [`Vtf::first_frame`] and face 0: animated textures and cubemap
/// faces are phase 2.
pub fn vtf_image(vtf: &Vtf, srgb: bool) -> Result<Image, TextureError> {
    let frame = usize::from(vtf.first_frame).min(usize::from(vtf.frames) - 1);
    let (format, data, mips) = match passthrough_format(vtf.format, srgb) {
        Some(format) => {
            let (data, mips) = collect_mips(vtf, frame)?;
            (format, data, mips)
        }
        None => {
            // CPU decode, base mip only: these formats are rare enough that
            // rebuilding a chain is not worth the code.
            let rgba = vtf.decode_rgba8(0, frame, 0)?;
            let format = if srgb {
                TextureFormat::Rgba8UnormSrgb
            } else {
                TextureFormat::Rgba8Unorm
            };
            (format, rgba, 1)
        }
    };

    let (width, height, _) = vtf.mip_dimensions(0);
    // `Image::new` must not be used here. It carries a `debug_assert_eq!` that
    // `data.len()` is exactly **one** mip level's worth of pixels, so handing it
    // a whole chain panics in a debug build while release silently compiles the
    // check out:
    //
    // ```text
    // assertion `left == right` failed: Pixel data, size and format have to match
    //   bevy_image/src/image.rs:1110
    // ```
    //
    // The assertion is skipped for BC formats — `pixel_size()` returns `Err`
    // for anything whose block is larger than 1x1 — so only the uncompressed
    // passthrough formats (`RGBA8888`, `BGRA8888`: 481 of the textures the maps
    // use) ever tripped it, which is why the first maps rendered looked fine.
    // `new_uninit` skips a single-level invariant that a mip chain does not
    // have, and the descriptor and data are then set together — so the size
    // check `Image::new` would have done is done by hand below instead. Skipping
    // it is what let a wrong offset yield a plausible texture.
    let expected = gpu_bytes(format, width, height, mips)?;
    if expected != data.len() as u64 {
        return Err(TextureError::SizeMismatch {
            format,
            width,
            height,
            mips,
            expected,
            actual: data.len() as u64,
        });
    }

    let mut image = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        // Never read back after upload.
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = mips;
    image.data = Some(data);
    image.sampler = ImageSampler::Descriptor(sampler_for(vtf));
    Ok(image)
}

/// Concatenate the mip chain largest-first, stopping where wgpu can no longer
/// take a block-compressed level.
pub(crate) fn collect_mips(vtf: &Vtf, frame: usize) -> Result<(Vec<u8>, u32), vtf::VtfError> {
    let block = vtf.format.block_bytes().map(|_| 4u32);
    let mut data = Vec::with_capacity(vtf.image_bytes() as usize);
    let mut count = 0u32;

    for mip in 0..vtf.mip_count as usize {
        let (w, h, _) = vtf.mip_dimensions(mip);
        // wgpu requires every level of a BC texture to be a whole number of
        // blocks. TF2's textures are all powers of two so this never triggers,
        // but a packed community texture could be 96x40, where the chain has
        // to stop before 3x1.
        if let Some(block) = block
            && mip > 0
            && (w % block != 0 || h % block != 0)
        {
            break;
        }
        data.extend_from_slice(vtf.surface(mip, frame, 0)?);
        count += 1;
    }
    Ok((data, count.max(1)))
}

/// Sampler from the VTF's own flags.
///
/// Every filter is linear, including `mipmap_filter` on a single-mip texture.
/// wgpu rejects a sampler with `anisotropy_clamp != 1` unless **all** filters
/// are linear, and a `Nearest` mipmap filter for the no-mip case failed on 7 of
/// the first 24 maps rendered:
///
/// ```text
/// In Device::create_sampler
///   Invalid filter mode for mipmapFilter: Nearest.
///   When anistropic clamp is not 1 (it is 8), all filter modes must be linear.
/// ```
fn sampler_for(vtf: &Vtf) -> ImageSamplerDescriptor {
    let address = |clamp: bool| {
        if clamp {
            ImageAddressMode::ClampToEdge
        } else {
            // Source's default: world textures tile.
            ImageAddressMode::Repeat
        }
    };
    ImageSamplerDescriptor {
        address_mode_u: address(vtf.clamp_s()),
        address_mode_v: address(vtf.clamp_t()),
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        // Ground planes at a grazing angle are most of a TF2 map.
        anisotropy_clamp: 8,
        ..default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-computed byte counts, so the arithmetic is pinned to numbers that
    /// were worked out independently of the code.
    #[test]
    fn gpu_bytes_matches_hand_computed_sizes() {
        // BC1: 4x4 blocks of 8 bytes. 512x512 is 128x128 blocks.
        assert_eq!(
            gpu_bytes(TextureFormat::Bc1RgbaUnorm, 512, 512, 1).unwrap(),
            128 * 128 * 8,
        );
        // The full chain down to 4x4, which is where `collect_mips` stops for a
        // BC texture: 128^2 + 64^2 + ... + 1^2 = 21 845 blocks.
        assert_eq!(
            gpu_bytes(TextureFormat::Bc1RgbaUnorm, 512, 512, 8).unwrap(),
            21_845 * 8,
        );
        // BC3 carries an alpha block too, so 16 bytes per block.
        assert_eq!(
            gpu_bytes(TextureFormat::Bc3RgbaUnorm, 512, 512, 1).unwrap(),
            128 * 128 * 16,
        );
        // Uncompressed: 1x1 "blocks" of 4 bytes, and non-square works.
        assert_eq!(
            gpu_bytes(TextureFormat::Rgba8Unorm, 512, 256, 1).unwrap(),
            512 * 256 * 4,
        );
    }

    /// A block-compressed level smaller than one block still costs a whole
    /// block, and mip sizes clamp at 1 rather than reaching 0.
    ///
    /// `collect_mips` stops before these levels for TF2's power-of-two
    /// textures, but a packed community texture can reach them, and getting the
    /// rounding wrong here would make the check reject a *correct* chain — a
    /// false alarm being just as bad as a missed one.
    #[test]
    fn gpu_bytes_rounds_partial_blocks_up() {
        // 8x4 BC1: level 0 is 2x1 blocks, levels 1 (4x2) and 2 (2x1) are 1x1
        // block each. 4 blocks of 8 bytes.
        assert_eq!(gpu_bytes(TextureFormat::Bc1RgbaUnorm, 8, 4, 3).unwrap(), 32);
        // A 1x1 BC1 texture is one whole block.
        assert_eq!(gpu_bytes(TextureFormat::Bc1RgbaUnorm, 1, 1, 1).unwrap(), 8);
        // Enough levels to drive both axes past 1: they clamp, not wrap. Level
        // 0 is 2x1 texels and levels 1-3 are all 1x1, so five texels of four
        // bytes rather than the zero-sized levels a bare shift would give.
        assert_eq!(gpu_bytes(TextureFormat::Rgba8Unorm, 2, 1, 4).unwrap(), 5 * 4);
    }

    /// **The check must be able to fail.** A `SizeMismatch` is only worth having
    /// if a wrong byte count is actually rejected, so both directions are
    /// asserted against the size the descriptor implies.
    #[test]
    fn a_wrong_byte_count_is_rejected() {
        let format = TextureFormat::Bc1RgbaUnorm;
        let correct = gpu_bytes(format, 512, 512, 8).unwrap();

        // One level miscounted either way changes the expected size, which is
        // exactly the bug this guards: `collect_mips` returning a count that
        // does not match what it concatenated.
        assert_ne!(gpu_bytes(format, 512, 512, 7).unwrap(), correct);
        assert_ne!(gpu_bytes(format, 512, 512, 9).unwrap(), correct);
        // So does a wrong format for the same dimensions — BC1 against BC3 is
        // the pairing `passthrough_format` could plausibly get wrong.
        assert_ne!(
            gpu_bytes(TextureFormat::Bc3RgbaUnorm, 512, 512, 8).unwrap(),
            correct,
        );
        // And a wrong base size.
        assert_ne!(gpu_bytes(format, 256, 512, 8).unwrap(), correct);
    }

    #[test]
    fn the_common_formats_go_to_the_gpu_compressed() {
        // 99.7% of TF2's textures are these three.
        assert_eq!(
            passthrough_format(ImageFormat::Dxt1, true),
            Some(TextureFormat::Bc1RgbaUnormSrgb)
        );
        assert_eq!(
            passthrough_format(ImageFormat::Dxt3, true),
            Some(TextureFormat::Bc2RgbaUnormSrgb)
        );
        assert_eq!(
            passthrough_format(ImageFormat::Dxt5, true),
            Some(TextureFormat::Bc3RgbaUnormSrgb)
        );
        // A normal map takes the linear view of the same blocks.
        assert_eq!(
            passthrough_format(ImageFormat::Dxt5, false),
            Some(TextureFormat::Bc3RgbaUnorm)
        );
        // DXT1's one-bit-alpha variant is the same BC1 block layout.
        assert_eq!(
            passthrough_format(ImageFormat::Dxt1OneBitAlpha, true),
            Some(TextureFormat::Bc1RgbaUnormSrgb)
        );
    }

    #[test]
    fn formats_with_undefined_alpha_take_the_cpu_path() {
        // BGRX's fourth channel is padding; uploading it as alpha would make
        // the surface randomly transparent.
        assert_eq!(passthrough_format(ImageFormat::Bgrx8888, true), None);
        assert_eq!(passthrough_format(ImageFormat::Uvlx8888, true), None);
        // ...as do the formats wgpu has no equivalent for.
        assert_eq!(passthrough_format(ImageFormat::Bgr888, true), None);
        assert_eq!(passthrough_format(ImageFormat::Abgr8888, true), None);
        assert_eq!(passthrough_format(ImageFormat::Uv88, false), None);
        assert_eq!(passthrough_format(ImageFormat::Rgba16161616F, false), None);
    }

    #[test]
    fn straight_rgba_and_bgra_pass_through_in_both_colour_spaces() {
        assert_eq!(
            passthrough_format(ImageFormat::Bgra8888, true),
            Some(TextureFormat::Bgra8UnormSrgb)
        );
        assert_eq!(
            passthrough_format(ImageFormat::Rgba8888, false),
            Some(TextureFormat::Rgba8Unorm)
        );
    }

    #[test]
    fn source_params_is_sixteen_byte_aligned() {
        // WebGL2 rejects a uniform struct that is not; the padding field is
        // load-bearing, not decoration.
        assert_eq!(size_of::<SourceParams>(), 16);
    }
}
