//! VTF — Valve Texture Format.
//!
//! ```no_run
//! let bytes = std::fs::read("concretewall001.vtf")?;
//! let vtf = vtf::Vtf::parse(&bytes)?;
//! println!("{}x{} {}", vtf.width, vtf.height, vtf.format.name());
//! let rgba = vtf.decode_rgba8(0, 0, 0)?;   // largest mip, frame 0, face 0
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Header layout
//!
//! Field offsets are fixed and *not* a packed struct: `VectorAligned
//! reflectivity` makes the C++ compiler insert padding that is part of the
//! on-disk format, which the SDK itself flags in a `!!!!CRITICAL!!!!` comment
//! ([`vtf.h:446`]). Measured against a real 7.4 file:
//!
//! ```text
//! 0x00 "VTF\0"          0x30 bumpScale f32
//! 0x04 version[2] i32   0x34 imageFormat i32
//! 0x0c headerSize i32   0x38 numMipLevels u8
//! 0x10 width u16        0x39 lowResImageFormat i32   <- unaligned
//! 0x12 height u16       0x3d lowResImageWidth u8
//! 0x14 flags u32        0x3e lowResImageHeight u8
//! 0x18 numFrames u16    0x3f depth u16              (7.2+)
//! 0x1a startFrame u16   0x41 pad[3]
//! 0x1c pad[4]           0x44 numResources u32       (7.3+)
//! 0x20 reflectivity     0x50 resources[]            <- not 0x48
//! ```
//!
//! **The resource dictionary starts at 0x50, not at the end of the struct
//! (0x48).** That is the padding the SDK warns about.
//!
//! # Finding the image data
//!
//! The plan said to seek by `headerSize`. That is right for not trusting a
//! computed struct size, but it does **not** locate the high-res image:
//! measured across 34 000 shipped VTFs, the high-res offset equals `headerSize`
//! in only **32** files and differs in **28 131**, because the low-res preview
//! image sits in between. So:
//!
//! - **7.3+**: read the resource dictionary and take the entry tagged
//!   `0x30, 0, 0` — the only reliable source.
//! - **7.0–7.2** (5 891 of the shipped files, so not a rare legacy path):
//!   there is no dictionary, and the image follows the low-res preview at
//!   `headerSize + low_res_bytes`.
//!
//! # Mip order and faces
//!
//! Mips are stored **smallest first**, and within each mip the order is frames
//! → faces → slices. So reaching mip 0 means skipping the entire chain above
//! it.
//!
//! An envmap stores **7 faces, not 6**: the seventh is a fallback spheremap
//! ([`vtf.h:143`] — "Cubemaps have *7* faces"). Rather than trust folklore
//! about which version dropped it, this was settled by arithmetic against the
//! real files — computing the total image size for each candidate count and
//! seeing which matches the bytes present. Verdict, unanimous per version:
//! **7 faces for 7.1 through 7.4** (48 files), **6 for 7.0** (2 files). No
//! 7.5 envmap ships with TF2, so that case follows the documented change and
//! is untested here.
//!
//! [`vtf.h:446`]: ../../../source-sdk-2013/src/public/vtf/vtf.h#L446
//! [`vtf.h:143`]: ../../../source-sdk-2013/src/public/vtf/vtf.h#L143

pub mod flags;
pub mod format;

pub use format::ImageFormat;

/// `"VTF\0"`.
pub const VTF_SIGNATURE: &[u8; 4] = b"VTF\0";

/// Offset of the 7.3+ resource dictionary. See the module docs: this is *not*
/// the end of the header struct.
const RESOURCES_AT: usize = 0x50;

/// Resource dictionary entries are `{ tag: [u8; 3], flags: u8, data: u32 }`.
const RESOURCE_LEN: usize = 8;

/// Valve caps the dictionary at `MAX_RSRC_DICTIONARY_ENTRIES`.
const MAX_RESOURCES: usize = 32;

/// Tag of the high-res image resource, `VTF_LEGACY_RSRC_IMAGE`.
pub const RSRC_IMAGE: [u8; 3] = [0x30, 0, 0];
/// Tag of the low-res preview image, `VTF_LEGACY_RSRC_LOW_RES_IMAGE`.
pub const RSRC_LOW_RES_IMAGE: [u8; 3] = [0x01, 0, 0];

/// `RSRCF_HAS_NO_DATA_CHUNK`: the entry's `data` word *is* the value, not an
/// offset. Set on the `CRC` and `LOD` entries.
const RSRCF_HAS_NO_DATA_CHUNK: u8 = 0x02;

#[derive(Debug, thiserror::Error)]
pub enum VtfError {
    #[error("not a VTF: expected {:?}, found {found:?}", VTF_SIGNATURE)]
    BadSignature { found: [u8; 4] },

    #[error("VTF version {major}.{minor} is not supported (this reader handles 7.0 to 7.5)")]
    UnsupportedVersion { major: i32, minor: i32 },

    #[error("truncated: {what} needs bytes up to {end} but the file is {len}")]
    Truncated {
        what: &'static str,
        end: usize,
        len: usize,
    },

    #[error("unknown image format index {index}")]
    UnknownFormat { index: i32 },

    #[error("header claims {width}x{height}, which is not a usable surface")]
    BadDimensions { width: u16, height: u16 },

    #[error("7.3+ header declares {count} resources, past the {MAX_RESOURCES} maximum")]
    TooManyResources { count: i64 },

    #[error("7.3+ header has no high-res image resource (tag {:02x?})", RSRC_IMAGE)]
    NoImageResource,

    #[error("mip {mip} does not exist: the texture has {count}")]
    NoSuchMip { mip: usize, count: usize },

    #[error("frame {frame} does not exist: the texture has {count}")]
    NoSuchFrame { frame: usize, count: usize },

    #[error("face {face} does not exist: the texture has {count}")]
    NoSuchFace { face: usize, count: usize },

    #[error(
        "image data for mip {mip} runs to {end} but only {len} bytes follow the \
         image offset"
    )]
    ImageTruncated { mip: usize, end: usize, len: usize },

    #[error(
        "{format} surface {width}x{height} needs {needed} bytes, got {got}"
    )]
    SurfaceTruncated {
        format: &'static str,
        width: u32,
        height: u32,
        needed: usize,
        got: usize,
    },

    #[error("no CPU decoder for {format}")]
    UnsupportedDecode { format: &'static str },
}

pub type Result<T> = std::result::Result<T, VtfError>;

/// The low-res preview image Source uses for average-colour queries.
#[derive(Clone, Copy, Debug)]
pub struct LowRes {
    pub format: ImageFormat,
    pub width: u8,
    pub height: u8,
    /// Offset into the file.
    pub offset: usize,
}

/// A parsed VTF, borrowing the file bytes.
#[derive(Clone, Debug)]
pub struct Vtf<'a> {
    pub major: i32,
    pub minor: i32,
    pub header_size: usize,
    pub width: u16,
    pub height: u16,
    /// Volume-texture depth; 1 for everything TF2 ships but one file.
    pub depth: u16,
    pub flags: u32,
    pub frames: u16,
    pub first_frame: u16,
    pub reflectivity: [f32; 3],
    pub bump_scale: f32,
    pub format: ImageFormat,
    pub mip_count: u8,
    pub low_res: Option<LowRes>,
    /// The high-res image block: everything from its offset to end of file.
    image: &'a [u8],
}

impl<'a> Vtf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Vtf<'a>> {
        let read = Reader { bytes };
        let signature = read.array4(0).ok_or(VtfError::Truncated {
            what: "signature",
            end: 4,
            len: bytes.len(),
        })?;
        if &signature != VTF_SIGNATURE {
            return Err(VtfError::BadSignature { found: signature });
        }

        // 0x40 covers every field through `lowResImageHeight`, which is all a
        // 7.0/7.1 file has.
        if bytes.len() < 0x40 {
            return Err(VtfError::Truncated {
                what: "header",
                end: 0x40,
                len: bytes.len(),
            });
        }
        let major = read.i32(0x04);
        let minor = read.i32(0x08);
        if major != 7 || !(0..=5).contains(&minor) {
            return Err(VtfError::UnsupportedVersion { major, minor });
        }

        let header_size = read.i32(0x0c).max(0) as usize;
        let width = read.u16(0x10);
        let height = read.u16(0x12);
        if width == 0 || height == 0 {
            return Err(VtfError::BadDimensions { width, height });
        }
        let format_index = read.i32(0x34);
        let format = ImageFormat::from_index(format_index)
            .ok_or(VtfError::UnknownFormat { index: format_index })?;

        // A `numMipLevels` of 0 means one level, not none.
        let mip_count = bytes[0x38].max(1);
        let low_format = ImageFormat::from_index(read.i32(0x39));
        let (low_width, low_height) = (bytes[0x3d], bytes[0x3e]);

        let depth = if minor >= 2 && bytes.len() >= 0x41 {
            read.u16(0x3f).max(1)
        } else {
            1
        };

        // 7.3+ carries a resource dictionary that says where everything is;
        // older versions have a fixed layout.
        let (image_offset, low_res) = if minor >= 3 {
            Self::locate_via_resources(&read, bytes, low_format, low_width, low_height)?
        } else {
            let low_res = low_format.map(|format| LowRes {
                format,
                width: low_width,
                height: low_height,
                offset: header_size,
            });
            let low_bytes = low_res.map_or(0, |low| {
                low.format
                    .surface_bytes(u32::from(low.width), u32::from(low.height))
                    as usize
            });
            (header_size + low_bytes, low_res)
        };

        let image = bytes.get(image_offset..).ok_or(VtfError::Truncated {
            what: "high-res image offset",
            end: image_offset,
            len: bytes.len(),
        })?;

        Ok(Vtf {
            major,
            minor,
            header_size,
            width,
            height,
            depth,
            flags: read.i32(0x14) as u32,
            frames: read.u16(0x18).max(1),
            first_frame: read.u16(0x1a),
            reflectivity: [read.f32(0x20), read.f32(0x24), read.f32(0x28)],
            bump_scale: read.f32(0x30),
            format,
            mip_count,
            low_res,
            image,
        })
    }

    /// Walk the 7.3+ resource dictionary for the image offsets.
    fn locate_via_resources(
        read: &Reader<'_>,
        bytes: &[u8],
        low_format: Option<ImageFormat>,
        low_width: u8,
        low_height: u8,
    ) -> Result<(usize, Option<LowRes>)> {
        if bytes.len() < RESOURCES_AT {
            return Err(VtfError::Truncated {
                what: "resource dictionary",
                end: RESOURCES_AT,
                len: bytes.len(),
            });
        }
        let count = i64::from(read.i32(0x44));
        if !(0..=MAX_RESOURCES as i64).contains(&count) {
            return Err(VtfError::TooManyResources { count });
        }

        let mut image_offset = None;
        let mut low_offset = None;
        for i in 0..count as usize {
            let at = RESOURCES_AT + i * RESOURCE_LEN;
            let entry = bytes.get(at..at + RESOURCE_LEN).ok_or(VtfError::Truncated {
                what: "resource entry",
                end: at + RESOURCE_LEN,
                len: bytes.len(),
            })?;
            let tag = [entry[0], entry[1], entry[2]];
            let entry_flags = entry[3];
            let data = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]) as usize;

            // `CRC` and `LOD` store their value in place of an offset; reading
            // one as an offset would send the image pointer into space.
            if entry_flags & RSRCF_HAS_NO_DATA_CHUNK != 0 {
                continue;
            }
            match tag {
                RSRC_IMAGE => image_offset = Some(data),
                RSRC_LOW_RES_IMAGE => low_offset = Some(data),
                _ => {}
            }
        }

        let image_offset = image_offset.ok_or(VtfError::NoImageResource)?;
        let low_res = match (low_format, low_offset) {
            (Some(format), Some(offset)) => Some(LowRes {
                format,
                width: low_width,
                height: low_height,
                offset,
            }),
            _ => None,
        };
        Ok((image_offset, low_res))
    }

    /// Faces per frame: 7 for an envmap on 7.1–7.4, else 6, else 1.
    ///
    /// See the module docs — this was measured, not assumed.
    pub fn face_count(&self) -> usize {
        if self.flags & flags::TEXTUREFLAGS_ENVMAP == 0 {
            return 1;
        }
        if (1..=4).contains(&self.minor) {
            7
        } else {
            6
        }
    }

    pub fn is_envmap(&self) -> bool {
        self.flags & flags::TEXTUREFLAGS_ENVMAP != 0
    }

    /// Whether the texture holds a tangent-space normal or an SSBUMP, and so
    /// must be sampled as linear data rather than sRGB colour.
    pub fn is_normal_map(&self) -> bool {
        self.flags & (flags::TEXTUREFLAGS_NORMAL | flags::TEXTUREFLAGS_SSBUMP) != 0
            || self.format == ImageFormat::Uv88
    }

    pub fn clamp_s(&self) -> bool {
        self.flags & flags::TEXTUREFLAGS_CLAMPS != 0
    }

    pub fn clamp_t(&self) -> bool {
        self.flags & flags::TEXTUREFLAGS_CLAMPT != 0
    }

    /// Dimensions of mip `mip`, where 0 is the largest. Never smaller than 1.
    pub fn mip_dimensions(&self, mip: usize) -> (u32, u32, u32) {
        let shift = mip as u32;
        (
            (u32::from(self.width) >> shift).max(1),
            (u32::from(self.height) >> shift).max(1),
            (u32::from(self.depth) >> shift).max(1),
        )
    }

    /// Bytes one mip level occupies for **all** frames, faces and slices.
    fn mip_bytes(&self, mip: usize) -> u64 {
        let (w, h, d) = self.mip_dimensions(mip);
        self.format.surface_bytes(w, h)
            * u64::from(self.frames)
            * self.face_count() as u64
            * u64::from(d)
    }

    /// Bytes one `(mip, frame, face, slice)` surface occupies.
    pub fn surface_bytes(&self, mip: usize) -> u64 {
        let (w, h, _) = self.mip_dimensions(mip);
        self.format.surface_bytes(w, h)
    }

    /// Total size of the image block, all mips.
    pub fn image_bytes(&self) -> u64 {
        (0..self.mip_count as usize).map(|m| self.mip_bytes(m)).sum()
    }

    /// The raw bytes of one surface, still in [`Vtf::format`].
    ///
    /// Mips are stored smallest-first, so mip 0 lives at the *end* of the
    /// block; within a mip the order is frame, then face, then slice.
    pub fn surface(&self, mip: usize, frame: usize, face: usize) -> Result<&'a [u8]> {
        self.surface_slice(mip, frame, face, 0)
    }

    pub fn surface_slice(
        &self,
        mip: usize,
        frame: usize,
        face: usize,
        slice: usize,
    ) -> Result<&'a [u8]> {
        let mip_count = self.mip_count as usize;
        if mip >= mip_count {
            return Err(VtfError::NoSuchMip {
                mip,
                count: mip_count,
            });
        }
        if frame >= self.frames as usize {
            return Err(VtfError::NoSuchFrame {
                frame,
                count: self.frames as usize,
            });
        }
        let faces = self.face_count();
        if face >= faces {
            return Err(VtfError::NoSuchFace { face, count: faces });
        }

        // Skip every mip smaller than this one — the chain runs from
        // `mip_count - 1` down to 0.
        let mut offset = 0u64;
        for smaller in ((mip + 1)..mip_count).rev() {
            offset += self.mip_bytes(smaller);
        }

        let surface = self.surface_bytes(mip);
        let (_, _, depth) = self.mip_dimensions(mip);
        let per_face = surface * u64::from(depth);
        let per_frame = per_face * faces as u64;
        offset += per_frame * frame as u64 + per_face * face as u64 + surface * slice as u64;

        let start = offset as usize;
        let end = start + surface as usize;
        self.image.get(start..end).ok_or(VtfError::ImageTruncated {
            mip,
            end,
            len: self.image.len(),
        })
    }

    /// Decode one surface to tightly packed RGBA8.
    pub fn decode_rgba8(&self, mip: usize, frame: usize, face: usize) -> Result<Vec<u8>> {
        let data = self.surface(mip, frame, face)?;
        let (w, h, _) = self.mip_dimensions(mip);
        self.format.decode_rgba8(data, w, h)
    }
}

/// Little-endian field reads at fixed offsets.
///
/// Bounds are checked by the caller before use; these panic on overrun, which
/// is why every entry point validates the header length first.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl Reader<'_> {
    fn array4(&self, at: usize) -> Option<[u8; 4]> {
        self.bytes.get(at..at + 4)?.try_into().ok()
    }

    fn u16(&self, at: usize) -> u16 {
        u16::from_le_bytes(self.bytes[at..at + 2].try_into().unwrap())
    }

    fn i32(&self, at: usize) -> i32 {
        i32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap())
    }

    fn f32(&self, at: usize) -> f32 {
        f32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic VTF. `minor` picks the header layout, so both the
    /// resource-dictionary path and the legacy path are exercised.
    ///
    /// The parameter list mirrors the header fields under test; bundling them
    /// into a struct would only move the same list one line up.
    #[allow(clippy::too_many_arguments)]
    fn synthetic(
        minor: i32,
        width: u16,
        height: u16,
        format: ImageFormat,
        mips: u8,
        frames: u16,
        envmap: bool,
        low_res: Option<(ImageFormat, u8, u8)>,
    ) -> Vec<u8> {
        let header_size: usize = match minor {
            0 | 1 => 64,
            2 => 80,
            _ => RESOURCES_AT + 2 * RESOURCE_LEN,
        };
        let mut out = vec![0u8; header_size];
        out[0..4].copy_from_slice(VTF_SIGNATURE);
        out[4..8].copy_from_slice(&7i32.to_le_bytes());
        out[8..12].copy_from_slice(&minor.to_le_bytes());
        out[12..16].copy_from_slice(&(header_size as i32).to_le_bytes());
        out[0x10..0x12].copy_from_slice(&width.to_le_bytes());
        out[0x12..0x14].copy_from_slice(&height.to_le_bytes());
        let flags: u32 = if envmap { flags::TEXTUREFLAGS_ENVMAP } else { 0 };
        out[0x14..0x18].copy_from_slice(&flags.to_le_bytes());
        out[0x18..0x1a].copy_from_slice(&frames.to_le_bytes());
        out[0x30..0x34].copy_from_slice(&1.0f32.to_le_bytes());
        out[0x34..0x38].copy_from_slice(&(format as i32).to_le_bytes());
        out[0x38] = mips;
        match low_res {
            Some((low_format, lw, lh)) => {
                out[0x39..0x3d].copy_from_slice(&(low_format as i32).to_le_bytes());
                out[0x3d] = lw;
                out[0x3e] = lh;
            }
            None => out[0x39..0x3d].copy_from_slice(&(-1i32).to_le_bytes()),
        }
        if minor >= 2 {
            out[0x3f..0x41].copy_from_slice(&1u16.to_le_bytes());
        }

        let low_bytes = low_res.map_or(0, |(f, w, h)| {
            f.surface_bytes(u32::from(w), u32::from(h)) as usize
        });

        if minor >= 3 {
            out[0x44..0x48].copy_from_slice(&2u32.to_le_bytes());
            // Entry 0: low-res image, immediately after the header.
            out[RESOURCES_AT..RESOURCES_AT + 3].copy_from_slice(&RSRC_LOW_RES_IMAGE);
            out[RESOURCES_AT + 4..RESOURCES_AT + 8]
                .copy_from_slice(&(header_size as u32).to_le_bytes());
            // Entry 1: high-res image, after the low-res one.
            let at = RESOURCES_AT + RESOURCE_LEN;
            out[at..at + 3].copy_from_slice(&RSRC_IMAGE);
            out[at + 4..at + 8]
                .copy_from_slice(&((header_size + low_bytes) as u32).to_le_bytes());
        }

        out.resize(header_size + low_bytes, 0xaa);

        // Image data, smallest mip first, filled with the mip index so a
        // mis-ordered read is detectable.
        let faces: u64 = if envmap {
            if (1..=4).contains(&minor) { 7 } else { 6 }
        } else {
            1
        };
        // A header `numMipLevels` of 0 still means one stored level, so the
        // fixture must write one — the same reading the parser applies.
        for mip in (0..mips.max(1) as usize).rev() {
            let w = (u32::from(width) >> mip).max(1);
            let h = (u32::from(height) >> mip).max(1);
            let bytes = format.surface_bytes(w, h) * u64::from(frames) * faces;
            out.extend(std::iter::repeat_n(mip as u8, bytes as usize));
        }
        out
    }

    #[test]
    fn parses_a_7_4_header_through_the_resource_dictionary() {
        let bytes = synthetic(
            4,
            64,
            64,
            ImageFormat::Dxt1,
            7,
            1,
            false,
            Some((ImageFormat::Dxt1, 16, 16)),
        );
        let vtf = Vtf::parse(&bytes).expect("parse");
        assert_eq!((vtf.major, vtf.minor), (7, 4));
        assert_eq!((vtf.width, vtf.height), (64, 64));
        assert_eq!(vtf.format, ImageFormat::Dxt1);
        assert_eq!(vtf.mip_count, 7, "64x64 is a 7-level chain");
        assert_eq!(vtf.face_count(), 1);
        let low = vtf.low_res.expect("low-res present");
        assert_eq!((low.width, low.height), (16, 16));
    }

    #[test]
    fn legacy_versions_place_the_image_after_the_low_res_preview() {
        // 7.1 has no resource dictionary. If the image offset were taken as
        // `headerSize`, mip 0 would read the low-res preview instead.
        let bytes = synthetic(
            1,
            32,
            32,
            ImageFormat::Dxt1,
            6,
            1,
            false,
            Some((ImageFormat::Dxt1, 16, 16)),
        );
        let vtf = Vtf::parse(&bytes).expect("parse");
        let mip0 = vtf.surface(0, 0, 0).expect("mip 0");
        assert!(
            mip0.iter().all(|&b| b == 0),
            "mip 0 should be filled with 0, got {:?}",
            &mip0[..8]
        );
        // The low-res filler is 0xaa; reading it would be the bug.
        assert!(!mip0.contains(&0xaa), "read into the low-res preview");
    }

    #[test]
    fn a_missing_low_res_image_contributes_no_offset() {
        // `lowResImageFormat == -1` on 144 shipped files.
        let bytes = synthetic(1, 16, 16, ImageFormat::Dxt1, 5, 1, false, None);
        let vtf = Vtf::parse(&bytes).expect("parse");
        assert!(vtf.low_res.is_none());
        assert_eq!(vtf.surface(0, 0, 0).expect("mip 0").len(), 128);
    }

    #[test]
    fn mips_are_read_smallest_first() {
        // Each mip is filled with its own index, so a chain walked from the
        // wrong end returns the wrong level.
        let bytes = synthetic(4, 16, 16, ImageFormat::Dxt1, 5, 1, false, None);
        let vtf = Vtf::parse(&bytes).expect("parse");
        for mip in 0..5 {
            let data = vtf.surface(mip, 0, 0).expect("surface");
            assert!(
                data.iter().all(|&b| b == mip as u8),
                "mip {mip} read the wrong level: {:?}",
                &data[..4]
            );
        }
    }

    #[test]
    fn every_mip_of_a_full_chain_is_one_block_at_the_small_end() {
        let bytes = synthetic(4, 8, 8, ImageFormat::Dxt5, 4, 1, false, None);
        let vtf = Vtf::parse(&bytes).expect("parse");
        assert_eq!(vtf.mip_dimensions(0), (8, 8, 1));
        assert_eq!(vtf.mip_dimensions(3), (1, 1, 1));
        // 8x8 -> 4 blocks, then 1, 1, 1: DXT5 is 16 bytes a block.
        assert_eq!(vtf.surface_bytes(0), 64);
        assert_eq!(vtf.surface_bytes(1), 16);
        assert_eq!(vtf.surface_bytes(3), 16);
        assert_eq!(vtf.image_bytes(), 64 + 16 + 16 + 16);
    }

    #[test]
    fn envmap_face_count_follows_the_measured_rule() {
        for (minor, expected) in [(0, 6), (1, 7), (2, 7), (3, 7), (4, 7), (5, 6)] {
            let bytes = synthetic(minor, 16, 16, ImageFormat::Dxt1, 5, 1, true, None);
            let vtf = Vtf::parse(&bytes).expect("parse");
            assert_eq!(
                vtf.face_count(),
                expected,
                "7.{minor} envmap should have {expected} faces"
            );
            // Every face must be addressable, and one past the end must not be.
            assert!(vtf.surface(0, 0, expected - 1).is_ok());
            assert!(matches!(
                vtf.surface(0, 0, expected),
                Err(VtfError::NoSuchFace { .. })
            ));
        }
    }

    #[test]
    fn frames_and_faces_are_addressed_independently() {
        let bytes = synthetic(4, 8, 8, ImageFormat::Dxt1, 1, 3, true, None);
        let vtf = Vtf::parse(&bytes).expect("parse");
        assert_eq!(vtf.frames, 3);
        assert_eq!(vtf.face_count(), 7);
        // 3 frames x 7 faces x one 8x8 DXT1 surface.
        assert_eq!(vtf.image_bytes(), 3 * 7 * 32);
        for frame in 0..3 {
            for face in 0..7 {
                assert!(vtf.surface(0, frame, face).is_ok(), "{frame}/{face}");
            }
        }
        assert!(matches!(
            vtf.surface(0, 3, 0),
            Err(VtfError::NoSuchFrame { .. })
        ));
    }

    #[test]
    fn a_zero_mip_count_means_one_level() {
        let bytes = synthetic(4, 8, 8, ImageFormat::Dxt1, 0, 1, false, None);
        let vtf = Vtf::parse(&bytes).expect("parse");
        assert_eq!(vtf.mip_count, 1);
        assert!(vtf.surface(0, 0, 0).is_ok());
    }

    #[test]
    fn rejects_files_that_are_not_vtfs() {
        assert!(matches!(
            Vtf::parse(b"DDS \0\0\0\0"),
            Err(VtfError::BadSignature { .. })
        ));
        assert!(matches!(
            Vtf::parse(b"VTF"),
            Err(VtfError::Truncated { .. })
        ));

        let mut bytes = synthetic(4, 8, 8, ImageFormat::Dxt1, 1, 1, false, None);
        bytes[8..12].copy_from_slice(&9i32.to_le_bytes());
        assert!(matches!(
            Vtf::parse(&bytes),
            Err(VtfError::UnsupportedVersion { minor: 9, .. })
        ));

        let mut bytes = synthetic(4, 8, 8, ImageFormat::Dxt1, 1, 1, false, None);
        bytes[0x34..0x38].copy_from_slice(&99i32.to_le_bytes());
        assert!(matches!(
            Vtf::parse(&bytes),
            Err(VtfError::UnknownFormat { index: 99 })
        ));
    }

    #[test]
    fn a_no_data_resource_is_not_mistaken_for_an_offset() {
        // The `CRC` entry's data word is a checksum. Read as an offset it
        // would point far outside the file — this is the tag flag's whole
        // purpose.
        let mut bytes = synthetic(4, 8, 8, ImageFormat::Dxt1, 1, 1, false, None);
        // Rewrite entry 0 as CRC-with-no-data-chunk carrying a huge value.
        bytes[RESOURCES_AT..RESOURCES_AT + 3].copy_from_slice(b"CRC");
        bytes[RESOURCES_AT + 3] = RSRCF_HAS_NO_DATA_CHUNK;
        bytes[RESOURCES_AT + 4..RESOURCES_AT + 8]
            .copy_from_slice(&0xdead_beefu32.to_le_bytes());
        let vtf = Vtf::parse(&bytes).expect("parse");
        assert!(vtf.low_res.is_none(), "CRC must not become the low-res image");
        assert!(vtf.surface(0, 0, 0).is_ok());
    }

    #[test]
    fn a_truncated_image_block_is_an_error_not_a_panic() {
        let mut bytes = synthetic(4, 64, 64, ImageFormat::Dxt5, 7, 1, false, None);
        bytes.truncate(bytes.len() - 100);
        let vtf = Vtf::parse(&bytes).expect("header still parses");
        assert!(matches!(
            vtf.surface(0, 0, 0),
            Err(VtfError::ImageTruncated { .. })
        ));
    }

    #[test]
    fn normal_maps_are_recognised_for_the_linear_view() {
        let mut bytes = synthetic(4, 8, 8, ImageFormat::Dxt5, 1, 1, false, None);
        bytes[0x14..0x18].copy_from_slice(&flags::TEXTUREFLAGS_NORMAL.to_le_bytes());
        assert!(Vtf::parse(&bytes).expect("parse").is_normal_map());

        // UV88 is a two-channel tangent normal even without the flag.
        let bytes = synthetic(4, 8, 8, ImageFormat::Uv88, 1, 1, false, None);
        assert!(Vtf::parse(&bytes).expect("parse").is_normal_map());

        let bytes = synthetic(4, 8, 8, ImageFormat::Dxt1, 1, 1, false, None);
        assert!(!Vtf::parse(&bytes).expect("parse").is_normal_map());
    }
}
