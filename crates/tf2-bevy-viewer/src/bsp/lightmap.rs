//! Milestone 1.4: the lightmap atlas.
//!
//! This is the shortest module in the project and the longest task in the
//! roadmap, because the fork does the hard part. `compute_lightmap_atlas_rgb32f`
//! walks every face, decodes its RGBE patch, packs the patches into one atlas
//! and hands back a per-face pixel rect. What is left is normalising those rects
//! against the atlas size, which happens in `geometry.rs` where the UVs are
//! built, and getting the result onto the GPU.
//!
//! Upstream `vbsp` cannot do any of this — it never reads the lighting lump. See
//! the README.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use vbsp::{Bsp, Rect};
use qbsp::data::lighting::LightmapStyle;
use qbsp::mesh::lightmap::{ComputeLightmapSettings, DefaultLightmapPacker, PerStyleLightmapData};

/// Where each face's patch landed in the atlas, plus the atlas itself.
#[derive(Default)]
pub struct Lightmaps {
    pub image: Option<Handle<Image>>,
    pub size: UVec2,
    /// Keyed on the *global* face index — the position in `bsp.faces`, which is
    /// what `Handle<Model>::faces_with_id()` yields.
    pub rects: HashMap<u32, Rect>,
}

impl Lightmaps {
    /// This face's patch as a normalised rect in the atlas, or `None` if it has
    /// no lightmap. The packer reports a zero-size rect for those, which would
    /// otherwise collapse every vertex onto one texel.
    pub fn uv_rect(&self, face_id: u32) -> Option<(Vec2, Vec2)> {
        if self.image.is_none() || self.size.x == 0 || self.size.y == 0 {
            return None;
        }
        let rect = self.rects.get(&face_id)?;
        if rect.width == 0 || rect.height == 0 {
            return None;
        }
        let atlas = self.size.as_vec2();
        let min = UVec2::new(rect.x, rect.y).as_vec2() / atlas;
        let size = UVec2::new(rect.width, rect.height).as_vec2() / atlas;
        Some((min, size))
    }
}

#[derive(Default, Clone, Copy)]
pub struct LightmapStats {
    pub atlas: UVec2,
    pub bytes: usize,
    /// How many lightmap styles the map defines. Style 0 is the base bake;
    /// the others are switchable lights, which we do not composite yet.
    pub styles: usize,
    pub faces_with_patch: usize,
    pub faces_without: usize,
    /// Faces that had a switchable light composited onto the base bake. Zero on
    /// every non-Halloween map.
    pub faces_composited: usize,
    pub build_time: std::time::Duration,
    pub error: Option<&'static str>,
}

pub fn build(bsp: &Bsp, images: &mut Assets<Image>) -> (Lightmaps, LightmapStats) {
    let start = std::time::Instant::now();
    let mut stats = LightmapStats::default();

    // RGBA rather than RGB: the packer needs `From<DynamicImage>`, which only
    // the 4-channel float buffer implements. Convenient anyway, since wgpu has
    // no 3-channel float texture format.
    let packer = DefaultLightmapPacker::<PerStyleLightmapData<image::Rgba32FImage>>::new(
        ComputeLightmapSettings {
            // Faces with no lightmap collapse to a single reserved texel, and
            // gaps between patches get filled. White rather than the default
            // black, so an unlit face renders at full albedo instead of
            // disappearing, and so extrusion bleed brightens rather than darkens.
            default_color: [255; 3],
            no_lighting_color: [255; 3],
            special_lighting_color: [255; 3],
            // Padding around each patch. Without it, bilinear sampling at a
            // patch edge pulls in its neighbour -- the classic lightmap seam.
            extrusion: 2,
            ..default()
        },
    );

    let atlas = match bsp.compute_lightmap_atlas_rgb32f(packer) {
        Ok(atlas) => atlas,
        Err(_) => {
            stats.error = Some("no lightmap data in this bsp");
            stats.build_time = start.elapsed();
            return (Lightmaps::default(), stats);
        }
    };

    for rect in atlas.rects.values() {
        if rect.width == 0 || rect.height == 0 {
            stats.faces_without += 1;
        } else {
            stats.faces_with_patch += 1;
        }
    }

    let mut per_style = atlas.data.into_inner();
    stats.styles = per_style.len();

    let Some(mut base) = per_style.remove(&LightmapStyle(0)) else {
        stats.error = Some("atlas has no style 0");
        stats.build_time = start.elapsed();
        return (Lightmaps::default(), stats);
    };

    stats.faces_composited = composite_styles(bsp, &atlas.rects, &per_style, &mut base);
    let base = &base;

    let size = UVec2::new(base.width(), base.height());
    // Straight reinterpretation: the buffer is already RGBA f32 in the layout
    // `Rgba32Float` wants. Keeping it float matters -- Source stores RGBE and
    // the shared exponent routinely pushes values well past 1.0, so an 8-bit
    // target would clip every bright surface in the map.
    let data: Vec<u8> = base
        .as_raw()
        .iter()
        .flat_map(|channel| channel.to_le_bytes())
        .collect();
    stats.bytes = data.len();
    stats.atlas = size;

    let mut image = Image::new(
        Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba32Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    // Linear, and clamped: patches are addressed by baked-in atlas UVs, so
    // repeat would wrap one patch into another.
    image.sampler = ImageSampler::linear();

    stats.build_time = start.elapsed();
    (
        Lightmaps {
            image: Some(images.add(image)),
            size,
            rects: atlas.rects,
        },
        stats,
    )
}

/// Add the switchable lights back onto the base bake.
///
/// `dface_t.styles` is four slots: slot 0 is the always-on bake, and a non-255
/// value in slots 1..3 names a switchable light that also contributes. The
/// packer returns one atlas *per style*, all sharing a size and a packing, so
/// the base and every extra live at the same pixel rect for a given face.
///
/// **Not a pixel-wise sum of the atlases.** A style atlas is created filled with
/// `default_color`, which is white here so that unlit faces render at full
/// albedo -- so adding whole images together would add white across every region
/// the style does not touch and blow the map out. Instead this walks faces, and
/// for each one only reads the styles that face actually declares. Regions a
/// style does not cover are never sampled from it.
///
/// The assumption worth stating: this composites every switchable light as
/// **on**, because a static viewer has no entity I/O to tell it otherwise. A map
/// that ships lights "initially dark" will render brighter here than in game.
///
/// Returns the number of faces that got a contribution. On ctf_2fort that is
/// zero -- the map has one style and nothing switchable -- so this is dead
/// weight there and only earns itself on the Halloween maps, where
/// koth_harvest_event has 4,200 of 8,831 visible faces touched by one of twelve
/// switchable lights.
fn composite_styles(
    bsp: &Bsp,
    rects: &HashMap<u32, Rect>,
    extra: &HashMap<LightmapStyle, image::Rgba32FImage>,
    base: &mut image::Rgba32FImage,
) -> usize {
    if extra.is_empty() {
        return 0;
    }
    let mut composited = 0;
    for (face_id, rect) in rects {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let Some(face) = bsp.face(*face_id as usize) else {
            continue;
        };
        let mut touched = false;
        for style in face.styles.iter().skip(1).filter(|s| **s != 255) {
            let Some(src) = extra.get(&LightmapStyle(*style)) else {
                continue;
            };
            for y in rect.y..(rect.y + rect.height).min(base.height()) {
                for x in rect.x..(rect.x + rect.width).min(base.width()) {
                    let add = *src.get_pixel(x, y);
                    let dst = base.get_pixel_mut(x, y);
                    // Alpha is not a light channel; leave it be.
                    for c in 0..3 {
                        dst.0[c] += add.0[c];
                    }
                }
            }
            touched = true;
        }
        composited += touched as usize;
    }
    composited
}
