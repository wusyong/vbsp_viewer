//! `SURF_*` and `CONTENTS_*` bits from `source-sdk-2013/src/public/bspflags.h`.

/// Value will hold the light strength.
pub const SURF_LIGHT: i32 = 0x0001;
/// Don't draw; sky-light and draw the 2D sky, but not the 3D skybox.
pub const SURF_SKY2D: i32 = 0x0002;
/// Don't draw, but add to the skybox.
pub const SURF_SKY: i32 = 0x0004;
/// Turbulent water warp.
pub const SURF_WARP: i32 = 0x0008;
pub const SURF_TRANS: i32 = 0x0010;
/// The surface cannot have a portal placed on it.
pub const SURF_NOPORTAL: i32 = 0x0020;
/// Xbox hack around elimination of trigger surfaces.
pub const SURF_TRIGGER: i32 = 0x0040;
/// Don't bother referencing the texture.
pub const SURF_NODRAW: i32 = 0x0080;
/// Make a primary BSP splitter.
pub const SURF_HINT: i32 = 0x0100;
/// Completely ignore, allowing non-closed brushes.
pub const SURF_SKIP: i32 = 0x0200;
/// Don't calculate light.
pub const SURF_NOLIGHT: i32 = 0x0400;
/// Three extra lightmaps are stored for bumpmapping — this quadruples the
/// per-style luxel count in the LIGHTING lump.
pub const SURF_BUMPLIGHT: i32 = 0x0800;
pub const SURF_NOSHADOWS: i32 = 0x1000;
pub const SURF_NODECALS: i32 = 0x2000;
/// Don't subdivide patches on this surface.
pub const SURF_NOCHOP: i32 = 0x4000;
/// Surface is part of a hitbox.
pub const SURF_HITBOX: i32 = 0x8000;

/// vrad's `TEX_SPECIAL`: these faces carry no lightmap at all.
pub const TEX_SPECIAL: i32 = SURF_SKY | SURF_NOLIGHT;

/// Faces the map viewer never renders: tool brushes, sky placeholders, and
/// surfaces vbsp marked as non-drawing.
pub const SURF_SKIP_RENDER: i32 =
    SURF_NODRAW | SURF_SKY | SURF_SKY2D | SURF_HINT | SURF_SKIP | SURF_TRIGGER;

pub const CONTENTS_EMPTY: i32 = 0;
pub const CONTENTS_SOLID: i32 = 0x1;
/// Translucent, but not watery (glass).
pub const CONTENTS_WINDOW: i32 = 0x2;
pub const CONTENTS_AUX: i32 = 0x4;
/// Alpha-tested "grate" textures; bullets and sight pass through.
pub const CONTENTS_GRATE: i32 = 0x8;
pub const CONTENTS_SLIME: i32 = 0x10;
pub const CONTENTS_WATER: i32 = 0x20;
pub const CONTENTS_BLOCKLOS: i32 = 0x40;
pub const CONTENTS_OPAQUE: i32 = 0x80;
pub const CONTENTS_TESTFOGVOLUME: i32 = 0x100;
pub const CONTENTS_TEAM1: i32 = 0x800;
pub const CONTENTS_TEAM2: i32 = 0x1000;
pub const CONTENTS_IGNORE_NODRAW_OPAQUE: i32 = 0x2000;
pub const CONTENTS_MOVEABLE: i32 = 0x4000;
pub const CONTENTS_AREAPORTAL: i32 = 0x8000;
pub const CONTENTS_PLAYERCLIP: i32 = 0x10000;
pub const CONTENTS_MONSTERCLIP: i32 = 0x20000;
/// Removed before bsping an entity.
pub const CONTENTS_ORIGIN: i32 = 0x1000000;
pub const CONTENTS_MONSTER: i32 = 0x2000000;
pub const CONTENTS_DEBRIS: i32 = 0x4000000;
/// Brushes added after vis leaves.
pub const CONTENTS_DETAIL: i32 = 0x8000000;
/// Auto-set if any surface has transparency.
pub const CONTENTS_TRANSLUCENT: i32 = 0x10000000;
pub const CONTENTS_LADDER: i32 = 0x20000000;
pub const CONTENTS_HITBOX: i32 = 0x40000000;

/// `DISPTRI_*` tags on [`crate::raw::DispTri`].
pub const DISPTRI_TAG_SURFACE: u16 = 1 << 0;
pub const DISPTRI_TAG_WALKABLE: u16 = 1 << 1;
pub const DISPTRI_TAG_BUILDABLE: u16 = 1 << 2;
pub const DISPTRI_FLAG_SURFPROP1: u16 = 1 << 3;
pub const DISPTRI_FLAG_SURFPROP2: u16 = 1 << 4;
pub const DISPTRI_TAG_REMOVE: u16 = 1 << 5;
