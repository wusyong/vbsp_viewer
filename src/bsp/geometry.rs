//! Turning BSP faces into Bevy meshes.
//!
//! Deliberately free of Bevy world access so it can be tested and timed on its
//! own: in, a parsed `Bsp`; out, one mesh per distinct texture plus the counters
//! the HUD needs to prove the conversion was faithful.
//!
//! Structure follows `vbspview`'s `bsp.rs` (group faces by texture, one mesh per
//! group) rather than `vbsp-to-gltf`'s (one primitive per face). Three
//! deliberate departures from both, argued at their call sites: brush entities
//! are found by their `*N` model reference instead of by matching a fixed list
//! of classnames, normals come from the face's plane rather than from
//! recomputing them off the triangles, and `dface_t.side` is ignored.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use vbsp::{Bsp, Face, Handle, TextureInfo};

use super::lightmap::Lightmaps;

/// vbsp's glam, which is 0.30 against bevy's 0.32 — a different crate version,
/// so a different `Vec3`. Aliased so the conversion boundary is visible rather
/// than a confusing "expected Vec3, found Vec3".
use glam::Vec3 as SourceVec3;

/// 1 Hammer unit = 1 inch. This puts 2fort at ~120 m end to end and a doorway at
/// human height, which is the check that matters. Both of icewind's tools use
/// 1.905 cm instead; that number makes the player 1.37 m tall.
pub const HAMMER_UNIT: f32 = 0.0254;

/// `vbsp-to-gltf` bakes a 90° yaw into the glb's root node. It means nothing,
/// but matching it keeps the A/B toggle against the reference aligned.
pub const REFERENCE_YAW: f32 = std::f32::consts::FRAC_PI_2;

/// Source is Z-up right-handed; Bevy is Y-up right-handed. `(x, y, z) → (y, z, x)`
/// is a cyclic permutation, so it preserves handedness and leaves triangle
/// winding alone. The roadmap's `(x, z, -y)` mirrors instead, which is why that
/// route then needs every index list reversed to compensate.
#[inline]
pub fn to_bevy(v: SourceVec3) -> Vec3 {
    Vec3::new(v.y, v.z, v.x) * HAMMER_UNIT
}

/// Same permutation without the unit scale, for directions.
#[inline]
fn dir_to_bevy(v: SourceVec3) -> Vec3 {
    Vec3::new(v.y, v.z, v.x)
}

/// A brush model and where it belongs. Model 0 is worldspawn; 1..n are owned by
/// brush entities.
pub struct BrushModel {
    pub index: usize,
    pub origin: SourceVec3,
    pub classname: String,
}

/// Find every brush model the entity lump actually references.
///
/// Both `vbspview` and `vbsp-to-gltf` match a fixed list of four classnames
/// (`func_brush`, `func_illusionary`, `func_wall`, `func_wall_toggle`), which on
/// ctf_2fort silently drops 84 of the map's 148 models -- the spawn doors and
/// resupply cabinets among them. Keying on the `*N` model reference instead
/// picks up every brush entity regardless of what it is, which is both simpler
/// and more complete.
pub fn brush_models(bsp: &Bsp) -> Vec<BrushModel> {
    let mut models = vec![BrushModel {
        index: 0,
        origin: SourceVec3::ZERO,
        classname: "worldspawn".into(),
    }];

    for entity in bsp.entities.iter() {
        let Some(model) = entity.prop("model") else {
            continue;
        };
        // Brush entities reference `*3`; point entities reference a .mdl path.
        let Some(index) = model.strip_prefix('*').and_then(|n| n.parse::<usize>().ok()) else {
            continue;
        };
        if index == 0 || index >= bsp.models.len() {
            continue;
        }
        models.push(BrushModel {
            index,
            // Most brush entities are already in world space; a few carry an
            // origin, so honour it where present. The fork dropped its `Vector`
            // type, but the blanket `[T; N]` impl parses the same "x y z".
            origin: entity
                .prop_parse::<[f32; 3]>("origin")
                .and_then(Result::ok)
                .map_or(SourceVec3::ZERO, SourceVec3::from_array),
            classname: entity.prop("classname").unwrap_or("?").to_string(),
        });
    }

    models
}

/// One mesh per distinct texture. A TF2 map has thousands of faces and a few
/// hundred textures; one entity per face would tank frame time for nothing.
pub struct Batch {
    pub texture: String,
    pub debug_color: Color,
    pub mesh: Mesh,
    pub triangles: usize,
}

#[derive(Default, Clone, Copy)]
pub struct Stats {
    /// Brush models reachable from worldspawn plus the entity lump.
    pub models_used: usize,
    /// Models present in the lump that nothing references. Non-zero is not
    /// necessarily a bug, but it is worth watching.
    pub models_orphaned: usize,
    pub faces_drawn: usize,
    /// Faces rejected by `is_visible()`: NODRAW, SKIP, HINT, TRIGGER and SKY.
    pub faces_skipped: usize,
    pub faces_displaced: usize,
    /// Faces that got a real lightmap patch. The rest sample the atlas's neutral
    /// texel and render unlit.
    pub faces_lit: usize,
    pub triangles: usize,
    pub vertices: usize,
    /// Faces whose plane normal disagrees with the normal implied by their
    /// winding. Should be zero; if it isn't, the normal rule, the fan order or
    /// the coordinate mapping is wrong. On ctf_2fort this sits at 49 of 14879.
    pub normal_mismatches: usize,
    /// Of those, the ones that are unambiguously backwards rather than just
    /// numerically noisy. This is the number that must be zero.
    pub normal_mismatches_strong: usize,
    /// How many faces set `side`. Informational, and a standing reminder of why
    /// it must be ignored: see `push_face`.
    pub faces_side_set: usize,
    pub build_time: Duration,
}

#[derive(Default)]
struct Accum {
    texture: String,
    debug_color: [u8; 3],
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    /// Second UV set, already remapped into atlas space. Bevy's `Lightmap`
    /// component carries a `uv_rect` for exactly this job, but it is per-entity
    /// and we batch hundreds of faces into one entity, so the remap is baked in
    /// here instead and `uv_rect` stays the full 0..1.
    lightmap_uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

pub fn build(bsp: &Bsp, lightmaps: &Lightmaps) -> (Vec<Batch>, Stats) {
    let start = Instant::now();
    let mut stats = Stats::default();

    // Keyed on texture_data_index rather than the texture name vbspview groups
    // by: same grouping, since the name is looked up through texture_data, but
    // an integer key avoids hashing a string per face.
    let mut accums: HashMap<i32, Accum> = HashMap::new();

    let placements = brush_models(bsp);
    stats.models_used = placements.len();
    stats.models_orphaned = bsp.models.len().saturating_sub(placements.len());

    for placement in &placements {
        let Some(model) = bsp.models().nth(placement.index) else {
            continue;
        };
        // `faces_with_id` rather than `faces`: the lightmap rects are keyed on
        // the global face index, which is what the id is.
        for (face_id, face) in model.faces_with_id() {
            if !face.is_visible() {
                stats.faces_skipped += 1;
                continue;
            }
            let texture = face.texture();
            let accum = accums
                .entry(texture.texture_data_index)
                .or_insert_with(|| Accum {
                    texture: texture.name().to_string(),
                    debug_color: texture.debug_color(),
                    ..default()
                });
            let patch = lightmaps.uv_rect(face_id);
            if patch.is_some() {
                stats.faces_lit += 1;
            }
            push_face(accum, &face, &texture, placement.origin, patch, &mut stats);
            stats.faces_drawn += 1;
        }
    }

    let mut batches: Vec<Batch> = accums
        .into_values()
        .map(|a| {
            stats.triangles += a.indices.len() / 3;
            stats.vertices += a.positions.len();
            let triangles = a.indices.len() / 3;
            let [r, g, b] = a.debug_color;
            Batch {
                texture: a.texture,
                debug_color: Color::srgb_u8(r, g, b),
                triangles,
                mesh: Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
                    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, a.positions)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, a.normals)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, a.uvs)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, a.lightmap_uvs)
                    .with_inserted_indices(Indices::U32(a.indices)),
            }
        })
        .collect();
    batches.sort_by_key(|b| std::cmp::Reverse(b.triangles));

    stats.build_time = start.elapsed();
    (batches, stats)
}


/// Append one face to its batch.
///
/// This used to be two paths -- a polygon fan for brush faces and a
/// pre-triangulated soup for displacements. The vbsp fork unified them:
/// `vertex_positions()` yields the face's own vertices either way (polygon
/// corners, or the displaced grid) and `triangulate_indices()` yields indices
/// into exactly that list, dispatching on displacement internally. So there is
/// one path now, and it is indexed rather than a soup.
fn push_face(
    accum: &mut Accum,
    face: &Handle<Face>,
    texture: &Handle<TextureInfo>,
    origin: SourceVec3,
    patch: Option<(Vec2, Vec2)>,
    stats: &mut Stats,
) {
    let base = accum.positions.len() as u32;
    let displaced = face.displacement().is_some();
    if displaced {
        stats.faces_displaced += 1;
    }

    let first = accum.positions.len();
    for pos in face.vertex_positions() {
        // UVs must be computed in Source space -- the texture vectors are a dot
        // product against the unconverted position. Convert after.
        accum.uvs.push(texture.uv(pos).to_array());
        accum.positions.push(to_bevy(pos + origin).to_array());
    }
    // `lightmap_uvs` runs 0..1 across this face's own patch; scale into the
    // patch's slot in the atlas. A face with no patch is parked at the atlas
    // origin, which the packer fills with the neutral colour, so it renders at
    // full albedo rather than black.
    match patch {
        Some((min, size)) => accum.lightmap_uvs.extend(
            face.lightmap_uvs()
                .map(|uv| (min + size * Vec2::new(uv.x, uv.y)).to_array()),
        ),
        None => accum
            .lightmap_uvs
            .resize(accum.positions.len(), [0.0, 0.0]),
    }

    if accum.positions.len() - first < 3 {
        accum.positions.truncate(first);
        accum.uvs.truncate(first);
        accum.lightmap_uvs.truncate(first);
        return;
    }
    // A displacement's lightmap grid and its vertex grid can disagree by a row
    // in malformed maps; pad rather than desync the attribute arrays.
    accum
        .lightmap_uvs
        .resize(accum.positions.len(), [0.0, 0.0]);

    let index_start = accum.indices.len();
    accum
        .indices
        .extend(face.triangulate_indices().map(|i| base + i as u32));

    // Taking the normal from the face's plane is exact and free; vbspview
    // recomputes normals from the triangles instead, which costs a pass and
    // loses nothing here.
    //
    // Do NOT negate this when `dface_t.side` is set, however tempting the field
    // name is. `side` records which side of the plane the face sits on for the
    // BSP tree's benefit; the surfedge walk is already wound to the face's true
    // facing, so the plane normal needs no correction. Measured on ctf_2fort:
    // flipping on `side` disagreed with the winding on 5508 of 14879 faces,
    // leaving it alone disagreed on 49.
    let plane_normal = dir_to_bevy(face.normal()).normalize_or_zero();
    if face.side != 0 {
        stats.faces_side_set += 1;
    }

    if displaced {
        // The plane normal belongs to the flat base quad and is wrong the moment
        // the surface is displaced, so derive one per triangle. Vertices are
        // shared across the displacement grid, so accumulate and normalise
        // rather than assigning -- that also smooths the terrain for free.
        accum.normals.resize(accum.positions.len(), [0.0; 3]);
        for tri in accum.indices[index_start..].as_chunks::<3>().0 {
            let [a, b, c] = [
                Vec3::from(accum.positions[tri[0] as usize]),
                Vec3::from(accum.positions[tri[1] as usize]),
                Vec3::from(accum.positions[tri[2] as usize]),
            ];
            // Un-normalised, so larger triangles weigh more.
            let face_normal = (b - a).cross(c - a);
            for &i in tri {
                let n = &mut accum.normals[i as usize];
                *n = (Vec3::from(*n) + face_normal).to_array();
            }
        }
        for i in first..accum.positions.len() {
            accum.normals[i] = Vec3::from(accum.normals[i]).normalize_or_zero().to_array();
        }
        return;
    }

    accum
        .normals
        .resize(accum.positions.len(), plane_normal.to_array());

    // Cross-check the plane normal against the winding of the first emitted
    // triangle. This is the cheapest way to catch a wrong coordinate mapping, a
    // wrong fan order or a wrong normal rule, and it is what caught the `side`
    // mistake above.
    let Some(tri) = accum.indices.get(index_start..index_start + 3) else {
        return;
    };
    let [a, b, c] = [
        Vec3::from(accum.positions[tri[0] as usize]),
        Vec3::from(accum.positions[tri[1] as usize]),
        Vec3::from(accum.positions[tri[2] as usize]),
    ];
    let agreement = (b - a).cross(c - a).normalize_or_zero().dot(plane_normal);
    if agreement < 0.0 {
        stats.normal_mismatches += 1;
        // Split the residual: a genuinely backwards face reads about -1, while a
        // sliver whose first triangle is nearly degenerate reads near 0 and
        // means nothing.
        if agreement < -0.5 {
            stats.normal_mismatches_strong += 1;
        }
    }
}
