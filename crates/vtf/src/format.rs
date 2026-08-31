//! Source's `ImageFormat` enum, its surface sizes, and CPU decoding to RGBA8.
//!
//! Indices are from [`public/bitmap/imageformat.h`], and the discriminants
//! matter: they are what a VTF header stores.
//!
//! # What TF2 actually uses
//!
//! A census of all 34 000 VTFs in the shipped VPKs, by `imageFormat`:
//!
//! | index | format | count |
//! |---|---|---|
//! | 15 | `DXT5` | 23 902 |
//! | 13 | `DXT1` | 9 045 |
//! | 12 | `BGRA8888` | 675 |
//! | 3 | `BGR888` | 322 |
//! | 22 | `UV88` | 31 |
//! | 0 | `RGBA8888` | 29 |
//! | 24 | `RGBA16161616F` | 28 |
//! | 16 | `BGRX8888` | 10 |
//! | 1 | `ABGR8888` | 7 |
//! | 9 | `RGB888_BLUESCREEN` | 5 |
//!
//! Note what is *missing* from that list: `DXT3`, `I8`, `IA88` and `A8`, all
//! four of which the plan asked for, while `ABGR8888` and
//! `RGB888_BLUESCREEN`, which it did not, are present.
//!
//! **That census was incomplete, and the plan was right.** It covers the
//! shipped VPKs only — not the zip inside each map. Sweeping the 39 438
//! textures the 233 maps actually reference turns up **12 `DXT3`, one `A8` and
//! one `I8`**, all packed inside map pakfiles. Implementing the formats the
//! census said were absent is what kept that sweep at zero failures.
//!
//! [`public/bitmap/imageformat.h`]: ../../../source-sdk-2013/src/public/bitmap/imageformat.h

use crate::VtfError;

/// A block-compressed format's block edge, in texels.
const BLOCK_SIZE: u32 = 4;

/// Source `ImageFormat`. Discriminants are the on-disk values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum ImageFormat {
    Rgba8888 = 0,
    Abgr8888 = 1,
    Rgb888 = 2,
    Bgr888 = 3,
    Rgb565 = 4,
    I8 = 5,
    Ia88 = 6,
    P8 = 7,
    A8 = 8,
    Rgb888Bluescreen = 9,
    Bgr888Bluescreen = 10,
    Argb8888 = 11,
    Bgra8888 = 12,
    Dxt1 = 13,
    Dxt3 = 14,
    Dxt5 = 15,
    Bgrx8888 = 16,
    Bgr565 = 17,
    Bgrx5551 = 18,
    Bgra4444 = 19,
    Dxt1OneBitAlpha = 20,
    Bgra5551 = 21,
    Uv88 = 22,
    Uvwq8888 = 23,
    Rgba16161616F = 24,
    Rgba16161616 = 25,
    Uvlx8888 = 26,
    R32F = 27,
    Rgb323232F = 28,
    Rgba32323232F = 29,
}

impl ImageFormat {
    pub fn from_index(value: i32) -> Option<ImageFormat> {
        use ImageFormat::*;
        Some(match value {
            0 => Rgba8888,
            1 => Abgr8888,
            2 => Rgb888,
            3 => Bgr888,
            4 => Rgb565,
            5 => I8,
            6 => Ia88,
            7 => P8,
            8 => A8,
            9 => Rgb888Bluescreen,
            10 => Bgr888Bluescreen,
            11 => Argb8888,
            12 => Bgra8888,
            13 => Dxt1,
            14 => Dxt3,
            15 => Dxt5,
            16 => Bgrx8888,
            17 => Bgr565,
            18 => Bgrx5551,
            19 => Bgra4444,
            20 => Dxt1OneBitAlpha,
            21 => Bgra5551,
            22 => Uv88,
            23 => Uvwq8888,
            24 => Rgba16161616F,
            25 => Rgba16161616,
            26 => Uvlx8888,
            27 => R32F,
            28 => Rgb323232F,
            29 => Rgba32323232F,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        use ImageFormat::*;
        match self {
            Rgba8888 => "RGBA8888",
            Abgr8888 => "ABGR8888",
            Rgb888 => "RGB888",
            Bgr888 => "BGR888",
            Rgb565 => "RGB565",
            I8 => "I8",
            Ia88 => "IA88",
            P8 => "P8",
            A8 => "A8",
            Rgb888Bluescreen => "RGB888_BLUESCREEN",
            Bgr888Bluescreen => "BGR888_BLUESCREEN",
            Argb8888 => "ARGB8888",
            Bgra8888 => "BGRA8888",
            Dxt1 => "DXT1",
            Dxt3 => "DXT3",
            Dxt5 => "DXT5",
            Bgrx8888 => "BGRX8888",
            Bgr565 => "BGR565",
            Bgrx5551 => "BGRX5551",
            Bgra4444 => "BGRA4444",
            Dxt1OneBitAlpha => "DXT1_ONEBITALPHA",
            Bgra5551 => "BGRA5551",
            Uv88 => "UV88",
            Uvwq8888 => "UVWQ8888",
            Rgba16161616F => "RGBA16161616F",
            Rgba16161616 => "RGBA16161616",
            Uvlx8888 => "UVLX8888",
            R32F => "R32F",
            Rgb323232F => "RGB323232F",
            Rgba32323232F => "RGBA32323232F",
        }
    }

    /// Bytes per 4x4 block, for the BCn formats.
    pub fn block_bytes(self) -> Option<u64> {
        use ImageFormat::*;
        match self {
            Dxt1 | Dxt1OneBitAlpha => Some(8),
            Dxt3 | Dxt5 => Some(16),
            _ => None,
        }
    }

    pub fn is_block_compressed(self) -> bool {
        self.block_bytes().is_some()
    }

    /// Bytes per texel, for the uncompressed formats.
    pub fn bytes_per_pixel(self) -> Option<u64> {
        use ImageFormat::*;
        Some(match self {
            I8 | P8 | A8 => 1,
            Rgb565 | Ia88 | Bgr565 | Bgrx5551 | Bgra4444 | Bgra5551 | Uv88 => 2,
            Rgb888 | Bgr888 | Rgb888Bluescreen | Bgr888Bluescreen => 3,
            Rgba8888 | Abgr8888 | Argb8888 | Bgra8888 | Bgrx8888 | Uvwq8888 | Uvlx8888 | R32F => 4,
            Rgba16161616F | Rgba16161616 => 8,
            Rgb323232F => 12,
            Rgba32323232F => 16,
            Dxt1 | Dxt1OneBitAlpha | Dxt3 | Dxt5 => return None,
        })
    }

    /// Bytes one `width x height` surface occupies.
    ///
    /// Block formats round up to whole blocks, so a 2x2 mip of DXT1 still costs
    /// a full 8-byte block. Getting this wrong desynchronises the mip chain at
    /// the small end, where the error is least visible.
    pub fn surface_bytes(self, width: u32, height: u32) -> u64 {
        // A zero-sized surface costs nothing — it is not one block. This is
        // exactly the case that made 12 shipped files appear 8 bytes short of
        // their image data during development.
        if width == 0 || height == 0 {
            return 0;
        }
        match self.block_bytes() {
            Some(per_block) => {
                let blocks_x = u64::from(width.div_ceil(BLOCK_SIZE));
                let blocks_y = u64::from(height.div_ceil(BLOCK_SIZE));
                blocks_x * blocks_y * per_block
            }
            None => {
                let bpp = self.bytes_per_pixel().unwrap_or(0);
                u64::from(width) * u64::from(height) * bpp
            }
        }
    }

    /// Whether the format carries meaningful alpha, for the renderer's blend
    /// mode. `BGRX`/`UVLX` have an X channel that is padding, not alpha.
    pub fn has_alpha(self) -> bool {
        use ImageFormat::*;
        matches!(
            self,
            Rgba8888
                | Abgr8888
                | Argb8888
                | Bgra8888
                | Bgra4444
                | Bgra5551
                | Bgrx5551
                | A8
                | Ia88
                | Dxt3
                | Dxt5
                | Dxt1OneBitAlpha
                | Rgba16161616F
                | Rgba16161616
                | Rgba32323232F
        )
    }

    /// Decode one surface to tightly packed RGBA8.
    ///
    /// This is for the atlas dump and for backends without BC support; the
    /// renderer hands BCn blocks straight to the GPU instead.
    pub fn decode_rgba8(self, data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, VtfError> {
        let needed = self.surface_bytes(width, height) as usize;
        if data.len() < needed {
            return Err(VtfError::SurfaceTruncated {
                format: self.name(),
                width,
                height,
                needed,
                got: data.len(),
            });
        }
        let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
        match self {
            ImageFormat::Dxt1 | ImageFormat::Dxt1OneBitAlpha => {
                decode_bc(data, width, height, &mut out, BcKind::Bc1)
            }
            ImageFormat::Dxt3 => decode_bc(data, width, height, &mut out, BcKind::Bc2),
            ImageFormat::Dxt5 => decode_bc(data, width, height, &mut out, BcKind::Bc3),
            _ => self.decode_uncompressed(data, width, height, &mut out)?,
        }
        Ok(out)
    }

    fn decode_uncompressed(
        self,
        data: &[u8],
        width: u32,
        height: u32,
        out: &mut [u8],
    ) -> Result<(), VtfError> {
        use ImageFormat::*;
        let stride = self
            .bytes_per_pixel()
            .ok_or(VtfError::UnsupportedDecode { format: self.name() })? as usize;
        let count = (width as usize) * (height as usize);

        for i in 0..count {
            let src = &data[i * stride..i * stride + stride];
            let dst = &mut out[i * 4..i * 4 + 4];
            match self {
                Rgba8888 => dst.copy_from_slice(src),
                Abgr8888 => dst.copy_from_slice(&[src[3], src[2], src[1], src[0]]),
                Argb8888 => dst.copy_from_slice(&[src[1], src[2], src[3], src[0]]),
                Bgra8888 => dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]),
                Bgrx8888 | Uvlx8888 => dst.copy_from_slice(&[src[2], src[1], src[0], 255]),
                Rgb888 | Rgb888Bluescreen => dst.copy_from_slice(&[src[0], src[1], src[2], 255]),
                Bgr888 | Bgr888Bluescreen => dst.copy_from_slice(&[src[2], src[1], src[0], 255]),
                I8 => dst.copy_from_slice(&[src[0], src[0], src[0], 255]),
                A8 => dst.copy_from_slice(&[255, 255, 255, src[0]]),
                Ia88 => dst.copy_from_slice(&[src[0], src[0], src[0], src[1]]),
                // A tangent-space normal with the Z component reconstructed in
                // the shader; the dump shows the two stored channels.
                Uv88 => dst.copy_from_slice(&[src[0], src[1], 128, 255]),
                Uvwq8888 => dst.copy_from_slice(&[src[0], src[1], src[2], 255]),
                Rgb565 => {
                    let v = u16::from_le_bytes([src[0], src[1]]);
                    let [r, g, b] = rgb565(v);
                    dst.copy_from_slice(&[r, g, b, 255]);
                }
                Bgr565 => {
                    let v = u16::from_le_bytes([src[0], src[1]]);
                    let [r, g, b] = rgb565(v);
                    dst.copy_from_slice(&[b, g, r, 255]);
                }
                Bgra4444 => {
                    let v = u16::from_le_bytes([src[0], src[1]]);
                    let expand = |x: u16| ((x & 0xf) as u8) * 17;
                    dst.copy_from_slice(&[expand(v >> 8), expand(v >> 4), expand(v), expand(v >> 12)]);
                }
                Bgra5551 | Bgrx5551 => {
                    let v = u16::from_le_bytes([src[0], src[1]]);
                    let expand5 = |x: u16| (((x & 0x1f) as u32 * 255 + 15) / 31) as u8;
                    let alpha = if self == Bgra5551 && v & 0x8000 == 0 { 0 } else { 255 };
                    dst.copy_from_slice(&[
                        expand5(v >> 10),
                        expand5(v >> 5),
                        expand5(v),
                        alpha,
                    ]);
                }
                Rgba16161616F => {
                    // Linear half-float HDR: tonemapped here so the dump is
                    // legible. The renderer keeps the float data instead.
                    for c in 0..4 {
                        let h = half::f16::from_le_bytes([src[c * 2], src[c * 2 + 1]]);
                        let v = h.to_f32();
                        dst[c] = if c == 3 {
                            (v.clamp(0.0, 1.0) * 255.0) as u8
                        } else {
                            tonemap(v)
                        };
                    }
                }
                Rgba16161616 => {
                    for c in 0..4 {
                        dst[c] = src[c * 2 + 1];
                    }
                }
                R32F => {
                    let v = f32::from_le_bytes([src[0], src[1], src[2], src[3]]);
                    let g = tonemap(v);
                    dst.copy_from_slice(&[g, g, g, 255]);
                }
                Rgb323232F | Rgba32323232F => {
                    for (c, slot) in dst.iter_mut().take(3).enumerate() {
                        let at = c * 4;
                        let v = f32::from_le_bytes([src[at], src[at + 1], src[at + 2], src[at + 3]]);
                        *slot = tonemap(v);
                    }
                    dst[3] = 255;
                }
                P8 => return Err(VtfError::UnsupportedDecode { format: self.name() }),
                Dxt1 | Dxt1OneBitAlpha | Dxt3 | Dxt5 => unreachable!("handled by decode_rgba8"),
            }
        }
        Ok(())
    }
}

/// Reinhard plus gamma, so a linear HDR surface is visible in an 8-bit dump.
fn tonemap(v: f32) -> u8 {
    let mapped = v.max(0.0) / (v.max(0.0) + 1.0);
    (mapped.powf(1.0 / 2.2) * 255.0).clamp(0.0, 255.0) as u8
}

fn rgb565(v: u16) -> [u8; 3] {
    // Replicate the high bits into the low ones so white stays white.
    let r = ((v >> 11) & 0x1f) as u32;
    let g = ((v >> 5) & 0x3f) as u32;
    let b = (v & 0x1f) as u32;
    [
        ((r * 255 + 15) / 31) as u8,
        ((g * 255 + 31) / 63) as u8,
        ((b * 255 + 15) / 31) as u8,
    ]
}

/// Which BCn variant a block-compressed surface uses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BcKind {
    /// DXT1: colour only, with an optional one-bit alpha mode.
    Bc1,
    /// DXT3: 4-bit explicit alpha, then a DXT1 colour block.
    Bc2,
    /// DXT5: interpolated alpha, then a DXT1 colour block.
    Bc3,
}

impl BcKind {
    fn block_bytes(self) -> usize {
        match self {
            BcKind::Bc1 => 8,
            BcKind::Bc2 | BcKind::Bc3 => 16,
        }
    }
}

/// Decode a whole BCn surface into RGBA8.
fn decode_bc(data: &[u8], width: u32, height: u32, out: &mut [u8], kind: BcKind) {
    let blocks_x = width.div_ceil(BLOCK_SIZE);
    let blocks_y = height.div_ceil(BLOCK_SIZE);
    let stride = kind.block_bytes();

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let at = ((by * blocks_x + bx) as usize) * stride;
            let block = &data[at..at + stride];
            let mut texels = [[0u8; 4]; 16];

            match kind {
                BcKind::Bc1 => decode_bc1_colour(block, &mut texels, true),
                BcKind::Bc2 => {
                    decode_bc1_colour(&block[8..], &mut texels, false);
                    // 4 bits of alpha per texel, low nibble first.
                    for (i, texel) in texels.iter_mut().enumerate() {
                        let byte = block[i / 2];
                        let nibble = if i % 2 == 0 { byte & 0xf } else { byte >> 4 };
                        texel[3] = nibble * 17;
                    }
                }
                BcKind::Bc3 => {
                    decode_bc1_colour(&block[8..], &mut texels, false);
                    let alpha = decode_bc3_alpha(&block[..8]);
                    for (texel, a) in texels.iter_mut().zip(alpha) {
                        texel[3] = a;
                    }
                }
            }

            // Write the block's texels, clipping at the surface edge: a mip
            // narrower than 4 texels still stores a full block.
            for ty in 0..BLOCK_SIZE {
                let y = by * BLOCK_SIZE + ty;
                if y >= height {
                    break;
                }
                for tx in 0..BLOCK_SIZE {
                    let x = bx * BLOCK_SIZE + tx;
                    if x >= width {
                        break;
                    }
                    let dst = ((y * width + x) as usize) * 4;
                    out[dst..dst + 4]
                        .copy_from_slice(&texels[(ty * BLOCK_SIZE + tx) as usize]);
                }
            }
        }
    }
}

/// The 8-byte DXT1 colour block shared by all three formats.
///
/// `allow_punchthrough` is the difference between DXT1 standalone and DXT1 as
/// the colour half of DXT3/DXT5: only standalone DXT1 uses the `c0 <= c1` case
/// for a transparent fourth colour. In DXT3/DXT5 the four-colour interpolation
/// is always used, because alpha comes from the other half of the block.
fn decode_bc1_colour(block: &[u8], out: &mut [[u8; 4]; 16], allow_punchthrough: bool) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let rgb0 = rgb565(c0);
    let rgb1 = rgb565(c1);

    let mut palette = [[0u8; 4]; 4];
    palette[0] = [rgb0[0], rgb0[1], rgb0[2], 255];
    palette[1] = [rgb1[0], rgb1[1], rgb1[2], 255];

    if c0 > c1 || !allow_punchthrough {
        for c in 0..3 {
            palette[2][c] = ((2 * u16::from(rgb0[c]) + u16::from(rgb1[c])) / 3) as u8;
            palette[3][c] = ((u16::from(rgb0[c]) + 2 * u16::from(rgb1[c])) / 3) as u8;
        }
        palette[2][3] = 255;
        palette[3][3] = 255;
    } else {
        for c in 0..3 {
            palette[2][c] = ((u16::from(rgb0[c]) + u16::from(rgb1[c])) / 2) as u8;
            palette[3][c] = 0;
        }
        palette[2][3] = 255;
        // The punch-through case: index 3 is fully transparent black.
        palette[3][3] = 0;
    }

    let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    for (i, texel) in out.iter_mut().enumerate() {
        *texel = palette[((indices >> (i * 2)) & 0x3) as usize];
    }
}

/// The 8-byte interpolated-alpha block of DXT5 (also BC4).
fn decode_bc3_alpha(block: &[u8]) -> [u8; 16] {
    let (a0, a1) = (block[0], block[1]);
    let mut palette = [0u8; 8];
    palette[0] = a0;
    palette[1] = a1;
    if a0 > a1 {
        for i in 1..7u16 {
            palette[1 + i as usize] =
                (((7 - i) * u16::from(a0) + i * u16::from(a1)) / 7) as u8;
        }
    } else {
        for i in 1..5u16 {
            palette[1 + i as usize] =
                (((5 - i) * u16::from(a0) + i * u16::from(a1)) / 5) as u8;
        }
        palette[6] = 0;
        palette[7] = 255;
    }

    // 16 three-bit indices packed into 6 bytes, low bits first.
    let bits = u64::from_le_bytes([
        block[2], block[3], block[4], block[5], block[6], block[7], 0, 0,
    ]);
    let mut out = [0u8; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = palette[((bits >> (i * 3)) & 0x7) as usize];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_disk_indices_match_the_sdk_enum() {
        // These are read straight out of a file header; a shifted
        // discriminant would silently decode DXT5 as BGRX8888.
        assert_eq!(ImageFormat::from_index(0), Some(ImageFormat::Rgba8888));
        assert_eq!(ImageFormat::from_index(13), Some(ImageFormat::Dxt1));
        assert_eq!(ImageFormat::from_index(15), Some(ImageFormat::Dxt5));
        assert_eq!(ImageFormat::from_index(24), Some(ImageFormat::Rgba16161616F));
        assert_eq!(ImageFormat::from_index(29), Some(ImageFormat::Rgba32323232F));
        assert_eq!(ImageFormat::from_index(-1), None, "IMAGE_FORMAT_UNKNOWN");
        assert_eq!(ImageFormat::from_index(30), None);
    }

    #[test]
    fn block_formats_round_up_to_whole_blocks() {
        // The small end of a mip chain is where this matters: a 1x1 DXT1 mip
        // still occupies one 8-byte block.
        assert_eq!(ImageFormat::Dxt1.surface_bytes(4, 4), 8);
        assert_eq!(ImageFormat::Dxt1.surface_bytes(2, 2), 8);
        assert_eq!(ImageFormat::Dxt1.surface_bytes(1, 1), 8);
        assert_eq!(ImageFormat::Dxt5.surface_bytes(1, 1), 16);
        assert_eq!(ImageFormat::Dxt1.surface_bytes(1024, 1024), 524_288);
        assert_eq!(ImageFormat::Dxt5.surface_bytes(1024, 1024), 1_048_576);
    }

    #[test]
    fn a_zero_sized_surface_costs_nothing() {
        // Regression: rounding a 0x0 low-res image up to one block put the
        // high-res image offset 8 bytes too high on 12 shipped files.
        assert_eq!(ImageFormat::Dxt1.surface_bytes(0, 0), 0);
        assert_eq!(ImageFormat::Dxt1.surface_bytes(0, 16), 0);
        assert_eq!(ImageFormat::Bgra8888.surface_bytes(0, 0), 0);
    }

    #[test]
    fn uncompressed_sizes_are_width_times_height_times_depth() {
        assert_eq!(ImageFormat::Bgra8888.surface_bytes(16, 16), 1024);
        assert_eq!(ImageFormat::Bgr888.surface_bytes(16, 16), 768);
        assert_eq!(ImageFormat::Uv88.surface_bytes(16, 16), 512);
        assert_eq!(ImageFormat::I8.surface_bytes(16, 16), 256);
        assert_eq!(ImageFormat::Rgba16161616F.surface_bytes(16, 16), 2048);
    }

    /// A DXT1 block encoding a flat colour: both endpoints equal, all indices 0.
    fn flat_dxt1(colour: u16) -> [u8; 8] {
        let mut block = [0u8; 8];
        block[0..2].copy_from_slice(&colour.to_le_bytes());
        block[2..4].copy_from_slice(&colour.to_le_bytes());
        block
    }

    #[test]
    fn dxt1_decodes_a_flat_block() {
        // 0xf800 is pure red in RGB565.
        let out = ImageFormat::Dxt1
            .decode_rgba8(&flat_dxt1(0xf800), 4, 4)
            .expect("decode");
        assert_eq!(out.len(), 4 * 4 * 4);
        for texel in out.chunks(4) {
            assert_eq!(texel, [255, 0, 0, 255], "flat red expected");
        }
    }

    #[test]
    fn dxt1_punchthrough_only_applies_to_standalone_dxt1() {
        // c0 <= c1 selects the 3-colour + transparent mode. Index 3 must be
        // transparent in DXT1...
        let mut block = [0u8; 8];
        block[0..2].copy_from_slice(&0x0000u16.to_le_bytes()); // c0 = black
        block[2..4].copy_from_slice(&0xffffu16.to_le_bytes()); // c1 = white, c0 < c1
        // All 16 texels use index 3.
        block[4..8].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        let out = ImageFormat::Dxt1.decode_rgba8(&block, 4, 4).expect("decode");
        assert_eq!(&out[0..4], [0, 0, 0, 0], "index 3 must be transparent");

        // ...but the same colour block inside DXT5 is opaque, because alpha
        // comes from the other half of the block. Build a DXT5 block whose
        // alpha half is a flat 255.
        let mut dxt5 = [0u8; 16];
        dxt5[0] = 255;
        dxt5[1] = 255;
        dxt5[8..16].copy_from_slice(&block);
        let out = ImageFormat::Dxt5.decode_rgba8(&dxt5, 4, 4).expect("decode");
        assert_eq!(out[3], 255, "DXT5 colour half must not punch through");
        // The 4-colour interpolation puts index 3 at 1/3 of the way from white.
        assert!(out[0] > 0, "index 3 should be a real colour, got {:?}", &out[0..4]);
    }

    #[test]
    fn dxt5_alpha_interpolates_between_the_endpoints() {
        let mut block = [0u8; 16];
        block[0] = 255; // a0
        block[1] = 0; // a1, so a0 > a1: the 8-step ramp
        // Indices: texel 0 -> 0 (a0), texel 1 -> 1 (a1), rest 0.
        let indices: u64 = 1 << 3;
        block[2..8].copy_from_slice(&indices.to_le_bytes()[..6]);
        block[8..10].copy_from_slice(&0xffffu16.to_le_bytes());
        block[10..12].copy_from_slice(&0xffffu16.to_le_bytes());

        let out = ImageFormat::Dxt5.decode_rgba8(&block, 4, 4).expect("decode");
        assert_eq!(out[3], 255, "texel 0 takes a0");
        assert_eq!(out[7], 0, "texel 1 takes a1");
    }

    #[test]
    fn dxt3_alpha_is_four_bits_low_nibble_first() {
        let mut block = [0u8; 16];
        // Texel 0 alpha = 0x0, texel 1 = 0xf, packed into one byte.
        block[0] = 0xf0;
        block[8..10].copy_from_slice(&0xffffu16.to_le_bytes());
        block[10..12].copy_from_slice(&0xffffu16.to_le_bytes());
        let out = ImageFormat::Dxt3.decode_rgba8(&block, 4, 4).expect("decode");
        assert_eq!(out[3], 0, "low nibble is texel 0");
        assert_eq!(out[7], 255, "high nibble is texel 1, 0xf -> 255");
    }

    #[test]
    fn a_block_wider_than_the_surface_is_clipped_not_overrun() {
        // A 2x2 mip is stored as a full 4x4 block; writing all 16 texels would
        // run past the output buffer.
        let out = ImageFormat::Dxt1
            .decode_rgba8(&flat_dxt1(0x07e0), 2, 2)
            .expect("decode");
        assert_eq!(out.len(), 2 * 2 * 4);
        for texel in out.chunks(4) {
            assert_eq!(texel, [0, 255, 0, 255]);
        }
    }

    #[test]
    fn channel_orders_are_not_transposed() {
        // One opaque pixel, distinct in every channel, through each swizzle.
        let cases: [(ImageFormat, &[u8], [u8; 4]); 6] = [
            (ImageFormat::Rgba8888, &[1, 2, 3, 4], [1, 2, 3, 4]),
            (ImageFormat::Abgr8888, &[4, 3, 2, 1], [1, 2, 3, 4]),
            (ImageFormat::Argb8888, &[4, 1, 2, 3], [1, 2, 3, 4]),
            (ImageFormat::Bgra8888, &[3, 2, 1, 4], [1, 2, 3, 4]),
            (ImageFormat::Bgr888, &[3, 2, 1], [1, 2, 3, 255]),
            (ImageFormat::Bgrx8888, &[3, 2, 1, 99], [1, 2, 3, 255]),
        ];
        for (format, input, expected) in cases {
            let out = format.decode_rgba8(input, 1, 1).expect("decode");
            assert_eq!(out, expected, "{}", format.name());
        }
    }

    #[test]
    fn a_truncated_surface_is_an_error_not_a_panic() {
        let err = ImageFormat::Dxt5
            .decode_rgba8(&[0u8; 8], 4, 4)
            .expect_err("must reject");
        assert!(matches!(err, VtfError::SurfaceTruncated { .. }), "{err}");
    }

    #[test]
    fn rgb565_expansion_keeps_white_white() {
        assert_eq!(rgb565(0xffff), [255, 255, 255]);
        assert_eq!(rgb565(0x0000), [0, 0, 0]);
    }
}
