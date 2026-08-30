use std::ops::Deref;

use binrw::BinRead;
use glam::{U8Vec3, Vec3};
use image::{Rgb, Rgba};

use crate::Leaf;

#[derive(BinRead, Debug, Default, Clone, Copy)]
pub struct ColorRGBExp32 {
    // #[br(map = |val: u8| (val as f32) / 255.)]
    pub r: u8,
    // #[br(map = |val: u8| (val as f32) / 255.)]
    pub g: u8,
    // #[br(map = |val: u8| (val as f32) / 255.)]
    pub b: u8,
    // #[br(map = |val: i8| 2f32.powi(val as i32))]
    pub exponent: i8,
}

impl ColorRGBExp32 {
    pub fn to_rgb32f(&self) -> Rgb<f32> {
        let scale = 2f32.powi(self.exponent as i32);

        [self.r, self.g, self.b].map(|v| scale * v as f32).into()
    }

    pub fn to_rgbm32f(&self) -> Rgba<u8> {
        let exp = self.exponent + 8;

        if exp < 0 {
            let Ok(scale): Result<u8, _> = 2i32.pow((-exp).try_into().unwrap()).try_into() else {
                return [0, 0, 0, 0].into();
            };

            [self.r / scale, self.g / scale, self.b / scale, 1]
        } else {
            let scale = 2i32
                .pow(exp.try_into().unwrap())
                .try_into()
                .unwrap_or(u8::MAX);

            [self.r, self.g, self.b, scale]
        }
        .into()
    }
}

const NUM_CUBE_SAMPLES: usize = 6;

#[derive(BinRead, Debug, Default, Clone, Copy)]
pub struct CompressedLightCube {
    pub color: [ColorRGBExp32; NUM_CUBE_SAMPLES],
}

#[derive(BinRead, Debug, Default, Clone, Copy)]
pub struct AmbientLighting {
    pub data: CompressedLightCube,
    #[br(map = |vals: [u8; 3]| vals.into())]
    pub position: U8Vec3,
}

// TODO: Compress
pub struct AmbientVoxelGridBuilder {
    samples: Vec<Option<[Rgb<f32>; 6]>>,
}

impl Default for AmbientVoxelGridBuilder {
    fn default() -> Self {
        Self::new()
    }
}

const SIZE: usize = u8::MAX as usize;

impl AmbientVoxelGridBuilder {
    pub fn new() -> Self {
        Self {
            samples: Vec::with_capacity(SIZE.pow(3)),
        }
    }

    pub fn set(&mut self, coords: U8Vec3, value: CompressedLightCube) {
        let idx = coords.z as usize * SIZE.pow(2) + coords.y as usize * SIZE + coords.x as usize;

        self.samples[idx] = Some(value.color.map(|c| c.to_rgb32f()));
    }

    pub fn finish(self) -> AmbientVoxelGrid {
        let mut last_cube = None;

        let samples = self
            .samples
            .into_iter()
            .flat_map(|cube| {
                // TODO: Should "look forward" too
                let cube = cube.or(last_cube).unwrap_or([Rgb([0.; 3]); 6]);
                last_cube = Some(cube);
                cube.into_iter().flat_map(|color| color.0)
            })
            .collect();

        AmbientVoxelGrid { samples }
    }
}

#[derive(Default)]
pub struct AmbientVoxelGrid {
    pub samples: Vec<f32>,
}

impl AmbientLighting {
    pub fn fraction(&self) -> Vec3 {
        self.position.as_vec3() / Vec3::splat(u8::MAX as f32)
    }

    pub fn position(&self, bounds: Vec3) -> Vec3 {
        bounds * self.fraction()
    }
}

#[derive(BinRead, Debug, Default, Clone, Copy)]
pub struct LeafAmbientIndex {
    pub count: u16,
    pub start: u16,
}

pub struct LeafWithAmbientIndex<'a> {
    pub leaf: &'a Leaf,
    pub ambient_index: Option<&'a LeafAmbientIndex>,
}

impl Deref for LeafWithAmbientIndex<'_> {
    type Target = Leaf;

    fn deref(&self) -> &Self::Target {
        self.leaf
    }
}
