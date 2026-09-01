//! The 2D skybox: six quads, six native-size 2D textures.
//!
//! # Source draws the sky in 2D, not as a cubemap
//!
//! This is the whole design, and it is not an approximation of what the engine
//! does — it is what the engine does. The `Sky` shader takes `VERTEX_POSITION`
//! plus **one 2D texcoord set** (`sky_dx9.cpp:59`), samples with `tex2D`
//! (`sky_ps2x.fxc`), and falls back to plain `UnlitGeneric` on dx6
//! (`sky_dx6.cpp:13`). `texCUBE` appears in no sky shader file. The engine's own
//! sky interface is six *materials*:
//!
//! ```c
//! struct SkyBoxMaterials_t
//! {
//!     // order: "rt", "bk", "lf", "ft", "up", "dn"
//!     IMaterial *material[6];
//! };
//! ```
//! — `public/cdll_int.h:123`
//!
//! Six separate 2D textures is also the only thing consistent with the files:
//! `sky_badlands_01`'s sides are 512x256 and its caps 512x512, which no cubemap
//! can hold. Every face carries `CLAMPS | CLAMPT | NOMIP | NOLOD`, so the engine
//! never filters across a face join and never leaves mip 0.
//!
//! # Why that matters
//!
//! An earlier attempt assembled one cubemap and searched for the face
//! arrangement by edge continuity. It was reverted. Two things were wrong with
//! it, and both are structural rather than a tuning problem:
//!
//! - **`textureSample` on a `texture_cube` blends across the face join.** That
//!   is smoother than Source when the arrangement is exactly right, and a hard
//!   line on four of the twelve edges when one face is turned wrong. Six clamped
//!   2D faces make cross-face filtering *impossible*, so a wrong face shows up
//!   as wrong content instead of as a hairline that reads as a rendering bug.
//! - **The continuity search had no signal.** TF2 skies are smooth gradients, so
//!   the minimum is a plateau: run over four skies it produced four different
//!   arrangements, and the cheapest single-face change cost +0.00 on every one.
//!
//! Here the four sides fall out of the axes ([`SkyFace::axes`]) and only the two
//! caps need measuring — a well-posed problem, because the sides pin them.
//!
//! # No resampling
//!
//! Each face keeps its own dimensions. The old code stretched 512x256 sides onto
//! a 512x512 cube face, so every side was filtered twice: once at load and again
//! when sampled. Faces are decoded to RGBA8 mip 0 rather than passed through as
//! BC blocks — six textures is a few MiB at most, and having the pixels on the
//! CPU is what makes the edge and sun checks possible at all.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, Mesh, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use vfs::{vmt, Vfs};

/// Where the sky shader lives once embedded in the binary.
const SHADER_PATH: &str = "embedded://bevy_bsp/sky_material.wgsl";

/// One of the six sky faces, named as Source names them.
///
/// The suffix is the filename suffix (`sky_badlands_01` + `rt` + `.vmt`) and the
/// direction comes from `CubeMapFaceIndex_t`'s own comments
/// (`public/vtf/vtf.h:135`), which are worth quoting because the enum names read
/// as wrong until you notice they are in **Source's Z-up axes**:
///
/// ```c
/// CUBEMAP_FACE_BACK,   // NOTE: This face is in the +y direction?!?!?
/// CUBEMAP_FACE_FRONT,  // NOTE: This face is in the -y direction!?!?
/// ```
///
/// # Two orderings ship, and `bk`/`lf` are swapped between them
///
/// The **drawn sky** uses `rt, bk, lf, ft, up, dn` (`cdll_int.h:125`,
/// `skyboxswapper.cpp:60`, `vscript_server.cpp:2190`); the **reflection
/// cubemap** uses `rt, lf, bk, ft, up, dn` (`vbsp/cubemap.cpp:195`). Anything
/// that indexes a six-element list *positionally* from the wrong file is
/// silently wrong. Nothing here does: directions come from the per-face table in
/// [`SkyFace::axes`], never from slot position, and [`SkyFace::ALL`] exists only
/// so callers have a stable iteration order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SkyFace {
    Rt,
    Lf,
    Bk,
    Ft,
    Up,
    Dn,
}

impl SkyFace {
    /// A stable iteration order. Deliberately *not* either of Valve's two
    /// orderings, so it can never be mistaken for one.
    pub const ALL: [SkyFace; 6] = [
        SkyFace::Rt,
        SkyFace::Lf,
        SkyFace::Bk,
        SkyFace::Ft,
        SkyFace::Up,
        SkyFace::Dn,
    ];

    /// The filename suffix, e.g. `rt` in `sky_badlands_01rt.vmt`.
    pub fn suffix(self) -> &'static str {
        match self {
            SkyFace::Rt => "rt",
            SkyFace::Lf => "lf",
            SkyFace::Bk => "bk",
            SkyFace::Ft => "ft",
            SkyFace::Up => "up",
            SkyFace::Dn => "dn",
        }
    }

    /// Index into [`SkyFace::ALL`].
    pub fn index(self) -> usize {
        match self {
            SkyFace::Rt => 0,
            SkyFace::Lf => 1,
            SkyFace::Bk => 2,
            SkyFace::Ft => 3,
            SkyFace::Up => 4,
            SkyFace::Dn => 5,
        }
    }

    /// Whether this is one of the two caps rather than one of the four sides.
    pub fn is_cap(self) -> bool {
        matches!(self, SkyFace::Up | SkyFace::Dn)
    }

    /// How this face sits on the box, in **Source's Z-up axes**.
    ///
    /// # The four sides are determined, not searched
    ///
    /// Each sky face is a photograph taken looking along its direction, so with
    /// the image the right way up its `u` runs to the viewer's right and its `v`
    /// runs down. Standing at the origin facing Source `+X` with `+Z` up, your
    /// right hand points at `-Y`; down is `-Z`. That is the whole derivation:
    ///
    /// | face | outward | `u` along | `v` along |
    /// |---|---|---|---|
    /// | `rt` | +X | −Y | −Z |
    /// | `lf` | −X | +Y | −Z |
    /// | `bk` | +Y | +X | −Z |
    /// | `ft` | −Y | −X | −Z |
    ///
    /// Read around the horizon in the direction `u` increases, the sides join
    /// `rt → ft → lf → bk → rt`, and each shared edge lands on the same two box
    /// coordinates from both sides — which is what `sides_join_without_a_gap`
    /// asserts.
    ///
    /// # The caps are measured
    ///
    /// With the sides pinned there are exactly four candidate orientations per
    /// cap ([`cap_rotations`]), and `up` is fixed by its four edges against the
    /// sides. [`UP_ROTATION`] records the measurement. `dn` is undeterminable
    /// from anything TF2 ships and is a labelled convention — see
    /// [`DN_ROTATION`].
    pub fn axes(self) -> FaceAxes {
        match self {
            SkyFace::Rt => FaceAxes::new([1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]),
            SkyFace::Lf => FaceAxes::new([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]),
            SkyFace::Bk => FaceAxes::new([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
            SkyFace::Ft => FaceAxes::new([0.0, -1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
            SkyFace::Up => cap_rotations(SkyFace::Up)[UP_ROTATION],
            SkyFace::Dn => cap_rotations(SkyFace::Dn)[DN_ROTATION],
        }
    }
}

/// Where a face sits on the box and how its image lies on it, in Source axes.
///
/// The invariant is **`u × v == outward`**. It says the image is not mirrored:
/// each face is a photograph, so no reflection should be needed anywhere. It
/// also cuts each cap's candidate set from eight orientations to four, which is
/// what makes the cap measurement tractable — see `axes_are_not_mirrored`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceAxes {
    /// Unit axis pointing out of the box through this face.
    pub outward: [f32; 3],
    /// Unit axis along which the image's `u` increases.
    pub u: [f32; 3],
    /// Unit axis along which the image's `v` increases. `v = 0` is the top row.
    pub v: [f32; 3],
}

impl FaceAxes {
    pub const fn new(outward: [f32; 3], u: [f32; 3], v: [f32; 3]) -> Self {
        FaceAxes { outward, u, v }
    }

    /// The point on the unit box (half-extent 1) at face coordinates `(u, v)`.
    pub fn point(&self, u: f32, v: f32) -> [f32; 3] {
        let du = 2.0 * u - 1.0;
        let dv = 2.0 * v - 1.0;
        [
            self.outward[0] + self.u[0] * du + self.v[0] * dv,
            self.outward[1] + self.u[1] * du + self.v[1] * dv,
            self.outward[2] + self.u[2] * du + self.v[2] * dv,
        ]
    }

    /// Face coordinates of a point on the unit box.
    ///
    /// Exact inverse of [`FaceAxes::point`] because `outward`, `u` and `v` are
    /// mutually perpendicular unit axes, so `dot(point(u, v), u) == 2u - 1`.
    pub fn coords(&self, p: [f32; 3]) -> (f32, f32) {
        ((dot(p, self.u) + 1.0) * 0.5, (dot(p, self.v) + 1.0) * 0.5)
    }
}

/// The four orientations of a cap face that leave the image un-mirrored.
///
/// Generated rather than written out so the `u × v == outward` invariant cannot
/// drift: from a starting pair `(a, b)` with `a × b == outward`, quarter turns
/// are `(a, b) → (b, −a) → (−a, −b) → (−b, a)`.
pub fn cap_rotations(face: SkyFace) -> [FaceAxes; 4] {
    let (outward, a, b) = match face {
        SkyFace::Up => ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        SkyFace::Dn => ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        other => panic!("{other:?} is a side face, not a cap"),
    };
    [
        FaceAxes::new(outward, a, b),
        FaceAxes::new(outward, b, neg(a)),
        FaceAxes::new(outward, neg(a), neg(b)),
        FaceAxes::new(outward, neg(b), a),
    ]
}

/// Which of [`cap_rotations`] the `up` face uses.
///
/// **Measured: rot 3, on 38 of the 41 skies that carry a verdict.** `skydump
/// --caps` reprints the whole table; the numbers are the mean per-channel
/// difference along the four `up`-to-side edges, so lower is better. A sample of
/// the margins:
///
/// | sky | rot0 | rot1 | rot2 | **rot3** |
/// |---|---|---|---|---|
/// | `sky_day01_01` | 17.02 | 17.65 | 16.92 | **0.89** |
/// | `sky_hydro_01` | 12.80 | 12.81 | 12.70 | **1.07** |
/// | `sky_premuda_` | 17.23 | 20.89 | 16.93 | **0.53** |
/// | `sky_nightsnow_01` | 18.01 | 12.64 | 18.07 | **2.71** |
/// | `sky_alpinestorm_01` | 5.93 | 6.46 | 5.91 | **0.42** |
///
/// Ten- to twentyfold, and the same answer on 38 skies. Of the remaining 30
/// skynames, all but three are near-uniform in `up` and score flat across all
/// four rotations; those are excluded rather than averaged in, because a flat cap
/// votes for whichever rotation the tie-break happens to reach. Three dissent
/// with a real margin — `sky_cargo_01` and `sky_fuji` prefer rot 0,
/// `sky_frankenstorm_02` rot 2 — and are left as known outliers rather than
/// explained away.
///
/// An earlier cubemap attempt measured "rot 1 plus a mirror" from four skies. The
/// mirror was an artifact of wgpu's cube face conventions and is absent here,
/// which is why the rotation index differs; nothing about the artwork changed.
pub const UP_ROTATION: usize = 3;

/// Which of [`cap_rotations`] the `dn` face uses.
///
/// **Barely measurable, and treated as a convention.** Only **2 of 71** skynames
/// carry any verdict at all, because a down face is nearly always a stand-in:
/// most either point at another face's texture (`sky_hydro_01dn` reuses `bk`),
/// ship a 1x1-to-256x256 placeholder, or are a single flat colour. The two that
/// do agree:
///
/// | sky | rot0 | **rot1** | rot2 | rot3 |
/// |---|---|---|---|---|
/// | `overcast01` | 4.44 | **0.47** | 4.56 | 6.35 |
/// | `sky_hadal_01` | 8.84 | **2.82** | 8.91 | 9.32 |
///
/// Two skies is not a measurement. What promotes rot 1 over a coin flip is that
/// it is also what [`UP_ROTATION`] implies: `up` at rot 3 is `u = -Y, v = +X`,
/// and `dn` at rot 1 is `u = -Y, v = -X` — the same face reflected across the
/// horizon, which is what the two caps of one photographed sky should be. Take
/// the number as a convention that happens to have two votes behind it. It is
/// below the horizon, so nothing in a map looks at it, and
/// `skydump --all-maps` deliberately excludes it from the gate.
pub const DN_ROTATION: usize = 1;

/// One decoded sky face: mip 0, RGBA8, at the VTF's own size.
#[derive(Clone, Debug)]
pub struct SkyFaceTexture {
    pub face: SkyFace,
    pub width: u32,
    pub height: u32,
    /// Mip 0 as RGBA8, sRGB-encoded as the VTF stores it.
    pub rgba: Vec<u8>,
    /// The VTF this came from, or `None` for a `$color`-only face.
    pub source: Option<String>,
}

impl SkyFaceTexture {
    /// A flat face of one colour, for a VMT with no `$basetexture`.
    ///
    /// `sky_hacksaw_01dn.vmt` is `"sky" { $nofog 1  $ignorez 1  $color "0 0 0" }`
    /// — no texture at all.
    pub fn solid(face: SkyFace, colour: [u8; 4]) -> Self {
        SkyFaceTexture {
            face,
            width: 1,
            height: 1,
            rgba: colour.to_vec(),
            source: None,
        }
    }

    pub fn texel(&self, x: u32, y: u32) -> [u8; 4] {
        let x = x.min(self.width - 1) as usize;
        let y = y.min(self.height - 1) as usize;
        let i = (y * self.width as usize + x) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    /// Nearest texel at face coordinates `(u, v)`, clamped.
    ///
    /// Clamping rather than wrapping is `CLAMPS | CLAMPT`, and it is why a
    /// coordinate landing exactly on an edge reads that edge's texel instead of
    /// the opposite side's.
    pub fn sample_clamped(&self, u: f32, v: f32) -> [u8; 4] {
        let x = (u * self.width as f32)
            .floor()
            .clamp(0.0, (self.width - 1) as f32) as u32;
        let y = (v * self.height as f32)
            .floor()
            .clamp(0.0, (self.height - 1) as f32) as u32;
        self.texel(x, y)
    }

    /// How much of the square cube face this texture's height covers.
    ///
    /// # A half-height face fills the top half, it does not stretch
    ///
    /// Sky faces are not all the same shape. `sky_badlands_01`'s sides are
    /// 512x256 against 512x512 caps, and `sky_alpinestorm_01` mixes 1024x512
    /// sides with a **1024x1024 `bk`** — one square face among three
    /// half-height ones. A cube face is square, so something has to give, and
    /// the two candidate rules are distinguishable on exactly that kind of sky:
    ///
    /// - *Stretch* the texture over the whole face. Then `bk`'s horizon lands at
    ///   +2 degrees elevation and `rt`'s at -40, and the two joins that involve
    ///   `bk` score **15.02 and 14.31** while every join between two half-height
    ///   sides scores 0.25.
    /// - *Fit* it into the top of the face at 1:1 and let the bottom repeat the
    ///   last row. Then every face's horizon lands at the same elevation and
    ///   those joins drop to the same fraction of a unit as the rest.
    ///
    /// The second is right, and it is `CreateDefaultCubemaps`' rule
    /// (`vbsp/cubemap.cpp`), which fits a face twice as wide as tall into the
    /// cube face's top half and repeats its last row down the bottom. An earlier
    /// attempt dismissed that as reflection-only and stretched instead; the
    /// mixed-height skies say otherwise.
    ///
    /// It is also why the faces are flagged `CLAMPT`. The renderer needs no
    /// special case at all: the quad's `v` runs 0 to this value, and clamped
    /// addressing repeats the last row for everything past 1. Source sets the
    /// flag that makes the smear happen for free.
    ///
    /// Faces taller than wide do not occur in TF2 and are left alone rather than
    /// guessed at.
    pub fn vertical_fit(&self) -> f32 {
        if self.height == 0 {
            return 1.0;
        }
        (self.width as f32 / self.height as f32).max(1.0)
    }

    /// Sample by *cube-face* coordinate, applying [`SkyFaceTexture::vertical_fit`].
    ///
    /// [`SkyFaceTexture::sample_clamped`] takes texture coordinates; this takes
    /// the position on the face. Everything geometric — the edge report, the
    /// compass profile, the quad UVs — works in face coordinates, so this is the
    /// one to reach for.
    pub fn sample_face(&self, u: f32, v: f32) -> [u8; 4] {
        self.sample_clamped(u, v * self.vertical_fit())
    }

    /// Relative luminance of a texel, 0..255.
    pub fn luma(&self, x: u32, y: u32) -> f32 {
        let t = self.texel(x, y);
        0.2126 * t[0] as f32 + 0.7152 * t[1] as f32 + 0.0722 * t[2] as f32
    }
}

/// A resolved sky: its name and all six faces, in [`SkyFace::ALL`] order.
#[derive(Clone, Debug)]
pub struct Sky {
    pub name: String,
    pub faces: Vec<SkyFaceTexture>,
}

impl Sky {
    pub fn face(&self, face: SkyFace) -> &SkyFaceTexture {
        &self.faces[face.index()]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkyError {
    #[error("{skyname}{suffix}: {source}")]
    Vmt {
        skyname: String,
        suffix: &'static str,
        source: vmt::VmtError,
    },
    #[error("{path}: {source}")]
    Texture {
        path: String,
        source: vfs::VfsError,
    },
    #[error("{path}: {source}")]
    Decode { path: String, source: vtf::VtfError },
}

/// Load all six faces of a sky through the VFS.
///
/// # The texture comes from `$basetexture`, never from the filename
///
/// `sky_hydro_01dn.vmt` points at `sky_hydro_01bk`, and `sky_dustbowl_01rt.vtf`
/// does not exist under that name at all. vbsp reads `$basetexture` even on the
/// HDR path — its own comment: *"Since we're setting it to black anyway, just
/// use `$basetexture` for HDR"* (`vbsp/cubemap.cpp:203`) — which settles LDR as
/// the default. TF2's own skies point `$hdrbasetexture` at the LDR texture
/// anyway.
///
/// A whole sky can be absent: `sky_black_01` on `pd_atom_smash` is in no VPK and
/// in no pakfile. That is an `Err` for the caller to report, not a panic — the
/// engine draws its missing-material checkerboard and carries on.
pub fn load(vfs: &Vfs, skyname: &str) -> Result<Sky, SkyError> {
    let mut faces = Vec::with_capacity(6);
    for face in SkyFace::ALL {
        faces.push(load_face(vfs, skyname, face)?);
    }
    Ok(Sky {
        name: skyname.to_string(),
        faces,
    })
}

fn load_face(vfs: &Vfs, skyname: &str, face: SkyFace) -> Result<SkyFaceTexture, SkyError> {
    let suffix = face.suffix();
    let material = vmt::load(vfs, &format!("materials/skybox/{skyname}{suffix}.vmt")).map_err(
        |source| SkyError::Vmt {
            skyname: skyname.to_string(),
            suffix,
            source,
        },
    )?;

    let Some(path) = material.base_texture() else {
        return Ok(SkyFaceTexture::solid(face, colour_param(&material)));
    };

    let bytes = vfs.read(&path).map_err(|source| SkyError::Texture {
        path: path.clone(),
        source,
    })?;
    let vtf = vtf::Vtf::parse(&bytes).map_err(|source| SkyError::Decode {
        path: path.clone(),
        source,
    })?;
    let frame = usize::from(vtf.first_frame).min(usize::from(vtf.frames) - 1);
    // Mip 0 only. The VTFs ship a full chain but are flagged NOMIP | NOLOD, so
    // the engine never leaves the base level; decoding one level is how that
    // becomes true for us too.
    let rgba = vtf
        .decode_rgba8(0, frame, 0)
        .map_err(|source| SkyError::Decode {
            path: path.clone(),
            source,
        })?;
    let (width, height, _) = vtf.mip_dimensions(0);
    Ok(SkyFaceTexture {
        face,
        width,
        height,
        rgba,
        source: Some(path),
    })
}

/// `$color`, in both spellings Source accepts: `"0 0 0"` and `"[0 0 0]"`.
fn colour_param(material: &vmt::Material) -> [u8; 4] {
    let Some(raw) = material
        .params
        .string("$color")
        .or_else(|| material.params.string("$color2"))
    else {
        return [0, 0, 0, 255];
    };
    let mut out = [0u8, 0, 0, 255];
    let cleaned = raw
        .trim()
        .trim_start_matches(['[', '{'])
        .trim_end_matches([']', '}']);
    for (i, part) in cleaned.split_whitespace().take(3).enumerate() {
        let v: f32 = part.parse().unwrap_or(0.0);
        out[i] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    out
}

/// Upload one face as a texture.
///
/// `mip_level_count` stays 1 and addressing is `ClampToEdge`: together those are
/// `CLAMPS | CLAMPT | NOMIP | NOLOD`, and they are what make a seam impossible
/// rather than unlikely. With one level there is no coarser mip for the sampler
/// to select at the grazing angles near the horizon, and with clamping a
/// coordinate on the face border reads that border's texel instead of blending
/// with a neighbour it does not adjoin.
pub fn face_image(face: &SkyFaceTexture) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: face.width,
            height: face.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = 1;
    image.data = Some(face.rgba.clone());
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Nearest,
        ..default()
    });
    image
}

/// How far each quad is grown past its half of the box, as a fraction.
///
/// Two abutting quads can leave a one-pixel rasterization crack along their
/// shared edge. Growing each quad slightly makes the edges overlap instead, and
/// because the UVs are *not* grown with them, the overlap shows the same clamped
/// edge texel the join already showed. 0.2% at a 600 m radius is over a metre of
/// overlap — far more than any crack, and still well under one texel of sky.
pub const QUAD_OVERLAP: f32 = 0.002;

/// One face's quad, in Bevy space.
///
/// `radius` is the box half-extent, and `vertical_fit` comes from
/// [`SkyFaceTexture::vertical_fit`]. Backface culling is disabled by
/// [`SkyMaterial::specialize`] rather than relying on winding: the quads are
/// viewed from inside, and a sky that is invisible because its triangles face
/// outward is a tediously easy bug to reintroduce.
pub fn face_mesh(face: SkyFace, radius: f32, vertical_fit: f32) -> Mesh {
    let axes = face.axes();
    let grow = 1.0 + QUAD_OVERLAP;

    let mut positions = Vec::with_capacity(4);
    let mut uvs = Vec::with_capacity(4);
    for (u, v) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        // Grown about the face centre, in the face's own plane only, so the box
        // stays a box.
        let p = axes.point(0.5 + (u - 0.5) * grow, 0.5 + (v - 0.5) * grow);
        positions.push((crate::src_to_bevy_dir(p) * radius).to_array());
        // `v` past 1 is what makes a half-height face repeat its last row
        // instead of stretching; `ClampToEdge` does the repeating. See
        // `SkyFaceTexture::vertical_fit`.
        uvs.push([u, v * vertical_fit]);
    }
    // Inward normal: the box is seen from inside.
    let normal = (-crate::src_to_bevy_dir(axes.outward)).to_array();

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![normal; 4])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(vec![0, 2, 1, 1, 2, 3]))
}

// ---------------------------------------------------------------------------
// Material
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct SkyParams {
    /// Multiplier taking a 0..1 sky colour into Bevy's photometric range.
    pub brightness: f32,
    pub _padding: Vec3,
}

/// The material one sky face draws with: unlit, one texture, no depth write.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct SkyMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub texture: Handle<Image>,
    #[uniform(2)]
    pub params: SkyParams,
}

impl Material for SkyMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    /// The sky is not in the depth prepass and casts no shadow. Leaving it in
    /// the prepass would write the box's depth and occlude the entire map.
    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    /// # Why depth *write* is off, and why that is enough
    ///
    /// `MATERIAL_VAR_IGNOREZ` (`sky_dx9.cpp:34`) is Source's way of saying the
    /// sky neither tests nor writes depth, which works there because the engine
    /// draws it in a known slot. Bevy's opaque phase is *binned*, not distance
    /// sorted, so there is no draw order to rely on. Turning off depth **write**
    /// while leaving the test alone makes the result order-independent instead:
    ///
    /// - Sky drawn first: passes the test against a cleared buffer, writes no
    ///   depth, and geometry drawn afterwards passes over it.
    /// - Geometry drawn first: the sky fails the test wherever geometry already
    ///   sits, so it cannot paint over the map.
    ///
    /// This is why the box has a real radius instead of sitting at unit distance
    /// — the depth test is doing real work, so the box has to be genuinely
    /// further away than the map. See [`sky_radius`].
    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

/// Marks the six quads so [`follow_camera`] can keep them centred on the view.
#[derive(Component)]
pub struct SkyBox;

/// How much bigger than the map the sky box is.
///
/// The box is centred on the camera, so the worst case is the camera at one
/// corner of the map with geometry at the opposite corner — exactly one diagonal
/// away. Anything below 1.0 would let distant geometry poke through the sky; the
/// margin covers flying somewhat outside the map, which the fly camera makes
/// easy.
pub const SKY_RADIUS_MARGIN: f32 = 1.5;

/// The camera's far plane, from `setup_scene`. The box's corners are
/// `radius * sqrt(3)`, and they have to stay inside it.
pub const CAMERA_FAR: f32 = 2000.0;

/// Sky box half-extent for a map of the given diagonal, in Bevy metres.
///
/// Clamped below so a tiny map still gets a box comfortably outside it, and
/// above so the box's corners stay inside [`CAMERA_FAR`].
pub fn sky_radius(map_diagonal: f32) -> f32 {
    (map_diagonal * SKY_RADIUS_MARGIN).clamp(200.0, 900.0)
}

/// Keep the sky box centred on the camera.
///
/// Translation only. Parenting the box to the camera would rotate the sky with
/// the view, which is exactly wrong — the sky is the one thing that must *not*
/// move when you turn.
pub fn follow_camera(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut sky: Query<&mut Transform, With<SkyBox>>,
) {
    let Ok(view) = camera.single() else {
        return;
    };
    let centre = view.translation();
    for mut transform in &mut sky {
        transform.translation = centre;
    }
}

/// The `SkyParams::brightness` that makes a 0..1 sky colour read correctly.
///
/// # Why this is 1.0 and not the lightmap's exposure factor
///
/// The first version returned [`lightmap_exposure`]`(1.0)` — about 1000 — on the
/// reasoning that a sky texel is an albedo and should land where a fully lit
/// white surface lands. That reasoning is right about the *goal* and wrong about
/// the path. Bevy applies `Exposure` inside `apply_pbr_lighting`, which a lit
/// surface goes through and this shader does not; the sky writes straight to the
/// HDR target. Multiplying by the inverse exposure as well left `pl_badwater`'s
/// sky a flat blown-out white.
///
/// The correct comparison is with an **unlit** surface, which also skips
/// `apply_pbr_lighting` and emits its `base_color` as-is. A lightmapped surface
/// arrives at `albedo x lightmap x OVERBRIGHT` in the same HDR buffer, so a sky
/// texel of 0.7 sitting next to a lit wall of albedo 0.35 at full light is
/// exactly the parity Source has. That means no scaling at all.
///
/// Kept as a function rather than inlined as `1.0` because it is a real
/// calibration decision with a real derivation, and because `SkyParams` carries
/// the multiplier anyway for a future exposure override.
pub fn sky_brightness() -> f32 {
    1.0
}

/// Registers the sky material and embeds its shader.
pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "sky_material.wgsl");
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default())
            // Before transform propagation, so the box is centred on the camera
            // in the same frame the camera moved. A frame of lag would show as
            // the sky sliding whenever you start or stop.
            .add_systems(
                PostUpdate,
                follow_camera.before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

/// One shared edge of the box, and how well the two faces meeting there agree.
#[derive(Clone, Copy, Debug)]
pub struct EdgeScore {
    pub a: SkyFace,
    pub b: SkyFace,
    /// Mean absolute per-channel difference along the edge, 0..255.
    pub mean_diff: f32,
    /// The same difference at the [`EDGE_PERCENTILE`]th percentile along the
    /// edge — the figure that tells a wrong *mapping* from a defect in the
    /// *artwork*. See [`EdgeScore::spread`].
    pub low_diff: f32,
}

impl EdgeScore {
    /// Whether the mismatch runs the length of the edge rather than sitting in
    /// one patch of it.
    ///
    /// # Why the mean alone is not a usable gate
    ///
    /// `sky_stranded_01`'s `lf`-`ft` join has a mean difference of 39.93, which
    /// looks like a badly wrong face. Sampling the two columns says otherwise:
    ///
    /// ```text
    /// ft right column:  35,57,74  35,57,74  52,78,95 ... 48,73,90    0,0,0  0,0,0 ...
    /// lf left column:   35,57,74  35,57,74  52,79,95 ... 48,73,90  62,92,110 ...
    /// ```
    ///
    /// The top half agrees to the byte and the bottom half of `ft` is **pure
    /// black** — the sky ships that way. Raising the limit until it passed would
    /// have blinded the gate to real errors of the same magnitude; excluding the
    /// sky by name would have to be redone for every such sky.
    ///
    /// The two causes are distinguishable in kind, not degree. A mirrored or
    /// rotated face is wrong at *every point* along the edge, because every
    /// sample reads from the wrong column. A defect in the artwork is wrong over
    /// part of it and exact over the rest. So the gate reads a low percentile:
    /// near zero means "most of this edge is perfect, some of it is not", and a
    /// high value means "none of it lines up".
    pub fn spread(&self) -> f32 {
        self.low_diff
    }
}

/// How many points to sample along each edge.
const EDGE_SAMPLES: usize = 64;

/// Which percentile of the along-edge difference the gate reads.
///
/// 20 rather than the median: a mapping error is wrong along the whole edge, so
/// even its best fifth is wrong, while `sky_stranded_01`'s blacked-out lower
/// half is exactly half the edge and would sit right on a median.
pub const EDGE_PERCENTILE: f32 = 20.0;

/// Fraction of each edge skipped at both ends.
///
/// The last texel at either end of an edge is a box *corner*, where a third face
/// also joins and the three faces' texels genuinely need not agree. Scoring it
/// would add a constant to every edge and bury the signal.
const EDGE_CORNER_SKIP: f32 = 0.02;

/// The twelve shared edges of the box, as face pairs.
///
/// Every unordered pair of faces that are not opposite: 6 × 4 / 2 = 12.
pub fn box_edges() -> Vec<(SkyFace, SkyFace)> {
    let mut edges = Vec::with_capacity(12);
    for (i, a) in SkyFace::ALL.iter().enumerate() {
        for b in &SkyFace::ALL[i + 1..] {
            // Opposite faces share no edge; their outward axes are antiparallel.
            if dot(a.axes().outward, b.axes().outward).abs() < 0.5 {
                edges.push((*a, *b));
            }
        }
    }
    edges
}

/// Score every shared edge of the box against the sky's own pixels.
///
/// # What this can and cannot decide
///
/// Two faces meeting at an edge are one continuous image, so sampling either
/// side of the join must agree. That makes this a sound test of a *specific*
/// arrangement — and a useless way to *search* for one, because TF2's skies are
/// smooth gradients whose score barely moves when a face is mirrored or two are
/// swapped. Used as a search it produced four different answers on four skies
/// with a +0.00 margin, which is why the sides here come from the axes instead.
///
/// `axes` is a parameter rather than a call to [`SkyFace::axes`] so a test can
/// perturb one face and check that the score notices.
pub fn edge_report(sky: &Sky, axes: impl Fn(SkyFace) -> FaceAxes) -> Vec<EdgeScore> {
    box_edges()
        .into_iter()
        .map(|(a, b)| edge_score(sky, a, axes(a), b, axes(b)))
        .collect()
}

/// Score one shared edge, both as a mean and at [`EDGE_PERCENTILE`].
pub fn edge_score(sky: &Sky, a: SkyFace, axes_a: FaceAxes, b: SkyFace, axes_b: FaceAxes) -> EdgeScore {
    let mut samples = edge_profile(sky, a, axes_a, b, axes_b);
    let mean = samples.iter().sum::<f32>() / samples.len().max(1) as f32;
    samples.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let at = ((samples.len() as f32 - 1.0) * EDGE_PERCENTILE / 100.0).round() as usize;
    EdgeScore {
        a,
        b,
        mean_diff: mean,
        low_diff: samples.get(at).copied().unwrap_or(0.0),
    }
}

/// Mean per-channel difference along the edge shared by two faces.
pub fn edge_difference(sky: &Sky, a: SkyFace, axes_a: FaceAxes, b: SkyFace, axes_b: FaceAxes) -> f32 {
    let samples = edge_profile(sky, a, axes_a, b, axes_b);
    samples.iter().sum::<f32>() / samples.len().max(1) as f32
}

/// Per-sample difference along a shared edge, in order.
///
/// Both faces are sampled at the *same 3D points* on the box rather than by
/// walking rows and columns, so no per-pair reasoning about which edge of which
/// image touches which is needed — get the axes right and this follows.
pub fn edge_profile(
    sky: &Sky,
    a: SkyFace,
    axes_a: FaceAxes,
    b: SkyFace,
    axes_b: FaceAxes,
) -> Vec<f32> {
    let along = cross(axes_a.outward, axes_b.outward);
    let base = add(axes_a.outward, axes_b.outward);
    let (tex_a, tex_b) = (sky.face(a), sky.face(b));

    (0..EDGE_SAMPLES)
        .map(|i| {
            let s = (i as f32 + 0.5) / EDGE_SAMPLES as f32;
            let t = (EDGE_CORNER_SKIP + s * (1.0 - 2.0 * EDGE_CORNER_SKIP)) * 2.0 - 1.0;
            let p = add(base, scale(along, t));

            let (ua, va) = axes_a.coords(p);
            let (ub, vb) = axes_b.coords(p);
            let pa = tex_a.sample_face(ua, va);
            let pb = tex_b.sample_face(ub, vb);
            (0..3)
                .map(|c| (pa[c] as f32 - pb[c] as f32).abs())
                .sum::<f32>()
                / 3.0
        })
        .collect()
}

/// Score one cap face in each of its four orientations against the four sides.
///
/// Returns the mean difference for each of [`cap_rotations`], so a caller sees
/// the margin rather than only the winner. A cap whose scores are all similar
/// carries no detail and must be excluded, not averaged in — which is the case
/// for most TF2 `up` faces and every `dn`.
pub fn cap_scores(sky: &Sky, cap: SkyFace) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for (i, axes) in cap_rotations(cap).iter().enumerate() {
        let mut total = 0.0;
        let mut count = 0;
        for (a, b) in box_edges() {
            let other = if a == cap {
                b
            } else if b == cap {
                a
            } else {
                continue;
            };
            total += edge_difference(sky, cap, *axes, other, other.axes());
            count += 1;
        }
        out[i] = total / count as f32;
    }
    out
}

/// Direction *toward* the sun, in Source axes, from a `light_environment`.
///
/// # This is not `AngleVectors`
///
/// `light_environment` goes through `SetupLightNormalFromProps`
/// (`public/map_utils.cpp:13`), which is **not** the usual angle-to-vector
/// conversion — its `z` is `+sin(pitch)` where `AngleVectors` uses
/// `-sin(pitch)`. Two overrides matter, and both are "only when non-zero", which
/// is Valve's own idiom here rather than a missing-key check:
///
/// - the `pitch` key overrides `angles[PITCH]` — present on all 265 instances
///   across the 231 maps that have a `light_environment`,
/// - the `angle` key overrides `angles[YAW]`.
///
/// That function returns the direction the light *travels*, so the direction to
/// the sun is its negation — which is what this returns, because that is what
/// the sky gets checked against.
pub fn sun_direction(angles: [f32; 3], pitch_key: f32, angle_key: f32) -> [f32; 3] {
    let yaw = if angle_key != 0.0 { angle_key } else { angles[1] };
    let pitch = if pitch_key != 0.0 {
        pitch_key
    } else {
        angles[0]
    };
    let (yaw, pitch) = (yaw.to_radians(), pitch.to_radians());
    let travel = [
        yaw.cos() * pitch.cos(),
        yaw.sin() * pitch.cos(),
        pitch.sin(),
    ];
    normalise(neg(travel))
}

/// How many compass bins [`azimuth_profile`] splits the sky into.
///
/// 72 bins is 5 degrees each — fine enough to locate a sun to well inside the
/// gate's tolerance, coarse enough that each bin holds thousands of texels and
/// the profile is smooth.
pub const AZIMUTH_BINS: usize = 72;

/// Mean sky brightness as a function of compass direction.
///
/// # Why not just find the brightest texels
///
/// The first version of this check took the luminance-weighted centroid of the
/// brightest 0.05% of texels. On a hazy sky that is not the sun — it is the
/// bright band along the horizon, which runs all the way round the compass, and
/// whose centroid points **straight down**. `sky_badlands_01` reported a sun at
/// elevation −76 degrees. A mean over spread-out samples on a sphere is
/// meaningless whenever the samples are not clustered, and there is no way to
/// know in advance whether they are.
///
/// Binning by compass direction cannot fail that way. A uniform horizon band
/// contributes equally to every bin and so cancels out of the *comparison*
/// between bins, leaving only the part that actually varies with direction —
/// which, on a real sky, brightens toward the sun. It also measures only what
/// this check is for: the **yaw**. Elevation is pinned separately, by `v` running
/// down (`sides_have_v_running_down`) and by the cap measurement, while yaw is
/// exactly what a whole-sky rotation breaks and what the side-continuity gate
/// provably cannot see.
#[derive(Clone, Debug)]
pub struct AzimuthProfile {
    /// Mean luminance per bin. Bin 0 is centred on Source `+X`, and bins advance
    /// toward `+Y`.
    pub bins: [f32; AZIMUTH_BINS],
}

impl AzimuthProfile {
    /// Compass direction of the brightest bin, in degrees, measured as Source
    /// measures yaw: 0 along `+X`, increasing toward `+Y`.
    pub fn peak_yaw(&self) -> f32 {
        let peak = self
            .bins
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0);
        (peak as f32 + 0.5) * 360.0 / AZIMUTH_BINS as f32
    }

    /// How far the brightest bin stands above the average, as a fraction.
    ///
    /// Near zero means the sky has no preferred direction — an overcast dome, or
    /// a night sky — and asking where its sun is has no answer. The caller uses
    /// this to exclude such skies openly rather than counting them as passes.
    pub fn prominence(&self) -> f32 {
        let mean: f32 = self.bins.iter().sum::<f32>() / AZIMUTH_BINS as f32;
        if mean <= 0.0 {
            return 0.0;
        }
        let peak = self.bins.iter().copied().fold(0.0, f32::max);
        (peak - mean) / mean
    }
}

/// Elevation window the profile samples, in degrees above the horizon.
///
/// # Why there has to be a window
///
/// A cube face's rows are not lines of constant elevation. At face coordinate
/// `v` the elevation is `atan(dv / sqrt(1 + du^2))`, so one row sits at 45
/// degrees in the middle of a face and 35 degrees at its corner. Scanning "the
/// whole upper half" therefore samples a **different elevation range at
/// different yaws**, and any feature that varies with elevation — every sky has
/// one, the horizon gradient — leaks into the compass profile as a spurious
/// modulation with a period of exactly 90 degrees. Measured on a synthetic sky
/// uniform in yaw by construction, that leak reported a prominence of 0.67,
/// which would have read as a strong sun.
///
/// Restricting to a fixed elevation band computed from the *direction* removes
/// it: every yaw bin then averages the same slice of sky. The upper limit stays
/// below 35 degrees because that is the lowest elevation a side face's top edge
/// reaches (at its corners), so the whole band comes from the four sides at
/// every yaw. The lower limit skips the horizon line itself, where haze and
/// terrain silhouettes dominate.
pub const PROFILE_ELEVATION: (f32, f32) = (5.0, 30.0);

/// Build the compass profile of a sky.
///
/// Each texel is weighted by the horizontal length of its direction, which falls
/// to zero at the zenith where a compass direction stops meaning anything. Only
/// the four sides contribute: [`PROFILE_ELEVATION`] lies entirely within them, so
/// including a cap would add samples at some yaws and not others.
pub fn azimuth_profile(sky: &Sky) -> AzimuthProfile {
    let mut sums = [0.0f32; AZIMUTH_BINS];
    let mut weights = [0.0f32; AZIMUTH_BINS];
    let (low, high) = PROFILE_ELEVATION;

    for face in SkyFace::ALL {
        if face.is_cap() {
            continue;
        }
        let tex = sky.face(face);
        let axes = face.axes();
        let fit = tex.vertical_fit();
        for y in 0..tex.height {
            for x in 0..tex.width {
                let u = (x as f32 + 0.5) / tex.width as f32;
                // Texture row to face coordinate: the inverse of the vertical
                // fit. Without this a half-height face's rows would be read as
                // spanning the whole face and land at the wrong elevations.
                let v = (y as f32 + 0.5) / tex.height as f32 / fit;
                let d = normalise(axes.point(u, v));
                let elevation = d[2].clamp(-1.0, 1.0).asin().to_degrees();
                if elevation < low || elevation > high {
                    continue;
                }
                let horizontal = (d[0] * d[0] + d[1] * d[1]).sqrt();
                if horizontal <= 1e-6 {
                    continue;
                }
                let bin = ((yaw_of(d) / 360.0 * AZIMUTH_BINS as f32) as usize)
                    .min(AZIMUTH_BINS - 1);
                sums[bin] += tex.luma(x, y) * horizontal;
                weights[bin] += horizontal;
            }
        }
    }

    let mut bins = [0.0f32; AZIMUTH_BINS];
    for i in 0..AZIMUTH_BINS {
        if weights[i] > 0.0 {
            bins[i] = sums[i] / weights[i];
        }
    }
    AzimuthProfile { bins }
}

/// Source yaw of a direction, in degrees: 0 along `+X`, increasing toward `+Y`.
pub fn yaw_of(direction: [f32; 3]) -> f32 {
    direction[1].atan2(direction[0]).to_degrees().rem_euclid(360.0)
}

/// Smallest angle between two compass directions, in degrees.
pub fn yaw_difference(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

// ---------------------------------------------------------------------------
// Small vector helpers, on Source-space arrays
// ---------------------------------------------------------------------------

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn neg(a: [f32; 3]) -> [f32; 3] {
    [-a[0], -a[1], -a[2]]
}

fn normalise(a: [f32; 3]) -> [f32; 3] {
    let len = dot(a, a).sqrt();
    if len <= 0.0 {
        return a;
    }
    scale(a, 1.0 / len)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant that says no face is mirrored.
    ///
    /// A sky face is a photograph, so a reflection anywhere would mean the axes
    /// are wrong rather than that the artwork is unusual. This is also what cuts
    /// each cap from eight candidate orientations to four.
    #[test]
    fn axes_are_not_mirrored() {
        for face in SkyFace::ALL {
            let axes = face.axes();
            let c = cross(axes.u, axes.v);
            for i in 0..3 {
                assert!(
                    (c[i] - axes.outward[i]).abs() < 1e-6,
                    "{face:?}: u x v = {c:?}, outward = {:?}",
                    axes.outward
                );
            }
        }
        for cap in [SkyFace::Up, SkyFace::Dn] {
            for (i, axes) in cap_rotations(cap).iter().enumerate() {
                let c = cross(axes.u, axes.v);
                for k in 0..3 {
                    assert!(
                        (c[k] - axes.outward[k]).abs() < 1e-6,
                        "{cap:?} rotation {i}: u x v = {c:?}",
                    );
                }
            }
        }
    }

    /// Axes are unit and mutually perpendicular, which `coords` inverting
    /// `point` depends on.
    #[test]
    fn axes_are_orthonormal() {
        for face in SkyFace::ALL {
            let a = face.axes();
            for v in [a.outward, a.u, a.v] {
                assert!((dot(v, v) - 1.0).abs() < 1e-6, "{face:?}: {v:?} not unit");
            }
            assert!(dot(a.outward, a.u).abs() < 1e-6);
            assert!(dot(a.outward, a.v).abs() < 1e-6);
            assert!(dot(a.u, a.v).abs() < 1e-6);
        }
    }

    #[test]
    fn coords_inverts_point() {
        for face in SkyFace::ALL {
            let a = face.axes();
            for (u, v) in [(0.0, 0.0), (1.0, 0.0), (0.25, 0.75), (1.0, 1.0)] {
                let (ru, rv) = a.coords(a.point(u, v));
                assert!((ru - u).abs() < 1e-6 && (rv - v).abs() < 1e-6, "{face:?}");
            }
        }
    }

    /// The four sides join without a gap, and in the order the axes imply.
    ///
    /// Read around the horizon in the direction `u` increases, the ring is
    /// `rt → ft → lf → bk → rt`: each face's `u = 1` edge is the same pair of box
    /// coordinates as the next face's `u = 0` edge. If any side's `u` were
    /// mirrored this would break, so it is the geometric half of what the
    /// pixel-level side-continuity gate checks.
    #[test]
    fn sides_join_without_a_gap() {
        let ring = [SkyFace::Rt, SkyFace::Ft, SkyFace::Lf, SkyFace::Bk];
        for (i, face) in ring.iter().enumerate() {
            let next = ring[(i + 1) % 4];
            let (a, b) = (face.axes(), next.axes());
            // Midpoints of the two edges that should coincide.
            let end = a.point(1.0, 0.5);
            let start = b.point(0.0, 0.5);
            for k in 0..3 {
                assert!(
                    (end[k] - start[k]).abs() < 1e-6,
                    "{face:?} u=1 edge {end:?} does not meet {next:?} u=0 edge {start:?}",
                );
            }
        }
    }

    #[test]
    fn there_are_twelve_edges_and_no_opposite_pairs() {
        let edges = box_edges();
        assert_eq!(edges.len(), 12);
        for (a, b) in edges {
            assert!(dot(a.axes().outward, b.axes().outward).abs() < 0.5);
            assert_ne!(a, b);
        }
    }

    /// `v` runs down on every side, so `v = 0` is the top row.
    ///
    /// Asserted separately from the axes table because an upside-down sky is the
    /// one orientation error that looks almost plausible on a gradient sky.
    #[test]
    fn sides_have_v_running_down() {
        for face in [SkyFace::Rt, SkyFace::Lf, SkyFace::Bk, SkyFace::Ft] {
            let a = face.axes();
            assert_eq!(a.v, [0.0, 0.0, -1.0], "{face:?}");
            assert!(a.point(0.5, 0.0)[2] > a.point(0.5, 1.0)[2], "{face:?}");
        }
    }

    fn flat(
        face: SkyFace,
        width: u32,
        height: u32,
        fill: impl Fn(f32, f32) -> [u8; 4],
    ) -> SkyFaceTexture {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let u = (x as f32 + 0.5) / width as f32;
                let v = (y as f32 + 0.5) / height as f32;
                rgba.extend_from_slice(&fill(u, v));
            }
        }
        SkyFaceTexture {
            face,
            width,
            height,
            rgba,
            source: None,
        }
    }

    /// A synthetic sky that is continuous across every edge by construction:
    /// each texel's colour is a function of its 3D direction alone.
    ///
    /// `layout` says how each face's image is laid out. Passing
    /// [`SkyFace::axes`] gives a sky the shipped table renders seamlessly;
    /// passing anything else gives a sky that is *correct for that layout*,
    /// which is what makes it possible to plant a known answer and check that a
    /// measurement recovers it.
    fn synthetic_sky_laid_out(layout: impl Fn(SkyFace) -> FaceAxes) -> Sky {
        let faces = SkyFace::ALL
            .iter()
            .map(|face| {
                let axes = layout(*face);
                // All square, so the vertical fit is the identity and this sky
                // is continuous across every edge. Mixed heights cannot be:
                // a half-height side has no lower half to be continuous with.
                // `the_vertical_fit_keeps_mixed_heights_consistent` covers that
                // case on its own terms.
                flat(*face, 64, 64, move |u, v| {
                    let d = normalise(axes.point(u, v));
                    [
                        ((d[0] * 0.5 + 0.5) * 255.0) as u8,
                        ((d[1] * 0.5 + 0.5) * 255.0) as u8,
                        ((d[2] * 0.5 + 0.5) * 255.0) as u8,
                        255,
                    ]
                })
            })
            .collect();
        Sky {
            name: "synthetic".into(),
            faces,
        }
    }

    fn synthetic_sky() -> Sky {
        synthetic_sky_laid_out(SkyFace::axes)
    }

    /// The edge check agrees with itself on a sky that is continuous by
    /// construction — so a non-zero score later means a real discontinuity and
    /// not an artifact of the sampling.
    #[test]
    fn edge_report_is_clean_on_a_continuous_sky() {
        let sky = synthetic_sky();
        for score in edge_report(&sky, SkyFace::axes) {
            assert!(
                score.mean_diff < 6.0,
                "{:?}-{:?} scored {} on a sky that is continuous by construction",
                score.a,
                score.b,
                score.mean_diff,
            );
        }
    }

    /// **The check must bite.** Mirroring exactly one face has to make its edges
    /// visibly worse — otherwise the gate is decoration, which is the trap the
    /// retired continuity *search* fell into.
    #[test]
    fn edge_report_catches_a_single_mirrored_face() {
        let sky = synthetic_sky();
        let clean = edge_report(&sky, SkyFace::axes);
        let worst_clean = clean
            .iter()
            .map(|s| s.spread().max(1.0))
            .fold(0.0, f32::max);

        for victim in SkyFace::ALL {
            let mirrored = move |face: SkyFace| {
                let a = face.axes();
                if face == victim {
                    // Mirror u: the exact error the cubemap attempt hit.
                    FaceAxes::new(a.outward, neg(a.u), a.v)
                } else {
                    a
                }
            };
            let report = edge_report(&sky, mirrored);
            // The *spread* figure, not the mean: that is what the gate reads, so
            // it is what has to move. A mirrored face is wrong along the whole
            // edge, so even its best fifth must be wrong.
            let worst = report
                .iter()
                .filter(|s| s.a == victim || s.b == victim)
                .map(|s| s.spread())
                .fold(0.0, f32::max);
            assert!(
                worst > worst_clean * 4.0,
                "mirroring {victim:?} only moved its worst edge spread to {worst} \
                 against a clean worst of {worst_clean}",
            );
        }
    }

    /// **A defect in one patch of the artwork must not read as a wrong face.**
    ///
    /// The other half of `EdgeScore::spread`'s job. Blacking out the lower half
    /// of one face reproduces `sky_stranded_01` exactly: the mean difference on
    /// its two joins jumps, and the gated spread figure must not, or every sky
    /// with a sloppy face would have to be excused by name.
    #[test]
    fn a_localised_artwork_defect_is_not_a_mapping_error() {
        let clean = synthetic_sky();
        let mut damaged = clean.clone();
        {
            let tex = &mut damaged.faces[SkyFace::Ft.index()];
            let (w, h) = (tex.width, tex.height);
            for y in h / 2..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    tex.rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
                }
            }
        }

        let before = edge_report(&clean, SkyFace::axes);
        let after = edge_report(&damaged, SkyFace::axes);
        // Side joins only, which is what the gate reads. `ft`'s join to `dn`
        // runs along its blacked-out bottom edge, so *every* sample there
        // mismatches — correctly, and it is not a localised defect from that
        // edge's point of view.
        let touching = |r: &Vec<EdgeScore>| -> (f32, f32) {
            r.iter()
                .filter(|e| !e.a.is_cap() && !e.b.is_cap())
                .filter(|e| e.a == SkyFace::Ft || e.b == SkyFace::Ft)
                .fold((0.0f32, 0.0f32), |(m, l), e| {
                    (m.max(e.mean_diff), l.max(e.spread()))
                })
        };
        let (mean_before, spread_before) = touching(&before);
        let (mean_after, spread_after) = touching(&after);

        // The mean notices, loudly...
        assert!(
            mean_after > mean_before + 20.0,
            "blacking out half a face barely moved the mean: {mean_before} -> {mean_after}",
        );
        // ...and the gated figure does not.
        assert!(
            spread_after < spread_before.max(1.0) * 4.0,
            "a localised defect moved the spread from {spread_before} to {spread_after}, \
             which would make it indistinguishable from a wrong face",
        );
    }

    /// **The cap measurement must recover an answer it was not told.**
    ///
    /// The obvious version of this test — build a sky with [`SkyFace::axes`] and
    /// assert [`cap_scores`] picks [`UP_ROTATION`] — cannot fail: the sky is laid
    /// out *using* `UP_ROTATION`, so it only ever checks that a constant equals
    /// itself. It would look like evidence for the measured rotation while
    /// providing none, which is the exact trap the retired continuity search fell
    /// into.
    ///
    /// So each of the four rotations is planted in turn and has to be recovered,
    /// with a margin. That tests the machinery — the edge selection, the corner
    /// skip, the sampling — and says nothing about which rotation TF2 actually
    /// uses. `skydump --caps` is what says that, on real pixels.
    #[test]
    fn cap_scores_recovers_a_planted_rotation() {
        for cap in [SkyFace::Up, SkyFace::Dn] {
            for planted in 0..4 {
                let axes = cap_rotations(cap)[planted];
                let sky = synthetic_sky_laid_out(move |face| {
                    if face == cap {
                        axes
                    } else {
                        face.axes()
                    }
                });
                let scores = cap_scores(&sky, cap);
                let best = scores
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                    .map(|(i, _)| i)
                    .unwrap();
                assert_eq!(
                    best, planted,
                    "{cap:?}: planted rot {planted}, measured rot {best}, scores {scores:?}",
                );
                let runner_up = scores
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != planted)
                    .map(|(_, s)| *s)
                    .fold(f32::MAX, f32::min);
                assert!(
                    runner_up > scores[planted] * 4.0,
                    "{cap:?} rot {planted}: margin too thin, scores {scores:?}",
                );
            }
        }
    }

    /// A sun straight overhead reads as straight overhead, and the `pitch` key
    /// wins over `angles`.
    #[test]
    fn sun_direction_matches_the_sdk_formula() {
        // pitch -90 => light travels to -Z => the sun is at +Z.
        let up = sun_direction([0.0, 0.0, 0.0], -90.0, 0.0);
        assert!((up[2] - 1.0).abs() < 1e-5, "{up:?}");

        // The `pitch` key overrides angles[PITCH].
        let overridden = sun_direction([45.0, 0.0, 0.0], -90.0, 0.0);
        assert!((overridden[2] - 1.0).abs() < 1e-5, "{overridden:?}");

        // With no pitch key, angles[PITCH] is used.
        let from_angles = sun_direction([-90.0, 0.0, 0.0], 0.0, 0.0);
        assert!((from_angles[2] - 1.0).abs() < 1e-5, "{from_angles:?}");

        // A typical TF2 sun: pitch -30, yaw 250. Above the horizon, and on the
        // side the yaw puts it.
        let sun = sun_direction([0.0, 250.0, 0.0], -30.0, 0.0);
        assert!(sun[2] > 0.0, "sun below the horizon: {sun:?}");
        let expect_x = -(250f32.to_radians().cos() * (-30f32).to_radians().cos());
        assert!((sun[0] - expect_x).abs() < 1e-5, "{sun:?} vs x {expect_x}");
    }

    /// **The compass profile must find a sun planted at a known yaw.**
    ///
    /// Planted on each of the four sides in turn, so the test exercises all four
    /// side orientations rather than whichever one happens to be first. The
    /// patch is placed above the horizon, because that is the only half the
    /// profile looks at.
    #[test]
    fn azimuth_profile_finds_a_planted_sun() {
        for planted in [SkyFace::Rt, SkyFace::Lf, SkyFace::Bk, SkyFace::Ft] {
            // Planted by *direction*, inside the elevation band the profile
            // reads, so the test does not depend on how face rows map onto
            // elevations.
            let target = {
                let mid = (PROFILE_ELEVATION.0 + PROFILE_ELEVATION.1) * 0.5;
                let yaw = yaw_of(planted.axes().outward).to_radians();
                let p = mid.to_radians();
                [yaw.cos() * p.cos(), yaw.sin() * p.cos(), p.sin()]
            };
            let faces = SkyFace::ALL
                .iter()
                .map(|face| {
                    let axes = face.axes();
                    flat(*face, 64, 64, move |u, v| {
                        let d = normalise(axes.point(u, v));
                        if dot(d, target) > 0.985 {
                            [255, 255, 255, 255]
                        } else {
                            [20, 20, 20, 255]
                        }
                    })
                })
                .collect();
            let sky = Sky {
                name: "planted".into(),
                faces,
            };
            let profile = azimuth_profile(&sky);
            let expected = yaw_of(target);
            let found = profile.peak_yaw();
            assert!(
                yaw_difference(expected, found) < 10.0,
                "planted on {planted:?} at yaw {expected:.1}, profile peaked at {found:.1}",
            );
            assert!(
                profile.prominence() > 0.5,
                "planted on {planted:?}: prominence only {}",
                profile.prominence(),
            );
        }
    }

    /// **The prominence must reject a sky with no sun**, or the gate would
    /// silently pass every overcast sky by reading noise as a direction.
    #[test]
    fn a_uniform_sky_has_no_findable_sun() {
        let faces = SkyFace::ALL
            .iter()
            .map(|face| flat(*face, 64, 64, |_, _| [120, 120, 120, 255]))
            .collect();
        let sky = Sky {
            name: "uniform".into(),
            faces,
        };
        let profile = azimuth_profile(&sky);
        assert!(
            profile.prominence() < 0.05,
            "a flat grey sky reported prominence {}",
            profile.prominence(),
        );
    }

    /// A horizon band that is uniform in yaw must not read as a sun.
    ///
    /// This is the exact failure the retired brightest-texel version had: the
    /// band is the brightest thing in the sky, so a centroid over "the brightest
    /// texels" landed on it and pointed at the nadir. A compass profile is
    /// indifferent to it, because it contributes equally to every bin.
    #[test]
    fn a_uniform_horizon_band_is_not_mistaken_for_a_sun() {
        let faces = SkyFace::ALL
            .iter()
            .map(|face| {
                let axes = face.axes();
                flat(*face, 64, 64, move |_u, v| {
                    // Defined by *elevation*, which is what "a horizon band"
                    // physically means. Uniform in yaw by construction, so any
                    // prominence the profile reports is its own artifact.
                    let d = normalise(axes.point(0.5, v));
                    let elevation = d[2].clamp(-1.0, 1.0).asin().to_degrees();
                    if (0.0..15.0).contains(&elevation) {
                        [255, 255, 255, 255]
                    } else {
                        [30, 30, 30, 255]
                    }
                })
            })
            .collect();
        let sky = Sky {
            name: "banded".into(),
            faces,
        };
        let profile = azimuth_profile(&sky);
        assert!(
            profile.prominence() < 0.2,
            "a yaw-uniform horizon band reported prominence {}",
            profile.prominence(),
        );
    }

    /// A half-height face covers the top half of the cube face and repeats its
    /// last row below — the rule `SkyFaceTexture::vertical_fit` documents.
    #[test]
    fn a_half_height_face_repeats_its_last_row() {
        // Rows ramp 0..63 in red; the last row is a distinctive green.
        let mut tex = flat(SkyFace::Rt, 64, 32, |_, v| {
            [(v * 255.0) as u8, 0, 0, 255]
        });
        for x in 0..64 {
            let i = ((31 * 64 + x) * 4) as usize;
            tex.rgba[i..i + 4].copy_from_slice(&[0, 255, 0, 255]);
        }
        assert_eq!(tex.vertical_fit(), 2.0);

        // Face v = 0 and v = 0.5 bracket the real content...
        assert_eq!(tex.sample_face(0.5, 0.0)[1], 0, "top of the face is content");
        // ...and everything below v = 0.5 is the repeated last row.
        for v in [0.51, 0.75, 1.0] {
            assert_eq!(
                tex.sample_face(0.5, v),
                [0, 255, 0, 255],
                "face v = {v} should repeat the last row",
            );
        }
    }

    /// **The fit rule must make a mixed-height sky continuous**, because that is
    /// the whole reason for preferring it over stretching.
    ///
    /// One side is built half-height, holding only what the top half of the face
    /// covers; the rest are square. If the fit were a stretch, that face's
    /// content would land at the wrong elevations and its two joins would break —
    /// which is exactly what `sky_alpinestorm_01` showed at 15.02 before the rule
    /// went in.
    #[test]
    fn the_vertical_fit_keeps_mixed_heights_consistent() {
        let short = SkyFace::Bk;
        let faces = SkyFace::ALL
            .iter()
            .map(|face| {
                let axes = face.axes();
                let (w, h) = if *face == short { (64, 32) } else { (64, 64) };
                let fit = (w as f32 / h as f32).max(1.0);
                flat(*face, w, h, move |u, v| {
                    // Texture row to face coordinate, then colour by direction.
                    let d = normalise(axes.point(u, v / fit));
                    [
                        ((d[0] * 0.5 + 0.5) * 255.0) as u8,
                        ((d[1] * 0.5 + 0.5) * 255.0) as u8,
                        ((d[2] * 0.5 + 0.5) * 255.0) as u8,
                        255,
                    ]
                })
            })
            .collect();
        let sky = Sky {
            name: "mixed".into(),
            faces,
        };
        // Only the joins in the half the short face actually covers can agree;
        // below that it has no data by construction, exactly as a real
        // half-height sky face does not.
        for (a, b) in box_edges() {
            if a == SkyFace::Dn || b == SkyFace::Dn {
                continue;
            }
            let diff = edge_difference(&sky, a, a.axes(), b, b.axes());
            assert!(
                diff < 40.0,
                "{a:?}-{b:?} scored {diff} on a mixed-height sky built to the fit rule",
            );
        }
        // And the joins that involve the short face are no worse than the ones
        // that do not — the actual claim.
        let with_short = box_edges()
            .into_iter()
            .filter(|(a, b)| (*a == short || *b == short) && *a != SkyFace::Dn && *b != SkyFace::Dn)
            .map(|(a, b)| edge_difference(&sky, a, a.axes(), b, b.axes()))
            .fold(0.0, f32::max);
        assert!(
            with_short < 40.0,
            "the half-height face's worst join is {with_short}",
        );
    }

    #[test]
    fn colour_param_accepts_both_spellings() {
        for text in [
            "\"sky\" { $color \"1 0 0\" }",
            "\"sky\" { $color \"[1 0 0]\" }",
        ] {
            let m = vmt::Material::parse("materials/skybox/x.vmt", text).unwrap();
            assert_eq!(colour_param(&m), [255, 0, 0, 255], "{text}");
        }
        // No `$color` at all: black, as `sky_hacksaw_01dn` effectively is.
        let m = vmt::Material::parse("materials/skybox/x.vmt", "\"sky\" { $nofog 1 }").unwrap();
        assert_eq!(colour_param(&m), [0, 0, 0, 255]);
    }

    /// The quads overlap rather than abut, and the overlap is small.
    #[test]
    fn quads_overlap_without_distorting_the_box() {
        let mesh = face_mesh(SkyFace::Rt, 100.0, 1.0);
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
            .expect("positions")
            .to_vec();
        for p in positions {
            let v = Vec3::from(p);
            // Bevy X is Source X, so the face's own axis is untouched by the
            // in-plane growth.
            assert!((v.x - 100.0).abs() < 1e-3, "{v:?}");
            let across = v.y.abs().max(v.z.abs());
            assert!(across > 100.0, "quad did not grow: {across}");
            assert!(across < 101.0, "quad grew too far: {across}");
        }
    }

    #[test]
    fn sky_radius_stays_inside_the_far_plane() {
        for diagonal in [1.0, 100.0, 419.0, 10_000.0] {
            let r = sky_radius(diagonal);
            assert!(
                r * 3f32.sqrt() < CAMERA_FAR,
                "diagonal {diagonal} -> radius {r}, corners past the far plane",
            );
        }
        // And outside any map it is meant to contain.
        assert!(sky_radius(419.0) > 419.0);
    }
}
