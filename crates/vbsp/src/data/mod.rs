mod displacement;
mod entity;
mod game;
mod leaves;
pub mod lighting;
mod prop;

pub use self::displacement::*;
pub use self::entity::*;
pub use self::game::*;
pub use self::leaves::*;
use crate::BspError;
use crate::bspfile::LumpType;
use crate::visdata::calculate_visdata_indices;
use crate::{BspResult, StringError};
use arrayvec::ArrayString;
use binrw::error::CustomError;
use binrw::{BinRead, BinResult, Endian};
use bitflags::bitflags;
use glam::IVec2;
use glam::UVec2;
use glam::Vec2;
use glam::Vec3;
use itertools::Either;
use num_enum::{TryFromPrimitive, TryFromPrimitiveError};
use std::borrow::Cow;
use std::fmt;
use std::fmt::{Debug, Display, Formatter};
use std::io::{Cursor, Read, Seek};
use std::mem::size_of;
use std::ops::Deref;
use std::ops::Index;
use std::sync::Mutex;
pub use vbsp_common::{Angles, Color, EntityProp, LightColor, Negated, PropPlacement};
use zip::ZipArchive;
use zip::result::ZipError;

/// Validate that reading the type consumes `size_of::<T>()` bytes
#[cfg(test)]
fn test_read_bytes<T: BinRead>()
where
    T::Args<'static>: Default,
    <T as BinRead>::Args<'static>: Clone,
{
    test_read_bytes_args::<T>(T::Args::default())
}

#[cfg(test)]
fn test_read_bytes_args<T: BinRead>(args: T::Args<'static>)
where
    <T as BinRead>::Args<'static>: Clone,
{
    use binrw::BinReaderExt;
    use std::any::type_name;

    let bytes = [0; 512];
    let mut reader = Cursor::new(bytes);

    let _ = reader.read_le_args::<T>(args).unwrap();

    assert_eq!(
        reader.position() as usize,
        size_of::<T>(),
        "Invalid number of bytes used to read {}",
        type_name::<T>()
    );
}

#[derive(Clone, BinRead, Debug)]
pub struct Directories {
    entries: [LumpEntry; 64],
}

impl Directories {
    /// Checks if the lump directory seems to use the L4D2 lump header order.
    ///
    /// L4D2 (BSP v21) changed the order of fields in `lump_t` compared to previous versions.
    /// This function uses heuristics to detect this altered order.
    /// It should be called only after `Directories` has been read assuming the standard (pre-L4D2) order.
    ///
    /// # Arguments
    ///
    /// * `file_size` - The total size of the BSP file in bytes. Used by one of the heuristics.
    ///
    /// # Returns
    ///
    /// * `true` if the L4D2 lump order is detected.
    /// * `false` if the standard lump order is detected or detection is inconclusive.
    pub fn is_l4d2_lump_order(&self, file_size: usize) -> bool {
        // Heuristic 1: Check lump versions (read into `offset` field assuming standard order).
        // Real lump versions are small integers (usually 0 or 1).
        // Real file offsets are usually large and 4-byte aligned.
        let mut maybe_l4d2 = false;

        for lump in self.entries.iter() {
            // If the value read into `offset` is large, it's likely a real offset.
            // Assume any value > 20 is not a lump version. Max known lump version is much lower.
            if lump.offset > 20 {
                return false; // Can confidently say it's standard order.
            }
            // If it's a potential version (<= 20) and it's non-zero and not 4-byte aligned,
            // it strongly suggests it's a real version read into the wrong field (L4D2 order).
            if lump.offset != 0 && lump.offset % 4 != 0 {
                maybe_l4d2 = true;
            }
        }

        if maybe_l4d2 {
            return true; // Found evidence for L4D2 order and no counter-evidence.
        }

        // Heuristic 2 (Fallback): Check lump offsets (read into `length` field assuming standard order).
        // If any "length" value is larger than the file size, it's almost certainly a file offset.
        if self
            .entries
            .iter()
            .any(|lump| lump.length as usize > file_size)
        {
            return true;
        }

        // If no heuristic triggered, assume standard order.
        false
    }

    /// Corrects the fields of all `LumpEntry` structs if they were read using the standard
    /// order but are actually in L4D2 order.
    ///
    /// This should only be called after `is_l4d2_lump_order` returns `true`.
    pub fn fixup_lumps(&mut self) {
        self.entries.iter_mut().for_each(LumpEntry::fixup_l4d2)
    }
}

impl Index<LumpType> for Directories {
    type Output = LumpEntry;

    fn index(&self, index: LumpType) -> &Self::Output {
        &self.entries[index as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BinRead)]
#[br(little)]
#[brw(repr=u32)]
#[non_exhaustive]
pub enum BspVersion {
    Version19 = 19,
    Version20 = 20,
    Version21 = 21,
    // Portal revolution
    Version25 = 25,
}

#[derive(Debug, Clone, PartialEq, Eq, BinRead)]
#[br(little)]
pub struct Header {
    #[brw(magic = b"VBSP")]
    pub version: BspVersion,
}

#[derive(Clone, Copy, Debug)]
pub struct LumpArgs {
    pub type_: LumpType,
    pub length: usize,
    pub version: u32,
}

#[derive(Clone, Copy, Debug, Default, BinRead)]
#[br(little)]
pub struct LumpEntry {
    pub offset: u32,
    pub length: u32,
    pub version: u32,
    pub ident: u32,
}

impl LumpEntry {
    /// handle l4d2 which uses {version, offset, length, ident} instead
    pub fn fixup_l4d2(&mut self) {
        *self = LumpEntry {
            offset: self.length,
            length: self.version,
            version: self.offset,
            ident: self.ident,
        }
    }
}

#[derive(Debug, Clone, BinRead)]
pub struct LeafFace {
    pub face: u16,
}

#[derive(BinRead, Debug, Clone, Copy)]
pub struct TextureFlags(u32);

bitflags! {
    impl TextureFlags: u32 {
        const LIGHT      = 0b0000_0000_0000_0000_0001; // value will hold the light strength
        const SKY2D      = 0b0000_0000_0000_0000_0010; // don't draw, indicate we should skylight + draw 2d sky but don't draw the 3d skybox
        const SKY        = 0b0000_0000_0000_0000_0100; // don't draw, but add the skybox
        const WARP       = 0b0000_0000_0000_0000_1000; // turbulent water warp
        const TRANS      = 0b0000_0000_0000_0001_0000; // texture is translucent
        const NOPORTAL   = 0b0000_0000_0000_0010_0000; // the surface can't have a portal placed on it
        const TRIGGER    = 0b0000_0000_0000_0100_0000; // xbox hack to work around elimination of trigger surfaces
        const NODRAW     = 0b0000_0000_0000_1000_0000; // don't bother referencing the texture
        const HINT       = 0b0000_0000_0001_0000_0000; // make a primary bsp splitter
        const SKIP       = 0b0000_0000_0010_0000_0000; // completely ignore, allowing non-closed brushes
        const NOLIGHT    = 0b0000_0000_0100_0000_0000; // dont calculate light
        const BUMPLIGHT  = 0b0000_0000_1000_0000_0000; // calculate thee light maps for the surface for bump mapping
        const NOSHADOWS  = 0b0000_0001_0000_0000_0000; // don't receive shadows
        const NODECALS   = 0b0000_0010_0000_0000_0000; // don't receive decals
        const NOCHOP     = 0b0000_0100_0000_0000_0000; // don't subdivide patches on this surface
        const HITBOX     = 0b0000_1000_0000_0000_0000; // surface is part of a hitbox
    }
}

/// Fixed length, null-terminated string
#[derive(Debug, Clone)]
pub struct FixedString<const LEN: usize>(ArrayString<LEN>);

impl<const N: usize> AsRef<str> for FixedString<N> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<const N: usize> FixedString<N> {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<const LEN: usize> Display for FixedString<LEN> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl<const LEN: usize> BinRead for FixedString<LEN> {
    type Args<'a> = ();

    fn read_options<R: Read + binrw::io::Seek>(
        reader: &mut R,
        endian: Endian,
        args: Self::Args<'static>,
    ) -> BinResult<Self> {
        use std::str;

        let start = reader.stream_position().unwrap();

        let name_buf = <[u8; LEN]>::read_options(reader, endian, args)?;

        let zero_pos =
            name_buf
                .iter()
                .position(|c| *c == 0)
                .ok_or_else(|| binrw::Error::Custom {
                    pos: start,
                    err: Box::new(StringError::NotNullTerminated),
                })?;
        let name = &name_buf[..zero_pos];
        Ok(FixedString(
            ArrayString::from(
                str::from_utf8(name)
                    .map_err(StringError::NonUTF8)
                    .map_err(|e| binrw::Error::Custom {
                        pos: start,
                        err: Box::new(e),
                    })?,
            )
            .expect(
                "Programmer error: it should be impossible for the string to exceed the capacity",
            ),
        ))
    }
}

#[derive(Debug, Clone, BinRead)]
pub struct TextureCoord {
    #[br(map = |vals: [f32; 3]| vals.into())]
    pub axis: Vec3,
    pub offset: f32,
}

impl TextureCoord {
    pub fn project(&self, position: Vec3) -> f32 {
        (self.axis.as_dvec3().dot(position.as_dvec3()) + self.offset as f64) as f32
    }
}

#[derive(Debug, Clone, BinRead)]
pub struct PlanarTextureCoords {
    pub u: TextureCoord,
    pub v: TextureCoord,
}

impl PlanarTextureCoords {
    pub fn project(&self, position: Vec3) -> Vec2 {
        Vec2::new(self.u.project(position), self.v.project(position))
    }
}

#[derive(Debug, Clone, BinRead)]
pub struct TextureInfo {
    pub texture_transforms: PlanarTextureCoords,
    pub lightmap_transforms: PlanarTextureCoords,
    pub flags: TextureFlags,
    pub texture_data_index: i32,
}

static_assertions::const_assert_eq!(size_of::<TextureInfo>(), 72);

#[derive(Debug, Clone, BinRead)]
pub struct TextureData {
    #[br(map = |vals: [f32; 3]| vals.into())]
    pub reflectivity: Vec3,
    pub name_string_table_id: i32,
    pub width: i32,
    pub height: i32,
    pub view_width: i32,
    pub view_height: i32,
}

#[derive(Debug, Clone, BinRead)]
pub struct Plane {
    #[br(map = |vals: [f32; 3]| vals.into())]
    pub normal: Vec3,
    pub dist: f32,
    pub ty: i32,
}

impl Plane {
    pub fn normal(&self) -> Vec3 {
        match self.ty {
            0 => Vec3::X,
            1 => Vec3::Y,
            2 => Vec3::Z,
            _ => self.normal,
        }
    }
}

#[derive(Debug, Clone, BinRead)]
pub struct NodeV0 {
    pub plane_index: i32,
    pub children: [i32; 2],
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub first_face: u16,
    pub face_count: u16,
    pub area: i16,
    pub _padding: i16,
}

static_assertions::const_assert_eq!(size_of::<NodeV0>(), 32);

#[derive(Debug, Clone, BinRead)]
pub struct Node {
    pub plane_index: i32,
    pub children: [i32; 2],
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub first_face: u32,
    pub face_count: u32,
    pub area: i16,
    pub _padding: i16,
}

static_assertions::const_assert_eq!(size_of::<Node>(), 48);

impl From<NodeV0> for Node {
    fn from(value: NodeV0) -> Self {
        Self {
            plane_index: value.plane_index,
            children: value.children,
            mins: value.mins.map(|val| val as _),
            maxs: value.maxs.map(|val| val as _),
            first_face: value.first_face as _,
            face_count: value.face_count as _,
            area: value.area,
            _padding: value._padding,
        }
    }
}

#[derive(Debug, Clone, BinRead)]
pub struct LeafBrush {
    pub brush: u16,
}

#[derive(Debug, Clone, BinRead)]
pub struct Model {
    #[br(map = |vals: [f32; 3]| vals.into())]
    pub mins: Vec3,
    #[br(map = |vals: [f32; 3]| vals.into())]
    pub maxs: Vec3,
    #[br(map = |vals: [f32; 3]| vals.into())]
    pub origin: Vec3,
    pub head_node: i32,
    pub first_face: u32,
    pub face_count: u32,
}

static_assertions::const_assert_eq!(size_of::<Model>(), 48);

#[derive(Debug, Clone, BinRead)]
pub struct Brush {
    pub brush_side: u32,
    pub num_brush_sides: u32,
    pub flags: BrushFlags,
}

impl Brush {
    pub fn is_visible(&self) -> bool {
        self.flags.intersects(
            BrushFlags::SOLID
                | BrushFlags::GRATE
                | BrushFlags::OPAQUE
                | BrushFlags::TESTFOGVOLUME
                | BrushFlags::TRANSLUCENT,
        )
    }
}

#[derive(BinRead, Debug, Clone, Copy)]
pub struct BrushFlags(u32);

bitflags! {
    impl BrushFlags: u32 {
        const EMPTY =       	        0; // 	No contents
        const SOLID =       	        0x1; // 	an eye is never valid in a solid
        const WINDOW =      	        0x2; // 	translucent, but not watery (glass)
        const AUX =         	        0x4;
        const GRATE =       	        0x8; // 	alpha-tested "grate" textures. Bullets/sight pass through, but solids don't
        const SLIME =       	        0x10;
        const WATER =       	        0x20;
        const MIST =        	        0x40;
        const OPAQUE =      	        0x80; // 	block AI line of sight
        const TESTFOGVOLUME =          0x100; // 	things that cannot be seen through (may be non-solid though)
        const UNUSED =      	        0x200; // 	unused
        const UNUSED6 =                0x400; // 	unused
        const TEAM1 =       	        0x800; // 	per team contents used to differentiate collisions between players and objects on different teams
        const TEAM2 =       	        0x1000;
        const IGNORE_NODRAW_OPAQUE =   0x2000; // 	ignore CONTENTS_OPAQUE on surfaces that have SURF_NODRAW
        const MOVEABLE =               0x4000; // 	hits entities which are MOVETYPE_PUSH (doors, plats, etc.)
        const AREAPORTAL =             0x8000; // 	remaining contents are non-visible, and don't eat brushes
        const PLAYERCLIP =             0x10000;
        const MONSTERCLIP =            0x20000;
        const CURRENT_0 =              0x40000; // 	currents can be added to any other contents, and may be mixed
        const CURRENT_90 =             0x80000;
        const CURRENT_180 =            0x100000;
        const CURRENT_270 =            0x200000;
        const CURRENT_UP =             0x400000;
        const CURRENT_DOWN =           0x800000;
        const ORIGIN =      	        0x1000000; // 	removed before bsping an entity
        const MONSTER =                0x2000000; // 	should never be on a brush, only in game
        const DEBRIS =      	        0x4000000;
        const DETAIL =      	        0x8000000; // 	brushes to be added after vis leafs
        const TRANSLUCENT =            0x10000000; // 	auto set if any surface has trans
        const LADDER =      	        0x20000000;
        const HITBOX =      	        0x40000000; // 	use accurate hitboxes on trace
    }
}

#[derive(Debug, Clone, BinRead)]
pub struct BrushSide {
    pub plane: u16,
    pub texture_info: i16,
    pub displacement_info: i16,
    pub bevel: i16,
}

#[derive(Debug, Clone, BinRead, PartialEq)]
pub struct Vertex {
    #[br(map = |vals: [f32; 3]| vals.into())]
    pub position: Vec3,
}

#[derive(Debug, Clone, BinRead)]
pub struct Edge {
    pub start_index: u16,
    pub end_index: u16,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum EdgeDirection {
    FirstToLast,
    LastToFirst,
}

#[derive(Debug, Clone, BinRead)]
pub struct SurfaceEdge {
    edge: i32,
}

impl SurfaceEdge {
    pub fn edge_index(&self) -> u32 {
        self.edge.unsigned_abs()
    }

    pub fn direction(&self) -> EdgeDirection {
        if self.edge >= 0 {
            EdgeDirection::FirstToLast
        } else {
            EdgeDirection::LastToFirst
        }
    }
}

#[derive(Debug, Clone, BinRead)]
pub struct FaceV1 {
    pub plane_num: u16,
    pub side: u8,
    pub on_node: u8,
    pub first_edge: i32,
    pub num_edges: i16,
    pub texture_info: i16,
    pub displacement_info: i16,
    pub surface_fog_volume_id: i16,
    pub styles: [u8; 4],
    pub light_offset: i32,
    pub area: f32,
    #[br(map = <[i32; 2]>::into)]
    pub light_map_texture_min: IVec2,
    #[br(map = <[u32; 2]>::into)]
    pub light_map_texture_size: UVec2,
    pub original_face: i32,
    pub primitive_count: u16,
    pub first_primitive_index: u16,
    pub smoothing_groups: u32,
}

impl FaceV1 {
    pub fn displacement_index(&self) -> Option<i16> {
        (self.displacement_info >= 0).then_some(self.displacement_info)
    }
}

#[derive(Debug, Clone, BinRead)]
pub struct PackedPrimitiveCount(u32);

impl PackedPrimitiveCount {
    const ENABLE_SHADOWS_MASK: u32 = 1 << 31;

    pub fn primitive_count(&self) -> u32 {
        self.0 & !Self::ENABLE_SHADOWS_MASK
    }

    pub fn enable_shadows(&self) -> bool {
        (self.0 & Self::ENABLE_SHADOWS_MASK) != 0
    }

    pub fn pack(primitive_count: u32, enable_shadows: bool) -> Self {
        let flag = if enable_shadows {
            Self::ENABLE_SHADOWS_MASK
        } else {
            0
        };

        assert_eq!(primitive_count & Self::ENABLE_SHADOWS_MASK, 0);

        Self(primitive_count | flag)
    }
}

#[derive(Debug, Clone, BinRead)]
pub struct Face {
    pub plane_num: u32,
    pub side: u8,
    pub on_node: u8,
    pub first_edge: i32,
    pub num_edges: i32,
    pub texture_info: i32,
    pub displacement_info: i32,
    pub surface_fog_volume_id: i32,
    pub styles: [u8; 4],
    pub light_offset: i32,
    pub area: f32,
    #[br(map = <[i32; 2]>::into)]
    pub light_map_texture_min: IVec2,
    #[br(map = <[u32; 2]>::into)]
    pub light_map_texture_size: UVec2,
    pub original_face: i32,
    pub primitive_count: PackedPrimitiveCount,
    pub first_primitive_index: u32,
    pub smoothing_groups: u32,
}

static_assertions::const_assert_eq!(size_of::<TextureInfo>(), 72);

#[derive(Debug, Clone)]
pub struct Faces {
    pub faces: Vec<Face>,
}

impl Deref for Faces {
    type Target = [Face];

    fn deref(&self) -> &Self::Target {
        &*self.faces
    }
}

impl BinRead for Faces {
    type Args<'a> = LumpArgs;

    fn read_options<R: Read + Seek>(
        reader: &mut R,
        endian: Endian,
        args: Self::Args<'_>,
    ) -> BinResult<Self> {
        let item_size = match args.version {
            // TODO: Is FaceV0 different?
            0 | 1 => size_of::<FaceV1>(),
            2 => size_of::<Face>(),
            version => {
                return Err(binrw::Error::Custom {
                    err: Box::new(BspError::LumpVersion(
                        crate::error::UnsupportedLumpVersion {
                            lump_type: "faces",
                            version: version as u16,
                        },
                    )),
                    pos: reader.stream_position().unwrap(),
                });
            }
        };
        if args.length % item_size != 0 {
            return Err(binrw::Error::Custom {
                err: Box::new(BspError::InvalidLumpSize {
                    lump: args.type_,
                    element_size: item_size,
                    lump_size: args.length,
                }),
                pos: reader.stream_position().unwrap(),
            });
        }
        let num_entries = args.length / item_size;
        let mut faces = Vec::with_capacity(num_entries);

        for _ in 0..num_entries {
            let face = match args.version {
                0 | 1 => FaceV1::read_options(reader, endian, ())?.into(),
                2 => Face::read_options(reader, endian, ())?,
                version => {
                    return Err(binrw::Error::Custom {
                        err: Box::new(BspError::LumpVersion(
                            crate::error::UnsupportedLumpVersion {
                                lump_type: "faces",
                                version: version as u16,
                            },
                        )),
                        pos: reader.stream_position().unwrap(),
                    });
                }
            };

            faces.push(face);
        }

        Ok(Self { faces })
    }
}

impl From<FaceV1> for Face {
    fn from(value: FaceV1) -> Self {
        Self {
            plane_num: value.plane_num as _,
            side: value.side,
            on_node: value.on_node,
            first_edge: value.first_edge,
            num_edges: value.num_edges as _,
            texture_info: value.texture_info as _,
            displacement_info: value.displacement_info as _,
            surface_fog_volume_id: value.surface_fog_volume_id as _,
            styles: value.styles,
            light_offset: value.light_offset,
            area: value.area,
            light_map_texture_min: value.light_map_texture_min,
            light_map_texture_size: value.light_map_texture_size,
            original_face: value.original_face,
            primitive_count: PackedPrimitiveCount::pack(value.primitive_count as _, true),
            first_primitive_index: value.first_primitive_index as _,
            smoothing_groups: value.smoothing_groups,
        }
    }
}

impl Face {
    pub fn displacement_index(&self) -> Option<i32> {
        (self.displacement_info >= 0).then_some(self.displacement_info)
    }
}

static_assertions::const_assert_eq!(size_of::<FaceV1>(), 56);

#[derive(Default, Debug, Clone)]
pub struct VisData {
    pub cluster_count: u32,
    pub pvs_offsets: Vec<i32>,
    pub pas_offsets: Vec<i32>,
    pub data: Vec<u8>,
}

impl VisData {
    fn offset_start(&self) -> usize {
        std::mem::size_of_val(&self.pvs_offsets[..])
            + std::mem::size_of_val(&self.pas_offsets[..])
            // For `cluster_count`
            + std::mem::size_of::<u32>()
    }

    pub fn visible_clusters(&self, cluster: u32) -> impl Iterator<Item = u32> + Clone {
        // The first visdata offset should be at the start of `data`.
        debug_assert_eq!(
            *self.pvs_offsets.iter().min().unwrap() as usize,
            self.offset_start(),
        );

        let Some(offset) = self.pvs_offsets.get(cluster as usize) else {
            return Either::Left(std::iter::empty());
        };

        let Ok(offset_usize) = usize::try_from(*offset - self.offset_start() as i32) else {
            return Either::Left(std::iter::empty());
        };

        Either::Right(calculate_visdata_indices(
            &self.data[offset_usize..],
            self.cluster_count,
        ))
    }
}

#[derive(Debug, Clone, BinRead)]
pub struct VertNormal {
    pub normal: f32,
}

#[derive(Debug, Clone, BinRead)]
pub struct VertNormalIndex {
    pub index: i16,
}

pub struct Packfile {
    zip: Mutex<ZipArchive<Cursor<Vec<u8>>>>,
}

impl Clone for Packfile {
    fn clone(&self) -> Self {
        Packfile {
            zip: Mutex::new(self.zip.lock().unwrap().clone()),
        }
    }
}

impl Debug for Packfile {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Packfile")
            .field(
                "zip",
                &self
                    .zip
                    .lock()
                    .unwrap()
                    .file_names()
                    .collect::<Vec<_>>()
                    .join(", "),
            )
            .finish()
    }
}

impl Packfile {
    pub fn read(data: Cow<[u8]>) -> BspResult<Self> {
        let reader = Cursor::new(data.into_owned());
        let zip = Mutex::new(ZipArchive::new(reader)?);
        Ok(Packfile { zip })
    }

    pub fn get(&self, name: &str) -> BspResult<Option<Vec<u8>>> {
        let mut zip = self.zip.lock().unwrap();
        let mut entry = match zip.by_name(name) {
            Ok(entry) => entry,
            Err(ZipError::FileNotFound) => {
                return Ok(None);
            }
            Err(e) => {
                return Err(e.into());
            }
        };
        let mut buff = vec![0; entry.size() as usize];
        entry.read_exact(&mut buff)?;
        Ok(Some(buff))
    }

    pub fn has(&self, name: &str) -> BspResult<bool> {
        let mut zip = self.zip.lock().unwrap();
        let result = match zip.by_name(name) {
            Ok(_) => Ok(true),
            Err(ZipError::FileNotFound) => {
                return Ok(false);
            }
            Err(e) => {
                return Err(e.into());
            }
        };
        result
    }

    pub fn into_zip(self) -> Mutex<ZipArchive<Cursor<Vec<u8>>>> {
        self.zip
    }
}

fn try_read_enum<Enum, Reader, Error, ErrorFn>(
    reader: &mut Reader,
    endian: Endian,
    args: <<Enum as TryFromPrimitive>::Primitive as BinRead>::Args<'static>,
    err_map: ErrorFn,
) -> BinResult<Enum>
where
    Reader: Read + Seek,
    Enum: TryFromPrimitive<Error = TryFromPrimitiveError<Enum>>,
    Enum::Primitive: BinRead,
    ErrorFn: FnOnce(Enum::Primitive) -> Error,
    Error: CustomError + 'static,
{
    let start = reader.stream_position().unwrap();
    let raw = <Enum::Primitive>::read_options(reader, endian, args)?;

    Enum::try_from_primitive(raw)
        .map_err(|e| err_map(e.number))
        .map_err(|e| binrw::Error::Custom {
            pos: start,
            err: Box::new(e),
        })
}
