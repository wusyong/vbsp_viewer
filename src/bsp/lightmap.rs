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

    let per_style = atlas.data.into_inner();
    stats.styles = per_style.len();

    // Style 0 is the base bake. The rest are switchable lights, which need a
    // per-style atlas composited at runtime -- deferred, and not something the
    // other Source viewers do either.
    let Some(base) = per_style.get(&LightmapStyle(0)) else {
        stats.error = Some("atlas has no style 0");
        stats.build_time = start.elapsed();
        return (Lightmaps::default(), stats);
    };

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
