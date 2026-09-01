//! Tessellate displacement surfaces — Source's terrain.
//!
//! Most TF2 maps put all ground, cliffs and embankments in displacements
//! rather than brush faces: `cp_badlands` has 1191 of them, and without these
//! the map is a set of buildings floating over a void.
//!
//! A displacement is a regular `(2^power + 1)²` grid of vertices bilinearly
//! interpolated across its parent brush face's four corners, with each vertex
//! then pushed along a stored direction by a stored distance. Everything here
//! is transcribed from `CCoreDispInfo` in
//! `source-sdk-2013/src/public/builddisp.cpp`; the three details that matter
//! are documented at their use sites:
//!
//! * which parent corner becomes grid origin ([`corner_ring`]),
//! * the interpolation order, which is *not* a plain `(u, v)` bilerp
//!   ([`build_displacements`]),
//! * and the per-quad diagonal, which alternates ([`quad_splits_top_left`]).
//!
//! # Coordinates are interpolated from the corners, never projected
//!
//! A brush face's UVs come from projecting each vertex through the texinfo's
//! affine axes. **Displacements do not work that way at all.**
//! `CCoreDispInfo::CalcDispSurfCoords` ([builddisp.cpp:1547]) takes the parent
//! quad's four *corner* coordinates and interpolates them across the same
//! parametric grid the positions use — the identical `i`/`j` walk, for both
//! `m_TexCoord` and `m_LuxelCoords`.
//!
//! Projecting the **displaced** position instead looks almost right and fails in
//! a specific, visible way: a vertex pushed off the parent plane projects to a
//! shifted coordinate, by an amount proportional to how far it moved.
//! Neighbouring displacements then disagree along their shared edge, so baked
//! shadows break at every displacement boundary and the terrain reads as a grid.
//!
//! The two corner sources are **not** the same:
//!
//! * **Texture** corners are the projection of the flat parent corners.
//! * **Lightmap** corners come from the luxel grid's own dimensions
//!   ([`corner_luxel_coords`]) and are never projected. This is not a
//!   micro-optimisation of the same formula — it is a different quantity. On
//!   `cp_badlands` face 13272 the parent ring spans 122 luxels of *world*
//!   projection along U and 0.5 along V, while the face's own
//!   `m_LightmapTextureSizeInLuxels` is `[1, 121]`: the grid axes are the
//!   displacement's parametric ones, and which world direction each corresponds
//!   to is decided by `LongestInU` and a possible swap inside
//!   `CalcLuxelCoords`, not by any projection this code could redo. Projecting
//!   put 5 533 of Badlands' 42 415 terrain vertices outside their own lightmap
//!   rect, up to 60x past its edge.
//!
//! Taking the corners from the grid makes every vertex land inside the rect by
//! construction, and makes a shared edge agree on both sides.
//!
//! [builddisp.cpp:1547]: ../../../source-sdk-2013/src/public/builddisp.cpp#L1547

use crate::flags;
use crate::geometry::{self, FaceChunk, Surface, SurfaceSet, Vertex};
use crate::raw::{DEdge, DFace, DVertex, DispVert, TexInfo};
use crate::{Bsp, Result};

/// Counters for a displacement build.
#[derive(Clone, Copy, Debug, Default)]
pub struct DispStats {
    /// Entries in the DISPINFO lump.
    pub displacements: usize,
    pub built: usize,
    /// `power` outside `MIN_MAP_DISP_POWER..=MAX_MAP_DISP_POWER`.
    pub skipped_bad_power: usize,
    /// Parent face missing, not a quad, or with out-of-range edge indices.
    pub skipped_bad_face: usize,
    pub skipped_no_texinfo: usize,
    /// DISP_VERTS did not hold a whole grid for this displacement.
    pub skipped_short_verts: usize,
    /// Triangles dropped for carrying `DISPTRI_TAG_REMOVE`.
    pub removed_triangles: usize,
    pub vertices: usize,
    pub triangles: usize,
}

impl DispStats {
    pub fn skipped(&self) -> usize {
        self.skipped_bad_power
            + self.skipped_bad_face
            + self.skipped_no_texinfo
            + self.skipped_short_verts
    }
}

/// Tessellated terrain, grouped by material like [`geometry::ModelGeometry`].
#[derive(Clone, Debug)]
pub struct DispGeometry {
    pub surfaces: Vec<Surface>,
    pub stats: DispStats,
}

impl DispGeometry {
    pub fn vertex_count(&self) -> usize {
        self.surfaces.iter().map(|s| s.vertices.len()).sum()
    }

    pub fn triangle_count(&self) -> usize {
        self.surfaces.iter().map(Surface::triangle_count).sum()
    }
}

/// `MIN_MAP_DISP_POWER` / `MAX_MAP_DISP_POWER` from `bspfile.h`.
const MIN_DISP_POWER: i32 = 2;
const MAX_DISP_POWER: i32 = 4;

/// Whether the quad at flat grid index `ndx` splits top-left to bottom-right.
///
/// Transcribed from `CCoreDispInfo::GenerateCollisionSurface`, which tests the
/// parity of the **flat** index `ndx = row * width + col`, not of `row + col`:
///
/// ```text
/// bool bOdd = ( ( ndx % 2 ) == 1 );
/// if ( bOdd ) BuildTriTLtoBR( ndx ); else BuildTriBLtoTR( ndx );
/// ```
///
/// The two agree only because `width = 2^power + 1` is always odd, which makes
/// `row * width` and `row` share parity. Keeping Valve's form means this stays
/// correct even if a map ever carried an even width.
#[inline]
fn quad_splits_top_left(ndx: usize) -> bool {
    ndx % 2 == 1
}

/// Rotate a parent face's four corners so the one nearest `start_position`
/// comes first.
///
/// Mirrors `CCoreDispSurface::FindSurfPointStartIndex` (nearest point wins)
/// combined with the `m_Points[(k + iOffset) % 4]` access pattern used
/// throughout `builddisp.cpp`. Getting this wrong rotates the whole grid, which
/// shows up as terrain that is the right shape but wrongly oriented.
fn corner_ring(ring: [[f32; 3]; 4], start_position: [f32; 3]) -> [[f32; 3]; 4] {
    let mut best = 0usize;
    let mut best_dist = f32::INFINITY;
    for (i, point) in ring.iter().enumerate() {
        let d = [
            point[0] - start_position[0],
            point[1] - start_position[1],
            point[2] - start_position[2],
        ];
        let dist = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        if dist < best_dist {
            best_dist = dist;
            best = i;
        }
    }
    [
        ring[best],
        ring[(best + 1) % 4],
        ring[(best + 2) % 4],
        ring[(best + 3) % 4],
    ]
}

/// A displacement's four corner lightmap coordinates.
///
/// Transcribed from `CCoreDispSurface::CalcLuxelCoords`
/// ([builddisp.cpp:508-513]), which assigns them straight from the luxel grid's
/// own dimensions with a half-luxel inset:
///
/// ```c
/// m_LuxelCoords[(0+iOffset)%4].Init( 0.5f,            0.5f );
/// m_LuxelCoords[(1+iOffset)%4].Init( 0.5f,            flVValue + 0.5 );
/// m_LuxelCoords[(2+iOffset)%4].Init( flUValue + 0.5,  flVValue + 0.5 );
/// m_LuxelCoords[(3+iOffset)%4].Init( flUValue + 0.5,  0.5f );
/// ```
///
/// `flUValue`/`flVValue` are the face's `m_LightmapTextureSizeInLuxels`, and
/// `iOffset` is the start-corner rotation [`corner_ring`] already applies — so
/// the table maps onto the rotated ring in order.
///
/// Returned normalised by the sample grid, matching what the atlas packer
/// expects from [`geometry::lightmap_uv`].
///
/// [builddisp.cpp:508-513]: ../../../source-sdk-2013/src/public/builddisp.cpp#L508
fn corner_luxel_coords(face: &DFace) -> [[f32; 2]; 4] {
    let (w, h) = (face.lightmap_width() as f32, face.lightmap_height() as f32);
    let size = face.lightmap_texture_size_in_luxels;
    let (u0, v0) = (0.5 / w, 0.5 / h);
    let (u1, v1) = ((size[0] as f32 + 0.5) / w, (size[1] as f32 + 0.5) / h);
    [[u0, v0], [u0, v1], [u1, v1], [u1, v0]]
}

/// Linear interpolation of a 2D coordinate.
#[inline]
fn lerp2(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// A coordinate at grid parameters `(s, t)`, interpolated from the four rotated
/// corner values.
///
/// The same two-stage walk as the positions — `s` along `c0 -> c1` and
/// `c3 -> c2`, then `t` between those — so a coordinate and the vertex it
/// belongs to always share their parameters. Both texture and lightmap
/// coordinates go through here; see the module docs for where the corner values
/// come from, which is *not* the same source for the two.
#[inline]
fn grid_coord(corners: &[[f32; 2]; 4], s: f32, t: f32) -> [f32; 2] {
    let near = lerp2(corners[0], corners[1], s);
    let far = lerp2(corners[3], corners[2], s);
    lerp2(near, far, t)
}

#[inline]
fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[inline]
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let len_sq = dot(v, v);
    if len_sq > 1e-20 {
        let inv = len_sq.sqrt().recip();
        [v[0] * inv, v[1] * inv, v[2] * inv]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Read a face's edge ring, returning `None` if it is not a well-formed quad.
fn face_quad(
    firstedge: i32,
    numedges: i16,
    surfedges: &[i32],
    edges: &[DEdge],
    positions: &[DVertex],
) -> Option<[[f32; 3]; 4]> {
    if numedges != 4 {
        return None;
    }
    let mut ring = [[0.0f32; 3]; 4];
    for (i, slot) in ring.iter_mut().enumerate() {
        let surfedge = *surfedges.get(firstedge as usize + i)?;
        let (edge_index, end) = if surfedge >= 0 {
            (surfedge as usize, 0)
        } else {
            (surfedge.unsigned_abs() as usize, 1)
        };
        *slot = positions.get(edges.get(edge_index)?.v[end] as usize)?.point;
    }
    Some(ring)
}

/// Tessellate every displacement in the map.
///
/// Individual malformed displacements are skipped and counted rather than
/// failing the map; only a bad lump is an error.
///
/// # No orientation fixup
///
/// An earlier version compared each displaced surface's accumulated normal
/// against its parent face's flat plane and flipped any that disagreed. That
/// check is **not a valid invariant** and was removed: on `ctf_crasher` most
/// disagreements have an agreement term that rounds to `-0.0` — pure
/// floating-point cancellation on symmetric or folded surfaces — while two are
/// genuinely negative because the displacement really does fold back past its
/// base plane. Flipping those would have inverted correct geometry.
///
/// Winding is instead correct *by construction*: the corner rotation, Valve's
/// diagonal parity, and one explicit reversal are each pinned by unit tests.
pub fn build_displacements(bsp: &Bsp) -> Result<DispGeometry> {
    let dispinfos = bsp.dispinfos()?;
    let disp_verts = bsp.disp_verts()?;
    let disp_tris = bsp.disp_tris()?;
    let faces = bsp.faces()?;
    let texinfos = bsp.texinfos()?;
    let texdatas = bsp.texdatas()?;
    let positions = bsp.vertices()?;
    let edges = bsp.edges()?;
    let surfedges = bsp.surfedges()?;

    let mut stats = DispStats {
        displacements: dispinfos.len(),
        ..Default::default()
    };
    let mut surfaces = SurfaceSet::new();
    // Reused across displacements to keep this allocation-light: 289 verts max.
    let mut grid: Vec<Vertex> = Vec::with_capacity(289);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(289);
    let mut local_indices: Vec<u32> = Vec::with_capacity(512 * 3);

    for disp in dispinfos {
        if !(MIN_DISP_POWER..=MAX_DISP_POWER).contains(&disp.power) {
            stats.skipped_bad_power += 1;
            continue;
        }

        let face_index = disp.map_face as usize;
        let Some(face) = faces.get(face_index) else {
            stats.skipped_bad_face += 1;
            continue;
        };
        let Some(texinfo) = usize::try_from(face.texinfo)
            .ok()
            .and_then(|i| texinfos.get(i))
        else {
            stats.skipped_no_texinfo += 1;
            continue;
        };
        let Some(texdata) = usize::try_from(texinfo.texdata)
            .ok()
            .and_then(|i| texdatas.get(i))
        else {
            stats.skipped_no_texinfo += 1;
            continue;
        };
        let Some(ring) = face_quad(
            face.firstedge,
            face.numedges,
            surfedges,
            edges,
            positions,
        ) else {
            stats.skipped_bad_face += 1;
            continue;
        };

        let side = disp.side_len();
        let count = disp.num_verts();
        let vert_start = disp.disp_vert_start.max(0) as usize;
        let Some(field) = disp_verts.get(vert_start..vert_start + count) else {
            stats.skipped_short_verts += 1;
            continue;
        };

        let corners = corner_ring(ring, disp.start_position);
        let step = (side - 1) as f32;

        // Interpolation order is Valve's, not a plain (u, v) bilerp: `i` walks
        // corner0 -> corner1 on one edge and corner3 -> corner2 on the other,
        // then `j` crosses between those two points. The flat index is
        // `i * side + j`, matching `CCoreDispInfo::GenerateDispSurf`.
        grid.clear();
        // Texture coordinates: project the **flat** parent corners, then
        // interpolate. Lightmap coordinates: the corners of the luxel grid
        // itself, which is a different rule entirely — see the module docs.
        let corner_uv = corners.map(|c| geometry::texture_uv(c, texinfo, texdata));
        let corner_lightmap_uv = corner_luxel_coords(face);

        for i in 0..side {
            let s = i as f32 / step;
            let near = lerp(corners[0], corners[1], s);
            let far = lerp(corners[3], corners[2], s);

            for j in 0..side {
                let t = j as f32 / step;
                let flat = lerp(near, far, t);
                let dv: &DispVert = &field[i * side + j];
                let position = [
                    flat[0] + dv.vector[0] * dv.dist,
                    flat[1] + dv.vector[1] * dv.dist,
                    flat[2] + dv.vector[2] * dv.dist,
                ];
                grid.push(Vertex {
                    position,
                    // Filled in below, once the triangles are known.
                    normal: [0.0, 0.0, 1.0],
                    uv: grid_coord(&corner_uv, s, t),
                    lightmap_uv: grid_coord(&corner_lightmap_uv, s, t),
                    // Stored 0..255 in the lump; the shader wants 0..1.
                    alpha: (dv.alpha / 255.0).clamp(0.0, 1.0),
                });
            }
        }

        // Two triangles per quad, alternating the diagonal.
        let tri_start = disp.disp_tri_start.max(0) as usize;
        local_indices.clear();
        for row in 0..side - 1 {
            for col in 0..side - 1 {
                let ndx = row * side + col;
                let quad = [
                    ndx as u32,                     // this row, this column
                    (ndx + side) as u32,            // next row, this column
                    (ndx + 1) as u32,               // this row, next column
                    (ndx + side + 1) as u32,        // next row, next column
                ];
                let (a, b) = if quad_splits_top_left(ndx) {
                    // BuildTriTLtoBR
                    ([quad[0], quad[1], quad[2]], [quad[2], quad[1], quad[3]])
                } else {
                    // BuildTriBLtoTR
                    ([quad[0], quad[1], quad[3]], [quad[0], quad[3], quad[2]])
                };

                let quad_index = row * (side - 1) + col;
                for (which, tri) in [a, b].into_iter().enumerate() {
                    let tag = disp_tris
                        .get(tri_start + quad_index * 2 + which)
                        .map_or(0, |t| t.tags);
                    if tag & flags::DISPTRI_TAG_REMOVE != 0 {
                        stats.removed_triangles += 1;
                        continue;
                    }
                    local_indices.extend_from_slice(&tri);
                }
            }
        }

        // Valve's `BuildTri*` emit clockwise-from-front, exactly like a brush
        // face's edge ring, while wgpu treats counter-clockwise as front. Turn
        // them around once here rather than transcribing the index triples
        // backwards above, so that code still reads as `builddisp.cpp` does.
        for tri in local_indices.as_chunks_mut::<3>().0 {
            tri.swap(1, 2);
        }

        // Smooth normals: terrain shaded flat looks like crumpled paper.
        // Accumulate un-normalised triangle normals so larger triangles carry
        // proportionally more weight, as area-weighting does for free.
        normals.clear();
        normals.resize(grid.len(), [0.0; 3]);
        for tri in local_indices.as_chunks::<3>().0 {
            let [i0, i1, i2] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let n = cross(
                sub(grid[i1].position, grid[i0].position),
                sub(grid[i2].position, grid[i0].position),
            );
            for &i in &[i0, i1, i2] {
                normals[i] = [
                    normals[i][0] + n[0],
                    normals[i][1] + n[1],
                    normals[i][2] + n[2],
                ];
            }
        }

        for (vertex, normal) in grid.iter_mut().zip(&normals) {
            vertex.normal = normalized(*normal);
        }

        let surface = surfaces.surface_for(texinfo.texdata, texinfo.flags);
        let base = surface.vertices.len() as u32;
        surface.vertices.extend(grid.iter().copied());
        surface.indices.extend(local_indices.iter().map(|i| i + base));
        surface.chunks.push(FaceChunk {
            face: face_index as u32,
            first_vertex: base,
            vertex_count: grid.len() as u32,
        });
        stats.built += 1;
    }

    let surfaces = surfaces.into_surfaces();
    stats.vertices = surfaces.iter().map(|s| s.vertices.len()).sum();
    stats.triangles = surfaces.iter().map(Surface::triangle_count).sum();

    Ok(DispGeometry { surfaces, stats })
}

/// The luxel grid dimensions vbsp would derive for a displacement's parent
/// face, recomputed from the rotated parent quad and the face's lightmap
/// vectors.
///
/// Transcribed from `CCoreDispSurface::CalcLuxelCoords`
/// (`public/builddisp.cpp:442`): the U extent is the longer of the two edges
/// running corner 0 -> 3 and corner 1 -> 2, the V extent the longer of
/// 0 -> 1 and 3 -> 2, each divided by the face's world-units-per-luxel and
/// truncated, plus one, capped at `MAX_DISP_LIGHTMAP_DIM_WITHOUT_BORDER`.
///
/// This exists to be compared against the `m_LightmapTextureSizeInLuxels` the
/// compiler actually stored — see [`check_luxel_axes`]. `None` when the
/// lightmap vectors are degenerate, which would make the scale meaningless.
pub fn luxel_dims_from_edges(corners: &[[f32; 3]; 4], texinfo: &TexInfo) -> Option<[i32; 2]> {
    // `nLuxelsPerWorldUnit` in Valve's source, but the reciprocal of its name:
    // the vector's length is luxels per unit, so this is units per luxel.
    //
    // Guard the length *before* dividing: a zeroed lightmap vector gives
    // infinity, and Rust's float-to-int cast saturates rather than trapping, so
    // `<= 0` alone would let `i32::MAX` through and silently return a 1 x 1
    // grid instead of admitting the scale is meaningless.
    let luxels_per_unit = length(&texinfo.lightmap_vecs[0][..3]);
    if !luxels_per_unit.is_finite() || luxels_per_unit <= 0.0 {
        return None;
    }
    let units_per_luxel = (1.0 / luxels_per_unit) as i32;
    if units_per_luxel <= 0 {
        return None;
    }
    let scale = 1.0 / units_per_luxel as f32;

    let u_length = distance(corners[3], corners[0]).max(distance(corners[2], corners[1]));
    let v_length = distance(corners[1], corners[0]).max(distance(corners[2], corners[3]));

    Some([
        ((u_length * scale) as i32 + 1).min(MAX_DISP_LUXEL_DIM),
        ((v_length * scale) as i32 + 1).min(MAX_DISP_LUXEL_DIM),
    ])
}

/// `MAX_DISP_LIGHTMAP_DIM_WITHOUT_BORDER` (`bspfile.h:35`).
const MAX_DISP_LUXEL_DIM: i32 = 125;

fn length(v: &[f32]) -> f32 {
    v.iter().map(|c| c * c).sum::<f32>().sqrt()
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    length(&[a[0] - b[0], a[1] - b[1], a[2] - b[2]])
}

/// Result of [`check_luxel_axes`].
#[derive(Clone, Copy, Debug, Default)]
pub struct LuxelAxisCheck {
    /// Recomputed dimensions matched the stored ones in the order
    /// [`corner_luxel_coords`] uses them.
    pub agree: usize,
    /// **Recomputed dimensions matched only when transposed.** Non-zero means
    /// U and V are the wrong way round, or the corner rotation is wrong. This
    /// is the failure this check exists for.
    pub transposed: usize,
    /// Neither pairing matched exactly. Expected at a low rate: vbsp truncates
    /// `int(length / units_per_luxel)`, so a displacement whose edge lands on
    /// an integer luxel boundary can differ by one from a recomputation over
    /// the final BSP winding, which vbsp clipped after computing this.
    pub inconclusive: usize,
    /// Displacements whose stored grid is square, where a transpose is
    /// invisible. Reported so the check's discriminating power is visible
    /// rather than assumed.
    pub square: usize,
}

/// Cross-check the luxel axes against the dimensions vbsp stored.
///
/// [`corner_luxel_coords`] assigns `m_LightmapTextureSizeInLuxels[0]` to the
/// axis running corner 0 -> 3 and `[1]` to corner 0 -> 1. Nothing in that
/// function can detect getting those backwards — the coordinates it produces
/// are inside `0..1` either way. The only independent evidence available is
/// the map itself: recompute what vbsp's own formula would have derived from
/// the parent quad's edge lengths, and see whether it matches the stored pair
/// directly or transposed.
pub fn check_luxel_axes(bsp: &Bsp) -> Result<LuxelAxisCheck> {
    let dispinfos = bsp.dispinfos()?;
    let faces = bsp.faces()?;
    let texinfos = bsp.texinfos()?;
    let positions = bsp.vertices()?;
    let edges = bsp.edges()?;
    let surfedges = bsp.surfedges()?;

    let mut check = LuxelAxisCheck::default();
    for disp in dispinfos {
        let Some(face) = faces.get(disp.map_face as usize) else {
            continue;
        };
        let Some(texinfo) = usize::try_from(face.texinfo)
            .ok()
            .and_then(|i| texinfos.get(i))
        else {
            continue;
        };
        let Some(ring) = face_quad(
            face.firstedge,
            face.numedges,
            surfedges,
            edges,
            positions,
        ) else {
            continue;
        };
        let corners = corner_ring(ring, disp.start_position);
        let Some(computed) = luxel_dims_from_edges(&corners, texinfo) else {
            continue;
        };

        let stored = face.lightmap_texture_size_in_luxels;
        if stored[0] == stored[1] {
            check.square += 1;
        }
        if computed == stored {
            check.agree += 1;
        } else if computed == [stored[1], stored[0]] {
            check.transposed += 1;
        } else {
            check.inconclusive += 1;
        }
    }
    Ok(check)
}

/// The lightmap coordinate [`corner_luxel_coords`] and [`grid_coord`] produce
/// for grid vertex `(i, j)`, in closed form.
///
/// Substituting the four luxel corners into the bilinear collapses to this:
///
/// ```text
/// u = (0.5 + U * j / (side - 1)) / (U + 1)
/// v = (0.5 + V * i / (side - 1)) / (V + 1)
/// ```
///
/// Note **`u` follows `j` and `v` follows `i`** — the axes cross, because `i`
/// walks corner 0 -> 1, which is the V edge. That transposition is the single
/// easiest thing to get backwards here and the reason this exists: it is a
/// second, independent derivation of the same quantity, so comparing the two
/// is a real check rather than a restatement.
pub fn analytic_lightmap_uv(face: &DFace, side: usize, i: usize, j: usize) -> [f32; 2] {
    let size = face.lightmap_texture_size_in_luxels;
    let step = (side - 1).max(1) as f32;
    [
        (0.5 + size[0] as f32 * j as f32 / step) / face.lightmap_width() as f32,
        (0.5 + size[1] as f32 * i as f32 / step) / face.lightmap_height() as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagonal_alternates_and_matches_valve_parity() {
        // Valve tests the flat index; for every legal width (5, 9, 17 — all
        // odd) that is the same as testing row + col. Prove the equivalence
        // holds for the widths that can actually occur.
        for side in [5usize, 9, 17] {
            for row in 0..side - 1 {
                for col in 0..side - 1 {
                    let ndx = row * side + col;
                    assert_eq!(
                        quad_splits_top_left(ndx),
                        (row + col) % 2 == 1,
                        "side {side} row {row} col {col}"
                    );
                }
            }
        }
        // ...and that it genuinely alternates along a row.
        assert_ne!(quad_splits_top_left(0), quad_splits_top_left(1));
    }

    #[test]
    fn corner_ring_rotates_nearest_corner_to_the_front() {
        let ring = [
            [0.0, 0.0, 0.0],
            [0.0, 64.0, 0.0],
            [64.0, 64.0, 0.0],
            [64.0, 0.0, 0.0],
        ];
        // Nearest to corner 2 -> that becomes the origin, order preserved.
        let rotated = corner_ring(ring, [60.0, 60.0, 0.0]);
        assert_eq!(rotated[0], ring[2]);
        assert_eq!(rotated[1], ring[3]);
        assert_eq!(rotated[2], ring[0]);
        assert_eq!(rotated[3], ring[1]);

        // An exact hit on corner 0 leaves the ring alone.
        assert_eq!(corner_ring(ring, ring[0]), ring);
    }

    /// The interpolation must put the four corners at the four grid corners,
    /// which is what catches a transposed or mis-paired bilerp.
    #[test]
    fn grid_interpolation_reproduces_the_corners() {
        let corners = [
            [0.0, 0.0, 0.0],
            [0.0, 64.0, 0.0],
            [64.0, 64.0, 0.0],
            [64.0, 0.0, 0.0],
        ];
        let side = 5usize;
        let step = (side - 1) as f32;
        let at = |i: usize, j: usize| {
            let s = i as f32 / step;
            let t = j as f32 / step;
            lerp(
                lerp(corners[0], corners[1], s),
                lerp(corners[3], corners[2], s),
                t,
            )
        };
        assert_eq!(at(0, 0), corners[0]);
        assert_eq!(at(side - 1, 0), corners[1]);
        assert_eq!(at(side - 1, side - 1), corners[2]);
        assert_eq!(at(0, side - 1), corners[3]);
        // The midpoint of a planar quad is the average of its corners.
        let mid = at(2, 2);
        assert!((mid[0] - 32.0).abs() < 1e-4 && (mid[1] - 32.0).abs() < 1e-4);
    }

    #[test]
    fn the_grid_reproduces_its_corner_values_exactly() {
        // A corner vertex must get its corner's coordinate, not something a
        // half-texel off, or every displacement's lighting is shifted.
        let corners = [[0.1, 0.2], [0.1, 0.8], [0.9, 0.8], [0.9, 0.2]];
        assert_eq!(grid_coord(&corners, 0.0, 0.0), corners[0]);
        assert_eq!(grid_coord(&corners, 1.0, 0.0), corners[1]);
        assert_eq!(grid_coord(&corners, 1.0, 1.0), corners[2]);
        assert_eq!(grid_coord(&corners, 0.0, 1.0), corners[3]);
    }

    #[test]
    fn an_edge_depends_only_on_that_edges_corners() {
        // This is what makes a seam continuous, and it is the property the
        // grid-shaped shadows violated. Two displacements sharing an edge agree
        // on that edge's two corners but not on the opposite two, so the
        // coordinates along it must ignore the far pair.
        //
        // "Ignore" holds mathematically but not bit-exactly: `lerp` computes
        // `a + (b - a) * t`, which at `t = 1` is not identical to `b` once the
        // endpoints are far apart. The deliberately absurd far corners below
        // make that rounding as large as it can get — about 2e-7, or 1e-5 of a
        // luxel on the narrowest grid TF2 ships. Hence a tolerance rather than
        // equality; the bug this guards against moved coordinates by up to 60
        // whole rect widths.
        const TOLERANCE: f32 = 1e-6;
        let close = |a: [f32; 2], b: [f32; 2]| {
            (a[0] - b[0]).abs() < TOLERANCE && (a[1] - b[1]).abs() < TOLERANCE
        };

        let corners = [[0.1, 0.2], [0.1, 0.8], [0.9, 0.8], [0.9, 0.2]];

        // Same near edge (0, 1), wildly different far edge (3, 2).
        let neighbour = [[0.1, 0.2], [0.1, 0.8], [-7.0, 4.0], [3.0, -5.0]];
        for step in 0..=8 {
            let s = step as f32 / 8.0;
            let (a, b) = (
                grid_coord(&corners, s, 0.0),
                grid_coord(&neighbour, s, 0.0),
            );
            assert!(close(a, b), "t = 0 edge moved: {a:?} vs {b:?} at s = {s}");
        }

        // ...and symmetrically, the far edge ignores the near corners.
        let neighbour = [[6.0, 6.0], [-2.0, 9.0], [0.9, 0.8], [0.9, 0.2]];
        for step in 0..=8 {
            let s = step as f32 / 8.0;
            let (a, b) = (
                grid_coord(&corners, s, 1.0),
                grid_coord(&neighbour, s, 1.0),
            );
            assert!(close(a, b), "t = 1 edge moved: {a:?} vs {b:?} at s = {s}");
        }

        // With the values a real map actually holds — all four corners inside
        // 0..1 — the same comparison passes far more tightly.
        let realistic = [[0.1, 0.2], [0.1, 0.8], [0.42, 0.61], [0.37, 0.05]];
        for step in 0..=8 {
            let s = step as f32 / 8.0;
            let (a, b) = (
                grid_coord(&corners, s, 0.0),
                grid_coord(&realistic, s, 0.0),
            );
            assert_eq!(a, b, "realistic corners should agree exactly at s = {s}");
        }
    }

    #[test]
    fn lightmap_corners_come_from_the_luxel_grid_not_a_projection() {
        // `cp_badlands` face 13272: a 2 x 122 luxel grid whose world projection
        // along U spans 122 luxels and along V spans 0.5 — the case that proved
        // no projection could produce these numbers.
        let mut face: DFace = bytemuck::Zeroable::zeroed();
        face.lightmap_texture_size_in_luxels = [1, 121];
        assert_eq!(face.lightmap_width(), 2);
        assert_eq!(face.lightmap_height(), 122);

        let corners = corner_luxel_coords(&face);
        // Half a luxel in from each edge of the grid, per `CalcLuxelCoords`.
        assert_eq!(corners[0], [0.5 / 2.0, 0.5 / 122.0]);
        assert_eq!(corners[1], [0.5 / 2.0, 121.5 / 122.0]);
        assert_eq!(corners[2], [1.5 / 2.0, 121.5 / 122.0]);
        assert_eq!(corners[3], [1.5 / 2.0, 0.5 / 122.0]);

        // Every grid coordinate is then a convex combination of those, so it
        // cannot leave 0..1. Worth stating, but note it is a *property of the
        // formula*, not a test of it: no implementation of `corner_luxel_coords`
        // returning corners inside the rect could fail this. The checks with
        // teeth are the two below.
        for i in 0..=8 {
            for j in 0..=8 {
                let uv = grid_coord(&corners, i as f32 / 8.0, j as f32 / 8.0);
                assert!(
                    uv.iter().all(|c| (0.0..=1.0).contains(c)),
                    "grid coord {uv:?} left the rect"
                );
            }
        }
    }

    /// Two independent derivations of the same coordinate must agree.
    ///
    /// The builder interpolates four corner values; [`analytic_lightmap_uv`]
    /// evaluates the closed form that bilinear collapses to. They share no
    /// code, so agreement is evidence rather than a restatement.
    /// Slack for the two derivations' differing operation order. One ULP near
    /// 1.0 is about 1.2e-7; the smallest luxel step this must not mask is
    /// 1/126 of the rect, or ~8e-3.
    const CLOSED_FORM_TOLERANCE: f32 = 1e-6;

    #[test]
    fn the_closed_form_agrees_with_the_interpolated_grid() {
        // Square, wide, tall, and the degenerate Badlands 2 x 122 case.
        for size in [[8, 8], [24, 7], [1, 121], [125, 3]] {
            let mut face: DFace = bytemuck::Zeroable::zeroed();
            face.lightmap_texture_size_in_luxels = size;
            let corners = corner_luxel_coords(&face);

            for side in [5usize, 9, 17] {
                let step = (side - 1) as f32;
                for i in 0..side {
                    for j in 0..side {
                        let built =
                            grid_coord(&corners, i as f32 / step, j as f32 / step);
                        let closed = analytic_lightmap_uv(&face, side, i, j);
                        // Not bit-identical: the two evaluate the same
                        // expression in a different order, so they differ by
                        // an ULP or so. A wrong axis or inset is off by whole
                        // luxels — orders of magnitude above this.
                        for axis in 0..2 {
                            assert!(
                                (built[axis] - closed[axis]).abs() <= CLOSED_FORM_TOLERANCE,
                                "size {size:?} side {side} vertex ({i}, {j}):                                  built {built:?} vs closed {closed:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The check above would be worthless if U and V were interchangeable.
    ///
    /// Transposing the stored luxel dimensions must move the coordinates, and
    /// `u` must follow `j` while `v` follows `i` — the axes cross, because `i`
    /// walks corner 0 -> 1, which is the V edge.
    #[test]
    fn the_luxel_axes_are_not_interchangeable() {
        let mut face: DFace = bytemuck::Zeroable::zeroed();
        face.lightmap_texture_size_in_luxels = [24, 7];
        let mut swapped: DFace = bytemuck::Zeroable::zeroed();
        swapped.lightmap_texture_size_in_luxels = [7, 24];

        // Somewhere off both diagonals, so a transpose cannot coincide.
        let (side, i, j) = (9usize, 2, 6);
        assert_ne!(
            analytic_lightmap_uv(&face, side, i, j),
            analytic_lightmap_uv(&swapped, side, i, j),
            "a transposed luxel grid must produce different coordinates"
        );

        // u varies with j alone; v with i alone.
        let base = analytic_lightmap_uv(&face, side, i, j);
        assert_eq!(analytic_lightmap_uv(&face, side, i + 1, j)[0], base[0]);
        assert_ne!(analytic_lightmap_uv(&face, side, i + 1, j)[1], base[1]);
        assert_ne!(analytic_lightmap_uv(&face, side, i, j + 1)[0], base[0]);
        assert_eq!(analytic_lightmap_uv(&face, side, i, j + 1)[1], base[1]);
    }

    /// [`luxel_dims_from_edges`] transcribes `CalcLuxelCoords`' length rule.
    #[test]
    fn luxel_dims_come_from_the_longer_edge_of_each_axis() {
        // 16 units per luxel, the common TF2 lightmap scale.
        let mut texinfo: TexInfo = bytemuck::Zeroable::zeroed();
        texinfo.lightmap_vecs[0] = [1.0 / 16.0, 0.0, 0.0, 0.0];
        texinfo.lightmap_vecs[1] = [0.0, 1.0 / 16.0, 0.0, 0.0];

        // A trapezoid: the 0->3 edge is 128 long, 1->2 is 160. Valve takes the
        // longer of the pair, so U must come from 160, not 128.
        let corners = [
            [0.0, 0.0, 0.0],
            [0.0, 64.0, 0.0],
            [160.0, 64.0, 0.0],
            [128.0, 0.0, 0.0],
        ];
        assert_eq!(
            luxel_dims_from_edges(&corners, &texinfo),
            Some([160 / 16 + 1, 64 / 16 + 1]),
        );

        // Degenerate lightmap vectors have no scale to divide by.
        let zeroed: TexInfo = bytemuck::Zeroable::zeroed();
        assert_eq!(luxel_dims_from_edges(&corners, &zeroed), None);
    }

    #[test]
    fn lerp2_matches_the_position_lerp_it_runs_alongside() {
        // The two must stay in step: a coordinate is interpolated with the same
        // parameter as the position it belongs to, so any divergence in the
        // formulas would slide the texture across the surface.
        let (a3, b3) = ([0.0, 10.0, 100.0], [1.0, 20.0, 200.0]);
        let (a2, b2) = ([0.0, 10.0], [1.0, 20.0]);
        for step in 0..=4 {
            let t = step as f32 / 4.0;
            let p = lerp(a3, b3, t);
            let c = lerp2(a2, b2, t);
            assert_eq!([p[0], p[1]], c, "position and coordinate lerps differ");
        }
    }

    #[test]
    fn face_quad_rejects_non_quads() {
        let positions = vec![DVertex { point: [0.0; 3] }; 4];
        let edges = vec![DEdge { v: [0, 1] }; 4];
        let surfedges = vec![0i32; 4];
        assert!(face_quad(0, 3, &surfedges, &edges, &positions).is_none());
        assert!(face_quad(0, 5, &surfedges, &edges, &positions).is_none());
        assert!(face_quad(0, 4, &surfedges, &edges, &positions).is_some());
        // Out-of-range firstedge must not panic.
        assert!(face_quad(900, 4, &surfedges, &edges, &positions).is_none());
    }
}
