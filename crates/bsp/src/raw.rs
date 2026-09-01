//! `#[repr(C)]` transcriptions of the on-disk BSP structures.
//!
//! Every struct here is a byte-for-byte match for its counterpart in
//! `source-sdk-2013/src/public/bspfile.h`, so lumps can be reinterpreted in
//! place with no parsing pass. Two rules keep that safe:
//!
//! * Padding the C compiler inserts implicitly is written out as an explicit
//!   `_pad` field. `#[derive(Pod)]` refuses to compile a type with implicit
//!   padding, so this is checked rather than trusted.
//! * Every type carries a `const` size assertion against the stride measured
//!   from real TF2 maps (see `docs/PHASE1_MAP_VIEWER.md`).

use bytemuck::{Pod, Zeroable};

/// Source's `Vector`: three little-endian `f32` in Source's Z-up space.
pub type Vec3 = [f32; 3];

macro_rules! assert_size {
    ($t:ty, $n:expr) => {
        const _: () = assert!(
            ::core::mem::size_of::<$t>() == $n,
            concat!(stringify!($t), " has the wrong on-disk size"),
        );
    };
}

/// `lump_t` — one entry of the 64-slot lump directory.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Lump {
    pub fileofs: i32,
    pub filelen: i32,
    /// Lump format version. `LEAFS` in particular changes layout with this.
    pub version: i32,
    /// Non-zero means the lump body is LZMA-compressed and this is the
    /// inflated length. Was `char fourCC[4]`, repurposed for compression.
    pub uncompressed_size: i32,
}
assert_size!(Lump, 16);

pub const HEADER_LUMPS: usize = 64;

/// `dheader_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Header {
    /// Little-endian `"VBSP"`.
    pub ident: i32,
    pub version: i32,
    pub lumps: [Lump; HEADER_LUMPS],
    pub map_revision: i32,
}
assert_size!(Header, 1036);

/// `dplane_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DPlane {
    pub normal: Vec3,
    pub dist: f32,
    /// `PLANE_X`..`PLANE_ANYZ`; trivially regenerable, unused by the viewer.
    pub kind: i32,
}
assert_size!(DPlane, 20);

/// `dvertex_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DVertex {
    pub point: Vec3,
}
assert_size!(DVertex, 12);

/// `dedge_t`. Edge 0 is never used: negative surfedges encode reversal.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DEdge {
    pub v: [u16; 2],
}
assert_size!(DEdge, 4);

/// `dnode_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DNode {
    pub planenum: i32,
    /// Negative values are `-(leaf + 1)`, not node indices.
    pub children: [i32; 2],
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub firstface: u16,
    pub numfaces: u16,
    /// Area index if every leaf below shares one, else -1.
    pub area: i16,
    pub _pad: i16,
}
assert_size!(DNode, 32);

/// `texinfo_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct TexInfo {
    /// `[s|t][x y z offset]`, texels per world unit. Divide by the texture
    /// size from [`DTexData`] to get a 0..1 UV.
    pub texture_vecs: [[f32; 4]; 2],
    /// `[s|t][x y z offset]`, luxels per world unit.
    pub lightmap_vecs: [[f32; 4]; 2],
    /// `SURF_*` flags, see [`crate::flags`].
    pub flags: i32,
    /// Index into the TEXDATA lump.
    pub texdata: i32,
}
assert_size!(TexInfo, 72);

/// `dtexdata_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DTexData {
    pub reflectivity: Vec3,
    /// Index into the TEXDATA_STRING_TABLE lump.
    pub name_string_table_id: i32,
    /// Source image dimensions, the divisor for [`TexInfo::texture_vecs`].
    pub width: i32,
    pub height: i32,
    pub view_width: i32,
    pub view_height: i32,
}
assert_size!(DTexData, 32);

/// `dface_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DFace {
    pub planenum: u16,
    /// Non-zero when the face points opposite its plane's normal.
    pub side: u8,
    pub on_node: u8,
    /// Index into SURFEDGES, not EDGES.
    pub firstedge: i32,
    pub numedges: i16,
    /// Index into TEXINFO, or -1.
    pub texinfo: i16,
    /// Index into DISPINFO, or -1 for an ordinary brush face.
    pub dispinfo: i16,
    /// Only meaningful for fog volume (water surface) boundaries.
    pub surface_fog_volume_id: i16,
    /// Light styles; `styles[0] == 255` means the face has no lightmap.
    pub styles: [u8; 4],
    /// Byte offset into the LIGHTING lump, or -1. Note this already points
    /// past the per-style average-colour block vrad writes ahead of the
    /// samples (`utils/vrad/lightmap.cpp`).
    pub lightofs: i32,
    pub area: f32,
    pub lightmap_texture_mins_in_luxels: [i32; 2],
    /// Luxel extent; the sample grid is this plus one on each axis.
    pub lightmap_texture_size_in_luxels: [i32; 2],
    pub orig_face: i32,
    /// Top bit is a "dynamic shadows disabled" flag; use [`DFace::num_prims`].
    num_prims: u16,
    pub first_prim_id: u16,
    pub smoothing_groups: u32,
}
assert_size!(DFace, 56);

impl DFace {
    /// Non-polygon primitive count, with the shadow flag masked off.
    #[inline]
    pub fn num_prims(&self) -> u16 {
        self.num_prims & 0x7FFF
    }

    /// Mirrors `dface_t::AreDynamicShadowsEnabled`.
    #[inline]
    pub fn dynamic_shadows_enabled(&self) -> bool {
        self.num_prims & 0x8000 == 0
    }

    /// Lightmap sample grid width in luxels.
    #[inline]
    pub fn lightmap_width(&self) -> i32 {
        self.lightmap_texture_size_in_luxels[0] + 1
    }

    /// Lightmap sample grid height in luxels.
    #[inline]
    pub fn lightmap_height(&self) -> i32 {
        self.lightmap_texture_size_in_luxels[1] + 1
    }
}

/// `dleaf_t`, lump version 1 — the layout every TF2 map uses.
///
/// Version 0 is 56 bytes because it carries an inline `CompressedLightCube`;
/// see [`DLeafV0`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DLeaf {
    pub contents: i32,
    pub cluster: i16,
    /// Bitfield: `area:9`, `flags:7`. Use [`DLeaf::area`] / [`DLeaf::flags`].
    area_flags: u16,
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub firstleafface: u16,
    pub numleaffaces: u16,
    pub firstleafbrush: u16,
    pub numleafbrushes: u16,
    /// -1 when the leaf is not in water.
    pub leaf_water_data_id: i16,
    pub _pad: i16,
}
assert_size!(DLeaf, 32);

impl DLeaf {
    #[inline]
    pub fn area(&self) -> u16 {
        self.area_flags & 0x01FF
    }

    #[inline]
    pub fn flags(&self) -> u16 {
        self.area_flags >> 9
    }
}

/// `dleaf_t` at lump version 0 (`dleaf_version_0_t`, `bspfile.h:799`):
/// [`DLeaf`]'s fields plus an inline 24-byte `CompressedLightCube`. Present
/// only in pre-2006 maps; the SDK's `dm_lockdown.bsp` is the one available
/// sample.
///
/// **The fields are spelled out rather than nesting [`DLeaf`], because the
/// light cube does not start where a nested struct would put it.** `DLeaf` is
/// 32 bytes *including two trailing pad bytes*, but in C the cube follows
/// `leafWaterDataID` immediately: `ColorRGBExp32` is four `u8`-sized members,
/// so `CompressedLightCube` has alignment 1 and no padding can precede it. It
/// begins at byte **30**, and the two pad bytes land at the *end* instead.
///
/// Both layouts are 56 bytes, so neither [`assert_size`] nor a stride check can
/// tell them apart — which is why [`OFFSET_OF_AMBIENT_LIGHTING`] is asserted
/// separately. Measured on `dm_lockdown.bsp`'s 2725 leaves: bytes 30..32 hold
/// 1950 distinct values, while bytes 54..56 are `0x0000` on every single leaf.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DLeafV0 {
    pub contents: i32,
    pub cluster: i16,
    /// Bitfield: `area:9`, `flags:7`. Use [`DLeafV0::area`] / [`DLeafV0::flags`].
    area_flags: u16,
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub firstleafface: u16,
    pub numleaffaces: u16,
    pub firstleafbrush: u16,
    pub numleafbrushes: u16,
    /// -1 when the leaf is not in water.
    pub leaf_water_data_id: i16,
    /// `CompressedLightCube`: six `ColorRGBExp32`, one per axis direction.
    /// Starts at byte 30 — see the type docs.
    pub ambient_lighting: [ColorRgbExp32; 6],
    /// Trailing padding to the struct's 4-byte alignment. Always zero in the
    /// maps measured.
    pub _pad: [u8; 2],
}
assert_size!(DLeafV0, 56);

/// Where the v0 light cube begins. Asserted below; see [`DLeafV0`].
pub const OFFSET_OF_AMBIENT_LIGHTING: usize = 30;

const _: () = assert!(
    ::core::mem::offset_of!(DLeafV0, ambient_lighting) == OFFSET_OF_AMBIENT_LIGHTING,
    "the v0 light cube follows leafWaterDataID with no padding; nesting DLeaf      would push it to 32 and silently shift every channel",
);

impl DLeafV0 {
    /// The fields this layout shares with [`DLeaf`], for code that does not
    /// care which lump version it is reading.
    pub fn leaf(&self) -> DLeaf {
        DLeaf {
            contents: self.contents,
            cluster: self.cluster,
            area_flags: self.area_flags,
            mins: self.mins,
            maxs: self.maxs,
            firstleafface: self.firstleafface,
            numleaffaces: self.numleaffaces,
            firstleafbrush: self.firstleafbrush,
            numleafbrushes: self.numleafbrushes,
            leaf_water_data_id: self.leaf_water_data_id,
            _pad: 0,
        }
    }

    #[inline]
    pub fn area(&self) -> u16 {
        self.area_flags & 0x01FF
    }

    #[inline]
    pub fn flags(&self) -> u16 {
        self.area_flags >> 9
    }
}

/// `dmodel_t`. Model 0 is worldspawn; 1.. are brush entities, referenced by
/// an entity's `model "*N"` key.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DModel {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub origin: Vec3,
    pub headnode: i32,
    pub firstface: i32,
    pub numfaces: i32,
}
assert_size!(DModel, 48);

/// `dbrushside_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DBrushSide {
    pub planenum: u16,
    pub texinfo: i16,
    pub dispinfo: i16,
    pub bevel: i16,
}
assert_size!(DBrushSide, 8);

/// `dbrush_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DBrush {
    pub firstside: i32,
    pub numsides: i32,
    /// `CONTENTS_*` flags, see [`crate::flags`].
    pub contents: i32,
}
assert_size!(DBrush, 12);

/// `CDispSubNeighbor`. Unused by the viewer, but part of [`DDispInfo`]'s
/// layout so it has to be exact.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DispSubNeighbor {
    /// Index into DISPINFO, or `0xFFFF` for no neighbour.
    pub neighbor: u16,
    pub neighbor_orientation: u8,
    pub span: u8,
    pub neighbor_span: u8,
    pub _pad: u8,
}
assert_size!(DispSubNeighbor, 6);

/// `CDispNeighbor` — one displacement edge, up to two neighbours.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DispNeighbor {
    pub sub_neighbors: [DispSubNeighbor; 2],
}
assert_size!(DispNeighbor, 12);

/// `CDispCornerNeighbors`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DispCornerNeighbors {
    pub neighbors: [u16; 4],
    pub num_neighbors: u8,
    pub _pad: u8,
}
assert_size!(DispCornerNeighbors, 10);

/// `MAX_DISPVERTS` padded to a word count: `PAD_NUMBER(289, 32) / 32`.
pub const ALLOWEDVERTS_SIZE: usize = 10;

/// `ddispinfo_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DDispInfo {
    /// Orients the grid: the parent face corner nearest this point is (0,0).
    pub start_position: Vec3,
    /// Index into DISP_VERTS.
    pub disp_vert_start: i32,
    /// Index into DISP_TRIS.
    pub disp_tri_start: i32,
    /// 2..=4; the vertex grid is `(1 << power) + 1` per side.
    pub power: i32,
    pub min_tess: i32,
    pub smoothing_angle: f32,
    /// `CONTENTS_*` flags.
    pub contents: i32,
    /// The face this displacement is built on.
    pub map_face: u16,
    pub _pad: u16,
    pub lightmap_alpha_start: i32,
    pub lightmap_sample_position_start: i32,
    /// Indexed by `NEIGHBOREDGE_*`.
    pub edge_neighbors: [DispNeighbor; 4],
    /// Indexed by `CORNER_*`.
    pub corner_neighbors: [DispCornerNeighbors; 4],
    pub allowed_verts: [u32; ALLOWEDVERTS_SIZE],
}
assert_size!(DDispInfo, 176);

impl DDispInfo {
    /// Vertices per grid side: `(1 << power) + 1` → 5, 9 or 17.
    #[inline]
    pub fn side_len(&self) -> usize {
        (1usize << self.power) + 1
    }

    /// `NUM_DISP_POWER_VERTS(power)` → 25, 81 or 289.
    #[inline]
    pub fn num_verts(&self) -> usize {
        self.side_len() * self.side_len()
    }

    /// `NUM_DISP_POWER_TRIS(power)` → 32, 128 or 512.
    #[inline]
    pub fn num_tris(&self) -> usize {
        (1usize << self.power) * (1usize << self.power) * 2
    }
}

/// `CDispVert` — one displacement grid vertex.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DispVert {
    /// Unit direction the vertex is pushed along.
    pub vector: Vec3,
    /// Distance along [`DispVert::vector`].
    pub dist: f32,
    /// Blend weight between `$basetexture` and `$basetexture2`.
    pub alpha: f32,
}
assert_size!(DispVert, 20);

/// `CDispTri` — `DISPTRI_*` tag bits per displacement triangle.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DispTri {
    pub tags: u16,
}
assert_size!(DispTri, 2);

/// `ColorRGBExp32` — one lightmap luxel. The RGB bytes are **linear**, scaled
/// by `2^exponent`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ColorRgbExp32 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub exponent: i8,
}
assert_size!(ColorRgbExp32, 4);

impl ColorRgbExp32 {
    /// Decode to linear RGB, mirroring `ColorRGBExp32ToVector`.
    #[inline]
    pub fn to_linear(self) -> [f32; 3] {
        let scale = (self.exponent as f32).exp2() / 255.0;
        [
            self.r as f32 * scale,
            self.g as f32 * scale,
            self.b as f32 * scale,
        ]
    }
}

/// `dcubemapsample_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DCubemapSample {
    pub origin: [i32; 3],
    pub size: u8,
    pub _pad: [u8; 3],
}
assert_size!(DCubemapSample, 16);

/// `dgamelump_t`. The GAME_LUMP body is an `i32` count followed by these.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DGameLump {
    /// Four-CC, e.g. `sprp` for static props.
    pub id: i32,
    /// `GAMELUMPFLAG_COMPRESSED` is bit 0.
    pub flags: u16,
    pub version: u16,
    pub fileofs: i32,
    pub filelen: i32,
}
assert_size!(DGameLump, 16);


#[cfg(test)]
mod tests {
    use super::*;

    /// A v0 leaf's light cube starts at byte 30, so a cast must read it there.
    ///
    /// Nesting [`DLeaf`] instead puts it at 32: still 56 bytes total, still an
    /// exact stride divisor, but every channel shifted two bytes and the last
    /// two read from the trailing padding. Only an offset check catches that,
    /// which is what this test and the `offset_of!` assertion are for.
    #[test]
    fn the_v0_light_cube_starts_at_byte_30_not_32() {
        let mut bytes = [0u8; 56];
        // leafWaterDataID = -1, the last field before the cube.
        bytes[28..30].copy_from_slice(&(-1i16).to_le_bytes());
        // Fill the cube with a recognisable ramp at its real offset.
        for (i, b) in bytes[30..54].iter_mut().enumerate() {
            *b = i as u8 + 1;
        }

        let leaf: &DLeafV0 = bytemuck::from_bytes(&bytes);

        assert_eq!(leaf.leaf_water_data_id, -1);
        // First sample is bytes 30..34, last is 50..54.
        assert_eq!(
            (
                leaf.ambient_lighting[0].r,
                leaf.ambient_lighting[0].g,
                leaf.ambient_lighting[0].b,
                leaf.ambient_lighting[0].exponent,
            ),
            (1, 2, 3, 4),
        );
        assert_eq!(leaf.ambient_lighting[5].exponent, 24);
        // The two spare bytes are at the end, where dm_lockdown's are zero.
        assert_eq!(leaf._pad, [0, 0]);
    }

    /// The shared fields must land at the same offsets in both lump versions,
    /// or `leaf()` would be reinterpreting rather than copying.
    #[test]
    fn both_leaf_versions_agree_on_the_fields_they_share() {
        let mut bytes = [0u8; 56];
        bytes[0..4].copy_from_slice(&1i32.to_le_bytes()); // contents
        bytes[4..6].copy_from_slice(&7i16.to_le_bytes()); // cluster
        // area = 300 (9 bits), flags = 5 (7 bits) -> the packed word.
        bytes[6..8].copy_from_slice(&(300u16 | (5u16 << 9)).to_le_bytes());
        bytes[20..22].copy_from_slice(&11u16.to_le_bytes()); // firstleafface
        bytes[28..30].copy_from_slice(&(-1i16).to_le_bytes());

        let v0: &DLeafV0 = bytemuck::from_bytes(&bytes);
        let v1: &DLeaf = bytemuck::from_bytes(&bytes[..32]);

        assert_eq!(v0.leaf().contents, v1.contents);
        assert_eq!(v0.leaf().cluster, v1.cluster);
        assert_eq!(v0.leaf().firstleafface, v1.firstleafface);
        assert_eq!((v0.area(), v0.flags()), (300, 5));
        assert_eq!((v0.area(), v0.flags()), (v1.area(), v1.flags()));
    }
}
