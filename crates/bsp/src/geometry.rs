//! Turn BSP faces into renderable mesh data.
//!
//! Output is plain `Vec`s in **Source space** — Z-up, unscaled world units,
//! and Source's winding. No engine types appear here; converting to a Bevy
//! `Mesh` (and to Bevy's Y-up basis) is `bevy_bsp`'s job. Keeping the two
//! apart is what lets later phases reuse this for collision and physics.
//!
//! # What is built
//!
//! [`build_model`] takes a model index: model 0 is worldspawn, and 1.. are the
//! brush entities referenced by an entity's `model "*N"` key. Callers position
//! non-zero models themselves — this module never reads the ENTITIES lump, so
//! it needs no KeyValues parser.
//!
//! Faces are grouped by `texdata` index, one [`Surface`] per material, because
//! the alternative is a draw call per face: `cp_badlands` has 13 483 world
//! faces across 151 materials.
//!
//! Displacement faces (`dispinfo >= 0`) are counted and skipped here; the
//! displacement tessellator builds those separately.

use crate::flags;
use crate::raw::{DFace, DTexData, TexInfo};
use crate::{Bsp, BspError, LumpId, Result};
use std::collections::HashMap;

/// Triangles whose two edge vectors cross to less than this are dropped as
/// degenerate.
///
/// On real maps these come out *exactly* zero rather than merely small — see
/// [`BuildStats::degenerate_triangles`] — with a clean gap up to the smallest
/// genuine sliver, so the exact value only has to be small enough not to eat
/// real geometry. Source units are inches, making this a far smaller area than
/// any face a mapper could author.
const DEGENERATE_CROSS_EPSILON: f32 = 1e-4;

/// One vertex of built geometry, in Source space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    /// World position, Source units, Z-up.
    pub position: [f32; 3],
    /// Face normal (flat shading). Source also ships smooth per-vertex normals
    /// in VERTNORMALS/VERTNORMALINDICES for phong materials; unused so far.
    pub normal: [f32; 3],
    /// Texture UV, already divided by the material's source dimensions, so it
    /// tiles outside 0..1 as Source intends.
    pub uv: [f32; 2],
    /// Lightmap UV normalised to this face's own sample grid. The atlas packer
    /// rescales these into its rect; see [`Surface::chunks`].
    pub lightmap_uv: [f32; 2],
    /// Displacement blend weight for `$basetexture2`. Always 0 on brush faces.
    pub alpha: f32,
}

/// The span of one BSP face inside a [`Surface`]'s buffers.
///
/// This is the provenance the lightmap atlas needs: it packs per-face rects,
/// then remaps [`Vertex::lightmap_uv`] for exactly these vertices.
#[derive(Clone, Copy, Debug)]
pub struct FaceChunk {
    /// Index into the FACES lump.
    pub face: u32,
    pub first_vertex: u32,
    pub vertex_count: u32,
}

/// All geometry in one model sharing a single material.
#[derive(Clone, Debug)]
pub struct Surface {
    /// Index into the TEXDATA lump; resolve a name with [`Bsp::texture_name`].
    pub texdata: i32,
    /// `SURF_*` flags of the first face contributing here. Faces sharing a
    /// material almost always share flags; the renderer only needs the shader
    /// hints (`SURF_TRANS`, `SURF_WARP`).
    pub flags: i32,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Per-face spans, in the order they were appended.
    pub chunks: Vec<FaceChunk>,
}

impl Surface {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Why a face contributed no geometry. Tracked because a silent skip and a
/// misparse look identical in a render.
#[derive(Clone, Copy, Debug, Default)]
pub struct SkipCounts {
    /// `texinfo < 0` — no material, nothing to draw.
    pub no_texinfo: usize,
    /// Tool brushes and sky placeholders, per [`flags::SURF_SKIP_RENDER`].
    pub tool_surface: usize,
    /// `dispinfo >= 0`; built by the displacement tessellator instead.
    pub displacement: usize,
    /// Fewer than three edges, so not a polygon.
    pub too_few_edges: usize,
    /// A surfedge or edge index pointed outside its lump.
    pub bad_index: usize,
}

/// Counters for the build, for the debug HUD and the all-maps sweep.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuildStats {
    pub faces_total: usize,
    pub faces_built: usize,
    pub skipped: SkipCounts,
    pub vertices: usize,
    pub triangles: usize,
    /// Zero-area fan triangles dropped.
    ///
    /// **This is normal map content, not an error** — around 10% of fan
    /// triangles on a shipped TF2 map. vbsp inserts extra vertices along
    /// straight edges to close t-junctions against neighbouring faces, so a
    /// face is often a triangle or quad carrying a long collinear run. Any fan
    /// triangle whose two other vertices sit on a line through vertex 0 then
    /// has exactly zero area, and dropping it loses no surface: `cp_badlands`
    /// has one 21-vertex face that is really a triangle with 18 collinear
    /// points on one edge.
    ///
    /// They are dropped rather than kept because they render nothing and would
    /// produce NaN tangents in M8.
    pub degenerate_triangles: usize,
}

impl BuildStats {
    pub fn faces_skipped(&self) -> usize {
        let s = &self.skipped;
        s.no_texinfo + s.tool_surface + s.displacement + s.too_few_edges + s.bad_index
    }
}

/// Groups geometry by material, keeping first-seen order rather than hash
/// order so output is deterministic across runs.
pub(crate) struct SurfaceSet {
    by_texdata: HashMap<i32, usize>,
    surfaces: Vec<Surface>,
}

impl SurfaceSet {
    pub(crate) fn new() -> Self {
        Self {
            by_texdata: HashMap::new(),
            surfaces: Vec::new(),
        }
    }

    /// The surface accumulating this material, creating it on first use.
    pub(crate) fn surface_for(&mut self, texdata: i32, flags: i32) -> &mut Surface {
        let slot = *self.by_texdata.entry(texdata).or_insert_with(|| {
            self.surfaces.push(Surface {
                texdata,
                flags,
                vertices: Vec::new(),
                indices: Vec::new(),
                chunks: Vec::new(),
            });
            self.surfaces.len() - 1
        });
        &mut self.surfaces[slot]
    }

    pub(crate) fn into_surfaces(self) -> Vec<Surface> {
        self.surfaces
    }
}

/// Built geometry for one BSP model.
#[derive(Clone, Debug)]
pub struct ModelGeometry {
    /// Index into the MODELS lump.
    pub model: usize,
    /// One entry per material, in first-seen order.
    pub surfaces: Vec<Surface>,
    /// The model's own bounds from the MODELS lump, Source units.
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// Where the model's brushes are anchored; brush entities are placed at
    /// their entity `origin` relative to this.
    pub origin: [f32; 3],
    pub stats: BuildStats,
}

impl ModelGeometry {
    pub fn vertex_count(&self) -> usize {
        self.surfaces.iter().map(|s| s.vertices.len()).sum()
    }

    pub fn triangle_count(&self) -> usize {
        self.surfaces.iter().map(Surface::triangle_count).sum()
    }
}

/// Build worldspawn (model 0) — the static level geometry.
pub fn build_worldspawn(bsp: &Bsp) -> Result<ModelGeometry> {
    build_model(bsp, 0)
}

/// Build one model's brush faces.
///
/// Fails only on a malformed lump; individual unusable faces are skipped and
/// counted in [`BuildStats::skipped`] rather than aborting the map.
pub fn build_model(bsp: &Bsp, model_index: usize) -> Result<ModelGeometry> {
    let models = bsp.models()?;
    let model = *models
        .get(model_index)
        .ok_or_else(|| BspError::IndexOutOfRange {
            lump: LumpId::Models.name(),
            index: model_index as i64,
            len: models.len(),
        })?;

    let positions = bsp.vertices()?;
    let edges = bsp.edges()?;
    let surfedges = bsp.surfedges()?;
    let faces = bsp.faces()?;
    let planes = bsp.planes()?;
    let texinfos = bsp.texinfos()?;
    let texdatas = bsp.texdatas()?;

    let first = model.firstface.max(0) as usize;
    let count = model.numfaces.max(0) as usize;
    let end = first.saturating_add(count).min(faces.len());

    let mut stats = BuildStats {
        faces_total: end.saturating_sub(first),
        ..Default::default()
    };

    let mut surfaces = SurfaceSet::new();
    let mut ring: Vec<[f32; 3]> = Vec::with_capacity(16);

    for (face_index, face) in faces[first..end].iter().enumerate() {
        let face_index = (first + face_index) as u32;

        if face.dispinfo >= 0 {
            stats.skipped.displacement += 1;
            continue;
        }
        let Some(texinfo) = usize::try_from(face.texinfo)
            .ok()
            .and_then(|i| texinfos.get(i))
        else {
            stats.skipped.no_texinfo += 1;
            continue;
        };
        if texinfo.flags & flags::SURF_SKIP_RENDER != 0 {
            stats.skipped.tool_surface += 1;
            continue;
        }
        if face.numedges < 3 {
            stats.skipped.too_few_edges += 1;
            continue;
        }
        let Some(texdata) = usize::try_from(texinfo.texdata)
            .ok()
            .and_then(|i| texdatas.get(i))
        else {
            stats.skipped.no_texinfo += 1;
            continue;
        };

        // Walk the face's edge ring. A surfedge is a *signed* index into
        // EDGES: negative means traverse that edge backwards, which is how
        // Source keeps a consistent winding without duplicating edges.
        ring.clear();
        let mut bad_index = false;
        for i in 0..face.numedges as usize {
            let Some(&surfedge) = surfedges.get(face.firstedge as usize + i) else {
                bad_index = true;
                break;
            };
            let (edge_index, end) = if surfedge >= 0 {
                (surfedge as usize, 0)
            } else {
                (surfedge.unsigned_abs() as usize, 1)
            };
            let Some(vertex) = edges
                .get(edge_index)
                .and_then(|e| positions.get(e.v[end] as usize))
            else {
                bad_index = true;
                break;
            };
            ring.push(vertex.point);
        }
        if bad_index || ring.len() < 3 {
            stats.skipped.bad_index += 1;
            continue;
        }

        // `planes[planenum]` is already the correctly-oriented plane: vbsp
        // stores planes in negated pairs and writes the face's own index, so
        // `df->side = f->planenum & 1` is parity bookkeeping, not an
        // instruction to flip (`utils/vbsp/writebsp.cpp:465`). Flipping on
        // `side` inverts the odd-numbered planes — 297 of Badlands' 1191
        // displacement parents alone.
        let normal = match planes.get(face.planenum as usize) {
            Some(plane) => plane.normal,
            None => {
                stats.skipped.bad_index += 1;
                continue;
            }
        };

        let surface = surfaces.surface_for(texinfo.texdata, texinfo.flags);

        let base = surface.vertices.len() as u32;
        for &position in &ring {
            surface.vertices.push(Vertex {
                position,
                normal,
                uv: texture_uv(position, texinfo, texdata),
                lightmap_uv: lightmap_uv(position, texinfo, face),
                alpha: 0.0,
            });
        }

        // Source faces are convex, so a fan from vertex 0 is a valid
        // triangulation and needs no ear clipping. Collinear t-junction
        // vertices make some of these exactly zero-area; that is expected and
        // costs no surface area, since such triangles lie on a line through
        // the apex.
        //
        for i in 1..ring.len() as u32 - 1 {
            // Reversed: a BSP face's edge ring runs *clockwise* seen from the
            // front, verified against a Badlands floor whose plane is exactly
            // +Z and whose ring goes +Y, +X, -Y, -X. wgpu treats
            // counter-clockwise as front-facing, so emitting the ring order
            // directly would make every surface a backface.
            let tri = [base, base + i + 1, base + i];
            if is_degenerate(&ring, i as usize) {
                stats.degenerate_triangles += 1;
                continue;
            }
            surface.indices.extend_from_slice(&tri);
        }

        surface.chunks.push(FaceChunk {
            face: face_index,
            first_vertex: base,
            vertex_count: ring.len() as u32,
        });

        stats.faces_built += 1;
    }

    let surfaces = surfaces.into_surfaces();
    stats.vertices = surfaces.iter().map(|s| s.vertices.len()).sum();
    stats.triangles = surfaces.iter().map(Surface::triangle_count).sum();

    Ok(ModelGeometry {
        model: model_index,
        surfaces,
        mins: model.mins,
        maxs: model.maxs,
        origin: model.origin,
        stats,
    })
}

/// Texture UV for a world position.
///
/// `textureVecsTexelsPerWorldUnits` is a pair of affine planes giving texel
/// coordinates; dividing by the material's source size normalises to 0..1 per
/// tile. Values outside 0..1 are normal and must wrap, not clamp.
pub(crate) fn texture_uv(position: [f32; 3], texinfo: &TexInfo, texdata: &DTexData) -> [f32; 2] {
    // A zero here would mean a broken TEXDATA entry; 1 keeps the UV finite so
    // one bad material cannot poison the whole mesh.
    let width = if texdata.width != 0 {
        texdata.width as f32
    } else {
        1.0
    };
    let height = if texdata.height != 0 {
        texdata.height as f32
    } else {
        1.0
    };
    [
        affine(position, texinfo.texture_vecs[0]) / width,
        affine(position, texinfo.texture_vecs[1]) / height,
    ]
}

/// Lightmap UV for a world position, normalised to this face's sample grid.
///
/// The luxel coordinate comes from `lightmapVecsLuxelsPerWorldUnits`, shifted
/// by the face's luxel origin. The grid is one larger than
/// `m_LightmapTextureSizeInLuxels` on each axis, and the half-luxel offset
/// puts samples at texel centres rather than corners.
pub(crate) fn lightmap_uv(position: [f32; 3], texinfo: &TexInfo, face: &DFace) -> [f32; 2] {
    let mins = face.lightmap_texture_mins_in_luxels;
    let (w, h) = (face.lightmap_width() as f32, face.lightmap_height() as f32);
    let lu = affine(position, texinfo.lightmap_vecs[0]) - mins[0] as f32;
    let lv = affine(position, texinfo.lightmap_vecs[1]) - mins[1] as f32;
    [(lu + 0.5) / w, (lv + 0.5) / h]
}

/// Evaluate one of Source's `[x, y, z, offset]` affine texture planes.
#[inline]
pub(crate) fn affine(p: [f32; 3], v: [f32; 4]) -> f32 {
    p[0] * v[0] + p[1] * v[1] + p[2] * v[2] + v[3]
}

/// Whether fan triangle `(0, i, i + 1)` of `ring` has no area worth drawing.
fn is_degenerate(ring: &[[f32; 3]], i: usize) -> bool {
    let (a, b, c) = (ring[0], ring[i], ring[i + 1]);
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let cross = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let len_sq = cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2];
    !len_sq.is_finite() || len_sq < DEGENERATE_CROSS_EPSILON * DEGENERATE_CROSS_EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texinfo_with(texture_vecs: [[f32; 4]; 2], lightmap_vecs: [[f32; 4]; 2]) -> TexInfo {
        TexInfo {
            texture_vecs,
            lightmap_vecs,
            flags: 0,
            texdata: 0,
        }
    }

    #[test]
    fn texture_uv_divides_by_material_size() {
        // s runs along +x at one texel per unit, t along +y.
        let texinfo = texinfo_with(
            [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
            [[0.0; 4], [0.0; 4]],
        );
        let texdata = DTexData {
            reflectivity: [0.0; 3],
            name_string_table_id: 0,
            width: 256,
            height: 128,
            view_width: 256,
            view_height: 128,
        };

        assert_eq!(texture_uv([0.0, 0.0, 0.0], &texinfo, &texdata), [0.0, 0.0]);
        // One full tile across in each axis.
        assert_eq!(texture_uv([256.0, 128.0, 0.0], &texinfo, &texdata), [1.0, 1.0]);
        // UVs must be allowed past 1.0 so the sampler can wrap.
        assert_eq!(texture_uv([512.0, 0.0, 0.0], &texinfo, &texdata), [2.0, 0.0]);
    }

    #[test]
    fn texture_uv_survives_a_zero_sized_texdata() {
        let texinfo = texinfo_with(
            [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
            [[0.0; 4], [0.0; 4]],
        );
        let texdata = DTexData {
            reflectivity: [0.0; 3],
            name_string_table_id: 0,
            width: 0,
            height: 0,
            view_width: 0,
            view_height: 0,
        };
        let uv = texture_uv([8.0, 4.0, 0.0], &texinfo, &texdata);
        assert!(uv[0].is_finite() && uv[1].is_finite(), "got {uv:?}");
    }

    #[test]
    fn lightmap_uv_spans_the_sample_grid_with_a_half_luxel_inset() {
        // A face 4x4 luxels in extent, so a 5x5 sample grid, whose luxel
        // origin is offset — the case where forgetting `mins` goes unnoticed
        // on a map with one face at the origin.
        let texinfo = texinfo_with(
            [[0.0; 4], [0.0; 4]],
            [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
        );
        // `DFace::num_prims` is private, so build it field-by-field rather
        // than with struct-update syntax.
        let mut face: DFace = bytemuck::Zeroable::zeroed();
        face.lightmap_texture_mins_in_luxels = [10, 20];
        face.lightmap_texture_size_in_luxels = [4, 4];
        assert_eq!(face.lightmap_width(), 5);
        assert_eq!(face.lightmap_height(), 5);

        // At the luxel origin: half a luxel in, i.e. the centre of texel 0.
        let at_origin = lightmap_uv([10.0, 20.0, 0.0], &texinfo, &face);
        assert!((at_origin[0] - 0.1).abs() < 1e-6, "got {at_origin:?}");
        assert!((at_origin[1] - 0.1).abs() < 1e-6, "got {at_origin:?}");

        // At the far corner: the centre of the last texel, inside the grid.
        let at_far = lightmap_uv([14.0, 24.0, 0.0], &texinfo, &face);
        assert!((at_far[0] - 0.9).abs() < 1e-6, "got {at_far:?}");
        assert!(at_far[0] < 1.0 && at_far[1] < 1.0, "must stay in the grid");
    }

    #[test]
    fn degenerate_detection_rejects_only_slivers() {
        // A healthy right triangle.
        let good = [[0.0, 0.0, 0.0], [16.0, 0.0, 0.0], [0.0, 16.0, 0.0]];
        assert!(!is_degenerate(&good, 1));

        // Three collinear points have no area.
        let collinear = [[0.0, 0.0, 0.0], [8.0, 0.0, 0.0], [16.0, 0.0, 0.0]];
        assert!(is_degenerate(&collinear, 1));

        // A duplicated vertex, which is what compiler slivers look like.
        let duplicate = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 0.0, 0.0]];
        assert!(is_degenerate(&duplicate, 1));
    }

    #[test]
    fn skip_render_mask_covers_the_tool_surfaces() {
        // Guards against a typo in the mask silently rendering tool brushes.
        for flag in [
            flags::SURF_NODRAW,
            flags::SURF_SKY,
            flags::SURF_SKY2D,
            flags::SURF_HINT,
            flags::SURF_SKIP,
            flags::SURF_TRIGGER,
        ] {
            assert_ne!(flags::SURF_SKIP_RENDER & flag, 0, "{flag:#x} not masked");
        }
        // ...and does not accidentally drop ordinary visible geometry.
        for flag in [
            flags::SURF_LIGHT,
            flags::SURF_WARP,
            flags::SURF_TRANS,
            flags::SURF_BUMPLIGHT,
            flags::SURF_NOSHADOWS,
        ] {
            assert_eq!(flags::SURF_SKIP_RENDER & flag, 0, "{flag:#x} over-masked");
        }
    }
}
