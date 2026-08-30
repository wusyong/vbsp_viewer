use std::io::{Read, Seek};
use std::mem::{align_of, size_of};
use std::ops::Deref;

use binrw::{BinRead, BinResult, Endian};
use glam::{I16Vec3, Vec3};

use crate::BspError;
use crate::lighting::CompressedLightCube;

use super::LumpArgs;

#[derive(Debug, Clone)]
pub struct Leaves {
    leaves: Vec<Leaf>,
}

impl Leaves {
    pub fn new(leaves: Vec<Leaf>) -> Self {
        Leaves { leaves }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Leaf> {
        self.into_iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Leaf> {
        self.into_iter()
    }

    pub fn into_inner(self) -> Vec<Leaf> {
        self.leaves
    }
}

impl BinRead for Leaves {
    type Args<'a> = LumpArgs;

    fn read_options<R: Read + Seek>(
        reader: &mut R,
        endian: Endian,
        args: Self::Args<'_>,
    ) -> BinResult<Self> {
        let item_size = match args.version {
            0 => size_of::<LeafV0>(),
            1 => size_of::<LeafV1>(),
            2 => size_of::<LeafV2>(),
            version => {
                return Err(binrw::Error::Custom {
                    err: Box::new(BspError::LumpVersion(
                        crate::error::UnsupportedLumpVersion {
                            lump_type: "leaves",
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
        let mut entries = Vec::with_capacity(num_entries);

        for _ in 0..num_entries {
            entries.push(Leaf::read_options(
                reader,
                endian,
                LeafArgs {
                    version: args.version,
                },
            )?);
        }

        Ok(Self::new(entries))
    }
}

impl From<Vec<Leaf>> for Leaves {
    fn from(other: Vec<Leaf>) -> Self {
        Self::new(other)
    }
}

impl Deref for Leaves {
    type Target = [Leaf];

    fn deref(&self) -> &Self::Target {
        &self.leaves
    }
}

impl IntoIterator for Leaves {
    type Item = Leaf;
    type IntoIter = <Vec<Leaf> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.leaves.into_iter()
    }
}

impl<'a> IntoIterator for &'a Leaves {
    type Item = &'a Leaf;
    type IntoIter = <&'a [Leaf] as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.leaves[..].iter()
    }
}

impl<'a> IntoIterator for &'a mut Leaves {
    type Item = &'a mut Leaf;
    type IntoIter = <&'a mut [Leaf] as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.leaves.iter_mut()
    }
}

#[derive(Default, Debug, Clone, BinRead)]
pub struct LeafV0 {
    pub contents: i32,
    pub cluster: i16,
    pub area_and_flags: i16,
    // first 9 bits is area, last 7 bits is flags
    #[br(map = |v: [i16; 3]| v.into())]
    pub mins: I16Vec3,
    #[br(map = |v: [i16; 3]| v.into())]
    pub maxs: I16Vec3,
    pub first_leaf_face: u16,
    pub leaf_face_count: u16,
    pub first_leaf_brush: u16,
    pub leaf_brush_count: u16,
    pub leaf_water_data_id: i16,
    #[br(align_after = align_of::< LeafV0 > ())]
    pub cube: CompressedLightCube,
}

// TODO: `CompressedLightCube` now contains floats, so is larger. We should figure out a cleaner way to handle this.
// static_assertions::const_assert_eq!(size_of::<LeafV0>(), 56);

impl From<LeafV0> for Leaf {
    fn from(value: LeafV0) -> Self {
        Self {
            contents: value.contents,
            cluster: value.cluster as _,
            area_and_flags: PackedAreaAndFlags::from_i16(value.area_and_flags),
            mins: value.mins.as_vec3(),
            maxs: value.maxs.as_vec3(),
            first_leaf_face: value.first_leaf_face as _,
            leaf_face_count: value.leaf_face_count as _,
            first_leaf_brush: value.first_leaf_brush as _,
            leaf_brush_count: value.leaf_brush_count as _,
            leaf_water_data_id: value.leaf_water_data_id as _,
            cube: Some(value.cube),
        }
    }
}

#[derive(Default, Debug, Clone, BinRead)]
pub struct LeafV1 {
    pub contents: i32,
    pub cluster: i16,
    pub area_and_flags: i16,
    // first 9 bits is area, last 7 bits is flags
    #[br(map = |v: [i16; 3]| v.into())]
    pub mins: I16Vec3,
    #[br(map = |v: [i16; 3]| v.into())]
    pub maxs: I16Vec3,
    pub first_leaf_face: u16,
    pub leaf_face_count: u16,
    pub first_leaf_brush: u16,
    pub leaf_brush_count: u16,
    #[br(align_after = align_of::< LeafV1 > ())]
    pub leaf_water_data_id: i16,
}

static_assertions::const_assert_eq!(size_of::<LeafV1>(), 32);

impl From<LeafV1> for Leaf {
    fn from(value: LeafV1) -> Self {
        Self {
            contents: value.contents,
            cluster: value.cluster as _,
            area_and_flags: PackedAreaAndFlags::from_i16(value.area_and_flags),
            mins: value.mins.as_vec3(),
            maxs: value.maxs.as_vec3(),
            first_leaf_face: value.first_leaf_face as _,
            leaf_face_count: value.leaf_face_count as _,
            first_leaf_brush: value.first_leaf_brush as _,
            leaf_brush_count: value.leaf_brush_count as _,
            leaf_water_data_id: value.leaf_water_data_id as _,
            cube: None,
        }
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct PackedAreaAndFlags(i32);

impl PackedAreaAndFlags {
    const AREA_MASK: i32 = 0b1111_1111_1111_1111_1000_0000_0000_0000_u32 as i32;
    const FLAGS_MASK: i32 = !Self::AREA_MASK;

    pub fn area(&self) -> i32 {
        (self.0 & Self::AREA_MASK) >> Self::AREA_MASK.trailing_zeros()
    }

    pub fn flags(&self) -> i32 {
        self.0 & Self::FLAGS_MASK
    }

    pub fn from_i16(value: i16) -> Self {
        const AREA_MASK_16: i16 = 0b1111_1111_1000_0000_u16 as i16;
        const FLAGS_MASK_16: i16 = !AREA_MASK_16;

        let area = (value & AREA_MASK_16) >> AREA_MASK_16.trailing_zeros();
        let flags = value & FLAGS_MASK_16;

        Self((area << Self::AREA_MASK.trailing_zeros()) as i32 | flags as i32)
    }
}

#[derive(Default, Debug, Clone)]
pub struct Leaf {
    pub contents: i32,
    pub cluster: i32,
    // first 9 bits is area, last 7 bits is flags
    pub area_and_flags: PackedAreaAndFlags,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub first_leaf_face: u32,
    pub leaf_face_count: u32,
    pub first_leaf_brush: u32,
    pub leaf_brush_count: u32,
    pub leaf_water_data_id: i32,
    pub cube: Option<CompressedLightCube>,
}

// static_assertions::const_assert_eq!(size_of::<Leaf>(), 56);

#[derive(Default, Debug, Clone, BinRead)]
pub struct LeafV2 {
    pub contents: i32,
    pub cluster: i32,
    // first 17 bits is area, last 15 bits is flags
    pub area_and_flags: i32,
    #[br(map = |v: [f32; 3]| v.into())]
    pub mins: Vec3,
    #[br(map = |v: [f32; 3]| v.into())]
    pub maxs: Vec3,
    pub first_leaf_face: u32,
    pub leaf_face_count: u32,
    pub first_leaf_brush: u32,
    pub leaf_brush_count: u32,
    pub leaf_water_data_id: i32,
    // Ambient light cube accessed via LUMP_LEAF_AMBIENT_INDEX
}

impl From<LeafV2> for Leaf {
    fn from(value: LeafV2) -> Self {
        Self {
            contents: value.contents,
            cluster: value.cluster,
            area_and_flags: PackedAreaAndFlags(value.area_and_flags),
            mins: value.mins,
            maxs: value.maxs,
            first_leaf_face: value.first_leaf_face,
            leaf_face_count: value.leaf_face_count,
            first_leaf_brush: value.first_leaf_brush,
            leaf_brush_count: value.leaf_brush_count,
            leaf_water_data_id: value.leaf_water_data_id,
            cube: Default::default(),
        }
    }
}

#[test]
fn test_leaf_bytes() {
    super::test_read_bytes::<Leaf>();
}

#[derive(Default, Debug, Clone)]
pub struct LeafArgs {
    pub version: u32,
}

impl BinRead for Leaf {
    type Args<'a> = LeafArgs;

    fn read_options<R: Read + Seek>(
        reader: &mut R,
        endian: Endian,
        args: Self::Args<'_>,
    ) -> BinResult<Self> {
        match args.version {
            0 => LeafV0::read_options(reader, endian, ()).map(Leaf::from),
            1 => LeafV1::read_options(reader, endian, ()).map(Leaf::from),
            2 => LeafV2::read_options(reader, endian, ()).map(Leaf::from),
            version => Err(binrw::Error::Custom {
                err: Box::new(crate::error::UnsupportedLumpVersion {
                    lump_type: "leaves",
                    version: version as u16,
                }),
                pos: reader.stream_position().unwrap(),
            }),
        }
    }
}
