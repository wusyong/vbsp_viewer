//! Brush geometry and materials straight from the BSP, no glTF in the middle.
//!
//! Loading is synchronous in a startup system, and after 1.2 that still looks
//! like the right call: the whole map is 6 ms of geometry and ~120 ms of
//! materials. The `AssetLoader` and VPK-backed `AssetSource` the roadmap
//! describes would buy streaming and hot-reload, neither of which a viewer
//! needs yet — and `Vfs` already exposes the `load(path) -> Option<Vec<u8>>`
//! that an `AssetReader` would be built on, so that door stays open.

pub mod geometry;
pub mod lightmap;

use std::path::PathBuf;

use bevy::math::Affine2;
use bevy::pbr::Lightmap;
use bevy::prelude::*;
use bevy::render::render_resource::{Face as CullFace, WgpuFeatures};
use bevy::render::renderer::RenderDevice;

use crate::vfs::Vfs;

pub use geometry::{HAMMER_UNIT, REFERENCE_YAW, Stats};
pub use lightmap::LightmapStats;

/// Source bakes lightmaps against its own light units; Bevy's PBR pipeline
/// expects something else. There is no principled conversion between them, so
/// this is tuned by eye — `TF2_LIGHTMAP_EXPOSURE` overrides it, and `-`/`=`
/// adjust it live, which is how the default was found.
pub fn lightmap_exposure() -> f32 {
    std::env::var("TF2_LIGHTMAP_EXPOSURE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_LIGHTMAP_EXPOSURE)
}

/// Found by sweeping: 1 is dim but legible, 200 washes out, 4000 is pure white.
pub const DEFAULT_LIGHTMAP_EXPOSURE: f32 = 20.0;

/// Where to find the map. Defaults to a stock Windows Steam install; override
/// with `TF2_BSP=/path/to/map.bsp` for custom maps or a different library.
const DEFAULT_BSP: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\tf\maps\ctf_2fort.bsp";

pub fn bsp_path() -> PathBuf {
    std::env::var_os("TF2_BSP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_BSP))
}

/// Marks everything spawned by our own loader, so the A/B toggle can hide it in
/// one query.
#[derive(Component)]
pub struct BspGeometry;

/// Three materials per batch. `textured` is the real one; the other two are
/// diagnostics worth keeping — a face batched under the wrong texture is
/// obvious in debug colour and invisible under a texture, and `plain` isolates
/// geometry problems from material ones.
#[derive(Component)]
pub struct BatchMaterials {
    pub textured: Handle<StandardMaterial>,
    pub plain: Handle<StandardMaterial>,
    pub debug: Handle<StandardMaterial>,
}

/// Where a batch's texture came from, for the HUD.
#[derive(Default, Clone, Copy)]
pub struct MaterialStats {
    pub resolved: usize,
    /// VMT parsed but its `$basetexture` did not resolve to a VTF.
    pub missing_texture: usize,
    /// No VMT at all, or it failed to parse. These render magenta.
    pub failed: usize,
    pub water: usize,
    pub bc_uploaded: usize,
    pub rgba_uploaded: usize,
    pub bytes: usize,
    pub load_time: std::time::Duration,
}

#[derive(Resource, Default)]
pub struct BspReport {
    pub stats: Stats,
    pub materials: MaterialStats,
    pub lightmaps: LightmapStats,
    /// Texture names that did not resolve, for the HUD. Capped — with a broken
    /// search path this would otherwise be every material in the map.
    pub missing: Vec<String>,
    pub batches: usize,
    pub largest: Vec<(String, usize)>,
    /// Brush-entity classnames and how many models each owns. Worth having on
    /// screen: it is the direct evidence for whether the `*N` lookup is picking
    /// up more than the four classnames the reference tools hardcode.
    pub brush_entities: Vec<(String, usize)>,
    pub error: Option<String>,
}

pub struct BspPlugin;

impl Plugin for BspPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BspReport>()
            .add_systems(Startup, load_bsp);
    }
}

fn load_bsp(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    render_device: Option<Res<RenderDevice>>,
    mut report: ResMut<BspReport>,
) {
    // Passing DXT blocks straight through needs adapter support. Bevy requests
    // features with `WgpuSettingsPriority::Functionality`, so on any desktop GPU
    // this is enabled; the check keeps the fallback honest rather than trusting
    // that. Absent a device, decode -- wrong guess costs VRAM, not correctness.
    let supports_bc = render_device
        .map(|d| d.features().contains(WgpuFeatures::TEXTURE_COMPRESSION_BC))
        .unwrap_or(false);

    let path = bsp_path();
    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(e) => {
            report.error = Some(format!("{}: {e}", path.display()));
            error!("could not read {}: {e}", path.display());
            return;
        }
    };
    // vbsp handles the LZMA-compressed lumps TF2 ships. Worth knowing that it
    // does not expose the lighting lump at all, which is what blocks 1.4.
    let bsp = match vbsp::Bsp::read(&data) {
        Ok(bsp) => bsp,
        Err(e) => {
            report.error = Some(format!("{e}"));
            error!("could not parse {}: {e}", path.display());
            return;
        }
    };

    // Before geometry: the atlas decides each face's second UV set.
    let (lightmaps, lightmap_stats) = lightmap::build(&bsp, &mut images);
    if let Some(e) = lightmap_stats.error {
        warn!("no lightmaps: {e}");
    } else {
        info!(
            "lightmap atlas {}x{}, {} styles, {:.0} MB in {:?}",
            lightmap_stats.atlas.x,
            lightmap_stats.atlas.y,
            lightmap_stats.styles,
            lightmap_stats.bytes as f32 / (1024.0 * 1024.0),
            lightmap_stats.build_time,
        );
    }
    report.lightmaps = lightmap_stats;

    let (batches, stats) = geometry::build(&bsp, &lightmaps);
    info!(
        "built {} batches, {} triangles from {} faces in {:?}",
        batches.len(),
        stats.triangles,
        stats.faces_drawn,
        stats.build_time
    );

    let mut by_class: std::collections::HashMap<String, usize> = default();
    for model in geometry::brush_models(&bsp) {
        *by_class.entry(model.classname).or_default() += 1;
    }
    report.brush_entities = by_class.into_iter().collect();
    report.brush_entities.sort_by_key(|e| std::cmp::Reverse(e.1));

    report.stats = stats;
    report.batches = batches.len();
    report.largest = batches
        .iter()
        .take(5)
        .map(|b| (b.texture.clone(), b.triangles))
        .collect();

    let root = commands
        .spawn((
            BspGeometry,
            Transform::from_rotation(Quat::from_rotation_y(REFERENCE_YAW)),
            Visibility::default(),
        ))
        .id();

    // The map's own pakfile goes at the front of the search path, ahead of the
    // install -- that is how a custom map overrides stock content.
    let (vfs, vfs_error) = Vfs::new(Some(bsp.pack.clone()));
    if let Some(e) = &vfs_error {
        warn!("{e}; falling back to whatever the map's pakfile carries");
        report.error.get_or_insert_with(|| e.clone());
    }

    let started = std::time::Instant::now();
    let mut mats = MaterialStats::default();

    for batch in batches {
        let textured = build_material(
            &vfs,
            &batch.texture,
            supports_bc,
            &mut images,
            &mut mats,
            &mut report.missing,
        );
        let plain = materials.add(StandardMaterial {
            base_color: Color::srgb(0.72, 0.72, 0.72),
            perceptual_roughness: 1.0,
            metallic: 0.0,
            ..default()
        });
        let debug = materials.add(StandardMaterial {
            base_color: batch.debug_color,
            unlit: true,
            ..default()
        });
        let textured = materials.add(textured);
        let mut entity = commands.spawn((
            ChildOf(root),
            BspGeometry,
            Mesh3d(meshes.add(batch.mesh)),
            MeshMaterial3d(textured.clone()),
            BatchMaterials {
                textured,
                plain,
                debug,
            },
        ));
        if let Some(image) = &lightmaps.image {
            entity.insert(Lightmap {
                image: image.clone(),
                // Full atlas: the per-face rects are already folded into UV_1,
                // because this field is per-entity and a batch holds many faces.
                uv_rect: Rect::new(0.0, 0.0, 1.0, 1.0),
                // Bicubic wants a linear sampler, which the atlas has. It is
                // smoother across the low-resolution patches, and the light
                // leaks it can cause are held off by the packer's extrusion.
                bicubic_sampling: true,
            });
        }
    }

    mats.load_time = started.elapsed();
    info!(
        "resolved {} materials ({} missing texture, {} failed) in {:?}",
        mats.resolved, mats.missing_texture, mats.failed, mats.load_time
    );
    report.materials = mats;
}

/// Resolve one texture name to a `StandardMaterial`.
///
/// These were `unlit` through 1.2, when there was nothing to light them with.
/// Now the lightmap supplies indirect diffuse, so they must be lit or Bevy
/// ignores the `Lightmap` component entirely — and the failure mode is "nothing
/// changed on screen" rather than an error, which is worth knowing about.
fn build_material(
    vfs: &Vfs,
    name: &str,
    supports_bc: bool,
    images: &mut Assets<Image>,
    stats: &mut MaterialStats,
    missing: &mut Vec<String>,
) -> StandardMaterial {
    /// Enough to see the shape of a problem without the HUD becoming the map.
    const MAX_REPORTED: usize = 12;

    let magenta = |stats: &mut MaterialStats| {
        stats.failed += 1;
        StandardMaterial {
            base_color: Color::srgb(1.0, 0.0, 1.0),
            unlit: true,
            ..default()
        }
    };

    let def = match crate::vmt::load(vfs, name) {
        Ok(def) => def,
        Err(e) => {
            if missing.len() < MAX_REPORTED {
                missing.push(format!("{name}: {e}"));
            }
            return magenta(stats);
        }
    };

    if def.is_water {
        stats.water += 1;
        return StandardMaterial {
            base_color: Color::srgba(0.32, 0.71, 0.85, 0.5),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        };
    }

    let Some(texture) = def.base_texture.as_deref() else {
        stats.missing_texture += 1;
        return magenta(stats);
    };
    let path = format!("materials/{}.vtf", texture.trim_end_matches(".vtf"));
    let Some(raw) = vfs.load(&path) else {
        stats.missing_texture += 1;
        if missing.len() < MAX_REPORTED {
            missing.push(format!("{name}: no {path}"));
        }
        return magenta(stats);
    };
    let image = match crate::vtf::to_image(&raw, supports_bc) {
        Ok(image) => image,
        Err(e) => {
            if missing.len() < MAX_REPORTED {
                missing.push(format!("{path}: {e}"));
            }
            return magenta(stats);
        }
    };

    if image.texture_descriptor.format.is_compressed() {
        stats.bc_uploaded += 1;
    } else {
        stats.rgba_uploaded += 1;
    }
    stats.bytes += image.data.as_ref().map(Vec::len).unwrap_or(0);
    stats.resolved += 1;

    StandardMaterial {
        base_color_texture: Some(images.add(image)),
        // `$translucent` blends; `$alphatest` cuts out. Getting this wrong makes
        // every grate and fence either a solid slab or invisible.
        alpha_mode: match (def.translucent, def.alpha_test) {
            (true, _) => AlphaMode::Blend,
            (false, Some(cutoff)) => AlphaMode::Mask(cutoff),
            _ => AlphaMode::Opaque,
        },
        double_sided: def.no_cull,
        cull_mode: (!def.no_cull).then_some(CullFace::Back),
        // Fully rough, non-metallic: Source's diffuse lightmap carries all the
        // shading, so any specular response here is invention.
        perceptual_roughness: 1.0,
        metallic: 0.0,
        // Source bakes at a different scale than Bevy's units; this is the knob
        // for overall brightness and is tuned by eye against the game.
        lightmap_exposure: lightmap_exposure(),
        unlit: false,
        uv_transform: def.transform.as_ref().map_or(Affine2::IDENTITY, |t| {
            // `$basetexturetransform` rotates and scales about a centre, so the
            // centre has to be moved to the origin and back around it.
            let center = Vec2::from(t.center);
            Affine2::from_translation(center)
                * Affine2::from_scale_angle_translation(
                    Vec2::from(t.scale),
                    t.rotate.to_radians(),
                    Vec2::from(t.translate),
                )
                * Affine2::from_translation(-center)
        }),
        ..default()
    }
}

/// Backface culling starts off, per the roadmap's advice: get something on
/// screen first, then turn culling on and see whether it survives.
pub fn set_culling(
    materials: &mut Assets<StandardMaterial>,
    handles: &[Handle<StandardMaterial>],
    on: bool,
) {
    for handle in handles {
        if let Some(mut material) = materials.get_mut(handle) {
            material.cull_mode = on.then_some(CullFace::Back);
            material.double_sided = !on;
        }
    }
}
