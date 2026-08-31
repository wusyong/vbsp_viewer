//! `TEXTUREFLAGS_*` from [`public/vtf/vtf.h:35`].
//!
//! Counts are how many of the 34 000 shipped VTFs set each bit, which is the
//! best guide to which ones matter.
//!
//! [`public/vtf/vtf.h:35`]: ../../../source-sdk-2013/src/public/vtf/vtf.h#L35

/// Nearest-neighbour sampling. 267 files.
pub const TEXTUREFLAGS_POINTSAMPLE: u32 = 0x0000_0001;
/// 69 files.
pub const TEXTUREFLAGS_TRILINEAR: u32 = 0x0000_0002;
/// Clamp instead of wrap on S. 1 427 files.
pub const TEXTUREFLAGS_CLAMPS: u32 = 0x0000_0004;
/// Clamp instead of wrap on T. 1 431 files.
pub const TEXTUREFLAGS_CLAMPT: u32 = 0x0000_0008;
/// 85 files.
pub const TEXTUREFLAGS_ANISOTROPIC: u32 = 0x0000_0010;
pub const TEXTUREFLAGS_HINT_DXT5: u32 = 0x0000_0020;
/// Set on 7 659 files — a minority, so this is *not* a reliable signal for
/// whether a texture is colour data. The shader decides that from its role.
pub const TEXTUREFLAGS_SRGB: u32 = 0x0000_0040;
/// A tangent-space normal map: must be sampled linear, never sRGB. 3 213 files.
pub const TEXTUREFLAGS_NORMAL: u32 = 0x0000_0080;
/// 12 451 files — mostly UI and effect textures.
pub const TEXTUREFLAGS_NOMIP: u32 = 0x0000_0100;
pub const TEXTUREFLAGS_NOLOD: u32 = 0x0000_0200;
pub const TEXTUREFLAGS_ALL_MIPS: u32 = 0x0000_0400;
pub const TEXTUREFLAGS_PROCEDURAL: u32 = 0x0000_0800;
/// The alpha channel is a 1-bit mask. 118 files.
pub const TEXTUREFLAGS_ONEBITALPHA: u32 = 0x0000_1000;
/// The alpha channel is a real gradient. 24 422 files — the most common flag.
pub const TEXTUREFLAGS_EIGHTBITALPHA: u32 = 0x0000_2000;
/// A cubemap, which changes the face count. 50 files. See `Vtf::face_count`.
pub const TEXTUREFLAGS_ENVMAP: u32 = 0x0000_4000;
pub const TEXTUREFLAGS_RENDERTARGET: u32 = 0x0000_8000;
pub const TEXTUREFLAGS_DEPTHRENDERTARGET: u32 = 0x0001_0000;
pub const TEXTUREFLAGS_NODEBUGOVERRIDE: u32 = 0x0002_0000;
pub const TEXTUREFLAGS_SINGLECOPY: u32 = 0x0004_0000;
pub const TEXTUREFLAGS_NODEPTHBUFFER: u32 = 0x0080_0000;
pub const TEXTUREFLAGS_CLAMPU: u32 = 0x0200_0000;
pub const TEXTUREFLAGS_VERTEXTEXTURE: u32 = 0x0400_0000;
/// Valve's self-shadowing bump format: linear data, like a normal map.
pub const TEXTUREFLAGS_SSBUMP: u32 = 0x0800_0000;
pub const TEXTUREFLAGS_BORDER: u32 = 0x2000_0000;
pub const TEXTUREFLAGS_STREAMABLE_COARSE: u32 = 0x4000_0000;
pub const TEXTUREFLAGS_STREAMABLE_FINE: u32 = 0x8000_0000;

/// Any alpha at all, for choosing a blend mode.
pub const TEXTUREFLAGS_ANY_ALPHA: u32 = TEXTUREFLAGS_ONEBITALPHA | TEXTUREFLAGS_EIGHTBITALPHA;

/// Names of the set bits, for the header dump.
pub fn describe(flags: u32) -> Vec<&'static str> {
    const NAMED: [(u32, &str); 25] = [
        (TEXTUREFLAGS_POINTSAMPLE, "POINTSAMPLE"),
        (TEXTUREFLAGS_TRILINEAR, "TRILINEAR"),
        (TEXTUREFLAGS_CLAMPS, "CLAMPS"),
        (TEXTUREFLAGS_CLAMPT, "CLAMPT"),
        (TEXTUREFLAGS_ANISOTROPIC, "ANISOTROPIC"),
        (TEXTUREFLAGS_HINT_DXT5, "HINT_DXT5"),
        (TEXTUREFLAGS_SRGB, "SRGB"),
        (TEXTUREFLAGS_NORMAL, "NORMAL"),
        (TEXTUREFLAGS_NOMIP, "NOMIP"),
        (TEXTUREFLAGS_NOLOD, "NOLOD"),
        (TEXTUREFLAGS_ALL_MIPS, "ALL_MIPS"),
        (TEXTUREFLAGS_PROCEDURAL, "PROCEDURAL"),
        (TEXTUREFLAGS_ONEBITALPHA, "ONEBITALPHA"),
        (TEXTUREFLAGS_EIGHTBITALPHA, "EIGHTBITALPHA"),
        (TEXTUREFLAGS_ENVMAP, "ENVMAP"),
        (TEXTUREFLAGS_RENDERTARGET, "RENDERTARGET"),
        (TEXTUREFLAGS_DEPTHRENDERTARGET, "DEPTHRENDERTARGET"),
        (TEXTUREFLAGS_NODEBUGOVERRIDE, "NODEBUGOVERRIDE"),
        (TEXTUREFLAGS_SINGLECOPY, "SINGLECOPY"),
        (TEXTUREFLAGS_NODEPTHBUFFER, "NODEPTHBUFFER"),
        (TEXTUREFLAGS_CLAMPU, "CLAMPU"),
        (TEXTUREFLAGS_VERTEXTEXTURE, "VERTEXTEXTURE"),
        (TEXTUREFLAGS_SSBUMP, "SSBUMP"),
        (TEXTUREFLAGS_BORDER, "BORDER"),
        (TEXTUREFLAGS_STREAMABLE_COARSE, "STREAMABLE_COARSE"),
    ];
    NAMED
        .iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|(_, name)| *name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_values_match_the_sdk() {
        // Transcription check against vtf.h; a wrong bit here would silently
        // sample a normal map as sRGB, or wrap a clamped texture.
        assert_eq!(TEXTUREFLAGS_CLAMPS, 0x4);
        assert_eq!(TEXTUREFLAGS_CLAMPT, 0x8);
        assert_eq!(TEXTUREFLAGS_SRGB, 0x40);
        assert_eq!(TEXTUREFLAGS_NORMAL, 0x80);
        assert_eq!(TEXTUREFLAGS_EIGHTBITALPHA, 0x2000);
        assert_eq!(TEXTUREFLAGS_ENVMAP, 0x4000);
        assert_eq!(TEXTUREFLAGS_SSBUMP, 0x0800_0000);
    }

    #[test]
    fn describe_lists_only_set_bits() {
        let named = describe(TEXTUREFLAGS_ENVMAP | TEXTUREFLAGS_NORMAL);
        assert_eq!(named, ["NORMAL", "ENVMAP"]);
        assert!(describe(0).is_empty());
    }
}
