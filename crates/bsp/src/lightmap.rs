//! Pack vrad's baked lightmaps into a single atlas.
//!
//! Source stores one small lightmap per face — a `w × h` grid of
//! [`ColorRgbExp32`] samples, typically under 32 luxels a side. Binding one
//! texture per face is unthinkable, so every face used by the geometry is
//! packed into one atlas and [`Vertex::lightmap_uv`] is rewritten from
//! face-local coordinates into atlas coordinates.
//!
//! # Where the samples live
//!
//! `dface_t::lightofs` is a **byte** offset into `LUMP_LIGHTING`, and it
//! already points *past* a `lightstyles * 4` byte block of average-face-colour
//! data that vrad writes immediately ahead of the samples
//! (`utils/vrad/lightmap.cpp:PrecompLightmapOffsets`). From there the layout is
//!
//! ```text
//! lightofs + (style * bumpSampleCount + bumpSample) * numluxels * 4
//! ```
//!
//! (`utils/vrad/radial.cpp:737`), with `bumpSampleCount = 4` when the face's
//! texinfo has `SURF_BUMPLIGHT` and 1 otherwise. Phase 1 reads **style 0,
//! bump sample 0** — the first `numluxels` samples at `lightofs` — which is
//! the unbumped, unswitched lighting the map ships with. Within a face,
//! samples are row-major with the S axis fastest: `luxel[s + t * width]`
//! (`lightmap.cpp:863`).
//!
//! Faces whose texinfo has `SURF_SKY` or `SURF_NOLIGHT` (vrad's `TEX_SPECIAL`)
//! were skipped by the compiler and have no samples at all, as are faces with
//! `styles[0] == 255` or a negative `lightofs`. Those are pointed at a
//! [`LightmapAtlas::fullbright`] block instead of being left to sample
//! whatever happens to sit at atlas origin.
//!
//! # Borders
//!
//! Each face's rect is padded by one pixel on every side, filled by
//! duplicating the edge luxels. A face's own UVs stay half a luxel inside its
//! interior, so bilinear taps land within the interior for *exact* arithmetic
//! — but displaced vertices project slightly outside their parent face's luxel
//! extent, and interpolation is not exact. The border is what turns those
//! cases into a harmless repeat of the edge colour instead of a neighbouring
//! face's lighting bleeding across the seam.
//!
//! [`ColorRgbExp32`]: crate::raw::ColorRgbExp32
//! [`Vertex::lightmap_uv`]: crate::geometry::Vertex::lightmap_uv

use crate::flags;
use crate::geometry::Surface;
use crate::raw::ColorRgbExp32;
use crate::{Bsp, Result};
use std::collections::HashMap;

/// Padding around each face's rect, in pixels. See the module docs.
const BORDER: u32 = 1;

/// Largest atlas edge this packer will produce.
///
/// WebGPU's floor for `maxTextureDimension2D` is 8192 and every desktop
/// backend meets it. The heaviest shipped TF2 map (`pl_redwood`) needs about
/// 7.4 M padded pixels, so it lands near 4096 × 2048 — this cap exists for
/// corrupt lumps and absurd community maps, not for anything Valve shipped.
pub const MAX_ATLAS_DIM: u32 = 8192;

/// Rejection threshold for a single face's luxel grid.
///
/// vbsp clamps authored lightmap scale so faces stay small; the largest across
/// all 233 shipped maps is 126 × 126. Anything past this is a misread offset
/// or a corrupt lump, and allocating for it would be worse than skipping it.
const MAX_FACE_DIM: i32 = 512;

/// Which set of baked samples to read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lighting {
    /// `LUMP_LIGHTING` — the LDR bake, present on every TF2 map.
    #[default]
    Ldr,
    /// `LUMP_LIGHTING_HDR`. Falls back to LDR when the map has no HDR bake.
    Hdr,
}

/// Where one face's samples ended up in the atlas, in pixels.
///
/// This is the **interior** rect: the one-pixel border sits just outside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightmapRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Counters for the build, for the HUD and the all-maps sweep.
#[derive(Clone, Copy, Debug, Default)]
pub struct LightmapStats {
    /// Faces given their own rect.
    pub faces_packed: usize,
    /// Faces the compiler never lit — `TEX_SPECIAL`, `styles[0] == 255`, or a
    /// negative `lightofs`. Pointed at the fullbright block.
    pub faces_unlit: usize,
    /// Faces whose sample range fell outside the lighting lump, or whose luxel
    /// grid was implausibly large. **A non-zero count here is a bug**, unlike
    /// [`Self::faces_unlit`].
    pub faces_bad_range: usize,
    /// Luxels copied out of the lighting lump.
    pub luxels: usize,
    /// Pixels occupied by rects and their borders.
    pub used_pixels: usize,
    /// Which lump the samples came from, after the HDR fallback.
    pub source: Lighting,
    /// Brightest single channel packed, before any exposure. Well above 1.0 on
    /// outdoor maps — the reason the atlas is float rather than 8-bit.
    pub peak: f32,
}

impl LightmapStats {
    /// Fraction of the atlas that is not wasted space.
    pub fn occupancy(&self, atlas_pixels: usize) -> f32 {
        if atlas_pixels == 0 {
            0.0
        } else {
            self.used_pixels as f32 / atlas_pixels as f32
        }
    }
}

/// One atlas holding every face's baked lighting, in linear RGB.
///
/// Values are **linear light** and routinely exceed 1.0, so this is kept as
/// `f32` and converted to a float texture format by the renderer rather than
/// being tonemapped here.
#[derive(Clone, Debug)]
pub struct LightmapAtlas {
    pub width: u32,
    pub height: u32,
    /// Row-major, `width * height` linear RGB triples.
    pub pixels: Vec<[f32; 3]>,
    /// Interior rect per FACES index.
    rects: HashMap<u32, LightmapRect>,
    /// A single white pixel for faces with no bake. Sampling anywhere inside
    /// it, borders included, gives 1.0.
    pub fullbright: LightmapRect,
    pub stats: LightmapStats,
}

impl LightmapAtlas {
    /// Interior rect for a face, or `None` if it was never packed.
    pub fn rect(&self, face: u32) -> Option<LightmapRect> {
        self.rects.get(&face).copied()
    }

    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Sample at integer atlas coordinates, for tests and the atlas dump.
    pub fn pixel(&self, x: u32, y: u32) -> [f32; 3] {
        if x >= self.width || y >= self.height {
            return [0.0; 3];
        }
        self.pixels[(y * self.width + x) as usize]
    }

    /// Rewrite every vertex's [`lightmap_uv`] from face-local to atlas space.
    ///
    /// Call this on the same surfaces whose faces were passed to [`build`] —
    /// including displacement surfaces, which carry their parent face's index
    /// in their chunks. Faces with no bake are aimed at [`Self::fullbright`].
    ///
    /// [`lightmap_uv`]: crate::geometry::Vertex::lightmap_uv
    pub fn remap(&self, surfaces: &mut [Surface]) {
        let (aw, ah) = (self.width as f32, self.height as f32);
        for surface in surfaces {
            for chunk in &surface.chunks {
                let rect = self.rect(chunk.face).unwrap_or(self.fullbright);
                let first = chunk.first_vertex as usize;
                let last = first + chunk.vertex_count as usize;
                let Some(vertices) = surface.vertices.get_mut(first..last) else {
                    continue;
                };
                for vertex in vertices {
                    let [u, v] = vertex.lightmap_uv;
                    // Face-local UVs are normalised to the luxel grid, so
                    // scaling by the rect's size lands on the same luxel
                    // centres. Clamping keeps a vertex that projects outside
                    // its parent face's extent — displacements do this — inside
                    // the duplicated border rather than in a neighbour's rect.
                    let px = (rect.x as f32 + u * rect.width as f32)
                        .clamp(rect.x as f32 - BORDER as f32, (rect.x + rect.width + BORDER) as f32);
                    let py = (rect.y as f32 + v * rect.height as f32)
                        .clamp(rect.y as f32 - BORDER as f32, (rect.y + rect.height + BORDER) as f32);
                    vertex.lightmap_uv = [px / aw, py / ah];
                }
            }
        }
    }
}

/// What one face needs from the atlas, resolved before packing.
struct Item {
    face: u32,
    width: u32,
    height: u32,
    /// Index of the first sample in the lighting slice.
    start: usize,
}

/// Pack the lightmaps of every face named in `faces`.
///
/// Duplicate indices are fine — each face is packed once. Faces the compiler
/// left unlit are counted and skipped rather than failing the build; only a
/// malformed lump is an error.
pub fn build(bsp: &Bsp, faces: impl IntoIterator<Item = u32>, lighting: Lighting) -> Result<LightmapAtlas> {
    let all_faces = bsp.faces()?;
    let texinfos = bsp.texinfos()?;

    // HDR is optional; a map compiled without it has an empty lump, and
    // silently returning a black atlas would look like a packing bug.
    let (samples, source) = match lighting {
        Lighting::Hdr => match bsp.lighting_hdr()? {
            hdr if !hdr.is_empty() => (hdr, Lighting::Hdr),
            _ => (bsp.lighting()?, Lighting::Ldr),
        },
        Lighting::Ldr => (bsp.lighting()?, Lighting::Ldr),
    };

    let mut stats = LightmapStats {
        source,
        ..Default::default()
    };
    let mut items: Vec<Item> = Vec::new();
    let mut seen: HashMap<u32, ()> = HashMap::new();

    for face_index in faces {
        if seen.insert(face_index, ()).is_some() {
            continue;
        }
        let Some(face) = all_faces.get(face_index as usize) else {
            stats.faces_bad_range += 1;
            continue;
        };
        let unlit = usize::try_from(face.texinfo)
            .ok()
            .and_then(|i| texinfos.get(i))
            .is_none_or(|ti| ti.flags & flags::TEX_SPECIAL != 0);
        if unlit || face.styles[0] == 255 || face.lightofs < 0 {
            stats.faces_unlit += 1;
            continue;
        }

        let (w, h) = (face.lightmap_width(), face.lightmap_height());
        if w <= 0 || h <= 0 || w > MAX_FACE_DIM || h > MAX_FACE_DIM {
            stats.faces_bad_range += 1;
            continue;
        }
        // `lightofs` counts bytes and every sample is 4 bytes wide; vrad only
        // ever advances it in whole samples, so a remainder means the offset
        // was misread.
        let byte_offset = face.lightofs as usize;
        if !byte_offset.is_multiple_of(size_of::<ColorRgbExp32>()) {
            stats.faces_bad_range += 1;
            continue;
        }
        let start = byte_offset / size_of::<ColorRgbExp32>();
        let count = (w * h) as usize;
        if start + count > samples.len() {
            stats.faces_bad_range += 1;
            continue;
        }

        items.push(Item {
            face: face_index,
            width: w as u32,
            height: h as u32,
            start,
        });
    }

    // The fullbright block is packed like any other item so it gets the same
    // border treatment; its samples are synthesised rather than read.
    let padded_area: u64 = items
        .iter()
        .map(|i| u64::from(i.width + 2 * BORDER) * u64::from(i.height + 2 * BORDER))
        .sum::<u64>()
        + u64::from(1 + 2 * BORDER).pow(2);
    let widest = items.iter().map(|i| i.width).max().unwrap_or(1) + 2 * BORDER;
    let width = choose_width(padded_area, widest);

    // Tallest first: shelves then hold near-uniform heights, which is what
    // makes a shelf packer competitive with a skyline one here.
    items.sort_by_key(|i| (std::cmp::Reverse(i.height), i.face));

    let mut shelf = Shelf::new(width);
    let fullbright = shelf.place(1, 1);
    let placed: Vec<(usize, LightmapRect)> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (i, shelf.place(item.width, item.height)))
        .collect();
    let height = shelf.height();

    if height > MAX_ATLAS_DIM {
        return Err(crate::BspError::LightmapAtlasTooLarge {
            width,
            height,
            max: MAX_ATLAS_DIM,
        });
    }

    let mut atlas = LightmapAtlas {
        width,
        height,
        pixels: vec![[0.0; 3]; (width as usize) * (height as usize)],
        rects: HashMap::with_capacity(items.len()),
        fullbright,
        stats,
    };

    atlas.blit(fullbright, |_, _| [1.0; 3]);
    atlas.stats.used_pixels += (1 + 2 * BORDER).pow(2) as usize;

    for (i, rect) in placed {
        let item = &items[i];
        let stride = item.width as usize;
        atlas.blit(rect, |x, y| {
            samples[item.start + y as usize * stride + x as usize].to_linear()
        });
        atlas.rects.insert(item.face, rect);
        atlas.stats.faces_packed += 1;
        atlas.stats.luxels += (item.width * item.height) as usize;
        atlas.stats.used_pixels +=
            ((item.width + 2 * BORDER) * (item.height + 2 * BORDER)) as usize;
        for k in 0..(item.width * item.height) as usize {
            let c = samples[item.start + k].to_linear();
            atlas.stats.peak = atlas.stats.peak.max(c[0]).max(c[1]).max(c[2]);
        }
    }

    Ok(atlas)
}

/// Every face that contributed geometry to `surfaces`, with duplicates.
///
/// Both brush and displacement surfaces record their source face per chunk, so
/// this is all [`build`] needs to know about the geometry.
pub fn faces_of(surfaces: &[Surface]) -> impl Iterator<Item = u32> + '_ {
    surfaces.iter().flat_map(|s| s.chunks.iter().map(|c| c.face))
}

impl LightmapAtlas {
    /// Write an interior rect and duplicate its edges into the border.
    #[allow(clippy::needless_range_loop)]
    fn blit(&mut self, rect: LightmapRect, sample: impl Fn(u32, u32) -> [f32; 3]) {
        for y in 0..rect.height {
            for x in 0..rect.width {
                let px = (rect.y + y) * self.width + rect.x + x;
                self.pixels[px as usize] = sample(x, y);
            }
        }
        // Clamp-extend outwards. Going one ring at a time means corners pick
        // up the already-written edge pixels, so a corner reads the nearest
        // interior luxel rather than staying black.
        let (x0, y0) = (rect.x as i64, rect.y as i64);
        let (x1, y1) = (x0 + rect.width as i64 - 1, y0 + rect.height as i64 - 1);
        let b = BORDER as i64;
        for y in (y0 - b)..=(y1 + b) {
            for x in (x0 - b)..=(x1 + b) {
                if (y0..=y1).contains(&y) && (x0..=x1).contains(&x) {
                    continue;
                }
                let sx = x.clamp(x0, x1);
                let sy = y.clamp(y0, y1);
                if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
                    continue;
                }
                let src = (sy as u32 * self.width + sx as u32) as usize;
                let dst = (y as u32 * self.width + x as u32) as usize;
                self.pixels[dst] = self.pixels[src];
            }
        }
    }
}

/// Atlas width: square-ish, a power of two, and never narrower than the widest
/// single face.
fn choose_width(padded_area: u64, widest_item: u32) -> u32 {
    let square = (padded_area as f64).sqrt().ceil() as u32;
    let width = square.max(widest_item).next_power_of_two();
    width.clamp(64, MAX_ATLAS_DIM)
}

/// A shelf (row) packer. Items arrive tallest-first, so each shelf's height is
/// set by its first item and the rest fit under it with little waste.
struct Shelf {
    width: u32,
    cursor_x: u32,
    shelf_y: u32,
    shelf_height: u32,
}

impl Shelf {
    fn new(width: u32) -> Self {
        Self {
            width,
            cursor_x: 0,
            shelf_y: 0,
            shelf_height: 0,
        }
    }

    /// Reserve space for a `width × height` interior plus its border, and
    /// return the interior rect.
    fn place(&mut self, width: u32, height: u32) -> LightmapRect {
        let (pw, ph) = (width + 2 * BORDER, height + 2 * BORDER);
        if self.cursor_x + pw > self.width && self.cursor_x > 0 {
            self.shelf_y += self.shelf_height;
            self.shelf_height = 0;
            self.cursor_x = 0;
        }
        let rect = LightmapRect {
            x: self.cursor_x + BORDER,
            y: self.shelf_y + BORDER,
            width,
            height,
        };
        self.cursor_x += pw;
        self.shelf_height = self.shelf_height.max(ph);
        rect
    }

    fn height(&self) -> u32 {
        self.shelf_y + self.shelf_height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelf_packer_wraps_and_stacks() {
        let mut shelf = Shelf::new(16);
        // 6x6 interiors are 8x8 padded, so two fit per 16-wide shelf.
        let a = shelf.place(6, 6);
        let b = shelf.place(6, 6);
        let c = shelf.place(6, 6);
        assert_eq!(a, LightmapRect { x: 1, y: 1, width: 6, height: 6 });
        assert_eq!(b, LightmapRect { x: 9, y: 1, width: 6, height: 6 });
        // Third wraps onto the next shelf, below the first one's border.
        assert_eq!(c, LightmapRect { x: 1, y: 9, width: 6, height: 6 });
        assert_eq!(shelf.height(), 16);
    }

    #[test]
    fn an_over_wide_item_still_gets_placed() {
        // A face wider than the atlas would otherwise loop forever or land
        // outside; `choose_width` prevents it, but the packer must not rely on
        // that to stay in bounds.
        let mut shelf = Shelf::new(8);
        let a = shelf.place(6, 2);
        let b = shelf.place(6, 2);
        assert_eq!(a.y, 1);
        assert_eq!(b.y, 5, "second item must start a new shelf");
    }

    #[test]
    fn chosen_width_is_square_ish_and_covers_the_widest_face() {
        // 1000x1000 worth of area wants a 1024-wide atlas.
        assert_eq!(choose_width(1_000_000, 8), 1024);
        // A single huge face forces the width up regardless of total area.
        assert_eq!(choose_width(100, 300), 512);
        // Tiny maps still get a usable floor.
        assert_eq!(choose_width(1, 1), 64);
    }

    /// Blit into a standalone atlas, so border behaviour can be checked
    /// without a BSP.
    fn test_atlas(width: u32, height: u32) -> LightmapAtlas {
        LightmapAtlas {
            width,
            height,
            pixels: vec![[0.0; 3]; (width * height) as usize],
            rects: HashMap::new(),
            fullbright: LightmapRect { x: 0, y: 0, width: 0, height: 0 },
            stats: LightmapStats::default(),
        }
    }

    #[test]
    fn blit_duplicates_edges_into_the_border() {
        let mut atlas = test_atlas(8, 8);
        let rect = LightmapRect { x: 2, y: 2, width: 3, height: 3 };
        // Distinct value per luxel so a wrong border can't coincide.
        atlas.blit(rect, |x, y| {
            let v = (y * 3 + x) as f32;
            [v, v, v]
        });

        // Interior intact.
        assert_eq!(atlas.pixel(2, 2)[0], 0.0);
        assert_eq!(atlas.pixel(4, 4)[0], 8.0);
        // Edges repeat the nearest interior luxel.
        assert_eq!(atlas.pixel(1, 3)[0], atlas.pixel(2, 3)[0]);
        assert_eq!(atlas.pixel(5, 3)[0], atlas.pixel(4, 3)[0]);
        assert_eq!(atlas.pixel(3, 1)[0], atlas.pixel(3, 2)[0]);
        assert_eq!(atlas.pixel(3, 5)[0], atlas.pixel(3, 4)[0]);
        // Corners take the diagonal interior luxel, not black.
        assert_eq!(atlas.pixel(1, 1)[0], 0.0);
        assert_eq!(atlas.pixel(5, 5)[0], 8.0);
        // Nothing written two pixels out.
        assert_eq!(atlas.pixel(0, 0)[0], 0.0);
        assert_eq!(atlas.pixel(6, 6)[0], 0.0);
    }

    #[test]
    fn blit_at_the_atlas_edge_does_not_wrap_or_panic() {
        let mut atlas = test_atlas(4, 4);
        // Interior touching x = 0 means the border falls outside the atlas.
        let rect = LightmapRect { x: 0, y: 0, width: 2, height: 2 };
        atlas.blit(rect, |_, _| [1.0; 3]);
        assert_eq!(atlas.pixel(0, 0), [1.0; 3]);
        assert_eq!(atlas.pixel(2, 0), [1.0; 3], "right border");
        // The clipped-away left border must not have wrapped to the far side.
        assert_eq!(atlas.pixel(3, 0), [0.0; 3]);
    }

    #[test]
    fn remap_maps_face_local_uvs_onto_the_rect() {
        use crate::geometry::{FaceChunk, Vertex};

        let mut atlas = test_atlas(64, 64);
        let rect = LightmapRect { x: 8, y: 16, width: 4, height: 2 };
        atlas.rects.insert(7, rect);
        atlas.fullbright = LightmapRect { x: 0, y: 0, width: 1, height: 1 };

        let vertex = |uv: [f32; 2]| Vertex {
            position: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
            uv: [0.0; 2],
            lightmap_uv: uv,
            alpha: 0.0,
        };
        let mut surfaces = vec![Surface {
            texdata: 0,
            flags: 0,
            // Face 7 is packed; face 9 is not, and must land on fullbright.
            vertices: vec![
                vertex([0.125, 0.25]),
                vertex([0.875, 0.75]),
                vertex([0.5, 0.5]),
            ],
            indices: vec![],
            chunks: vec![
                FaceChunk { face: 7, first_vertex: 0, vertex_count: 2 },
                FaceChunk { face: 9, first_vertex: 2, vertex_count: 1 },
            ],
        }];
        atlas.remap(&mut surfaces);

        // u = 0.125 over a 4-wide rect is the centre of its first luxel:
        // atlas x = 8 + 0.5, normalised by the 64-wide atlas.
        let v = &surfaces[0].vertices;
        assert!((v[0].lightmap_uv[0] - 8.5 / 64.0).abs() < 1e-6, "{:?}", v[0].lightmap_uv);
        assert!((v[0].lightmap_uv[1] - 16.5 / 64.0).abs() < 1e-6, "{:?}", v[0].lightmap_uv);
        assert!((v[1].lightmap_uv[0] - 11.5 / 64.0).abs() < 1e-6, "{:?}", v[1].lightmap_uv);
        assert!((v[1].lightmap_uv[1] - 17.5 / 64.0).abs() < 1e-6, "{:?}", v[1].lightmap_uv);
        // The unpacked face points at the fullbright pixel's interior.
        assert!((v[2].lightmap_uv[0] - 0.5 / 64.0).abs() < 1e-6, "{:?}", v[2].lightmap_uv);
    }

    #[test]
    fn remap_clamps_overshooting_uvs_into_the_border() {
        use crate::geometry::{FaceChunk, Vertex};

        let mut atlas = test_atlas(64, 64);
        let rect = LightmapRect { x: 10, y: 10, width: 4, height: 4 };
        atlas.rects.insert(1, rect);

        let vertex = |uv: [f32; 2]| Vertex {
            position: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
            uv: [0.0; 2],
            lightmap_uv: uv,
            alpha: 0.0,
        };
        let mut surfaces = vec![Surface {
            texdata: 0,
            flags: 0,
            // A displaced vertex whose projection lands well outside its
            // parent face's luxel extent.
            vertices: vec![vertex([-3.0, 4.0])],
            indices: vec![],
            chunks: vec![FaceChunk { face: 1, first_vertex: 0, vertex_count: 1 }],
        }];
        atlas.remap(&mut surfaces);

        let uv = surfaces[0].vertices[0].lightmap_uv;
        // Clamped to one pixel outside the interior — inside the duplicated
        // border, never into a neighbouring face's rect.
        assert!((uv[0] - 9.0 / 64.0).abs() < 1e-6, "{uv:?}");
        assert!((uv[1] - 15.0 / 64.0).abs() < 1e-6, "{uv:?}");
    }
}
