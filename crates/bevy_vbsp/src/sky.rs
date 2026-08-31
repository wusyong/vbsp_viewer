//! The 2D sky: six VMTs from `worldspawn`'s `skyname`, assembled into a cubemap.
//!
//! Two things this cannot assume, both established by probing ctf_2fort rather
//! than from convention:
//!
//! **The faces are not `<skyname><suffix>.vtf`.** On `sky_tf2_04` there is no
//! such file -- every face is a VMT, and all four sides resolve to one shared
//! `skybox/sky_tf2_04side` texture. Going through the VMT is the only thing that
//! works, and it is what Source does anyway.
//!
//! **The faces are not the same size.** Sides are 512x256, `up` is 512x512, and
//! `dn` is a single pixel, because nobody ever sees the ground of a skybox. wgpu
//! requires all six layers to match, so every face is resampled to one square
//! size -- which also means decoding to RGBA8 rather than passing BC blocks
//! through the way `vtf::to_image` does. Six small textures, so the cost is
//! nil, and it is the resampling that forces it: you cannot rescale a
//! compressed block.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::math::{Affine2, Vec2};
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use image::RgbaImage;

use crate::vfs::Vfs;
use crate::vmt;

/// Source's face suffixes in the order wgpu wants its cube layers: +X, -X, +Y,
/// -Y, +Z, -Z.
///
/// The mapping falls out of the world conversion. Source's sky faces are
/// `ft`=+X, `bk`=-X, `lf`=+Y, `rt`=-Y, `up`=+Z, `dn`=-Z; `geometry::to_bevy`
/// permutes `(x, y, z) -> (y, z, x)`, so Source +X becomes Bevy +Z, +Y becomes
/// +X and +Z becomes +Y. Hence: +X is `lf`, -X is `rt`, +Y is `up`, -Y is `dn`,
/// +Z is `ft`, -Z is `bk`.
///
/// Caveat worth keeping: Source also rotates some faces within their own plane,
/// and **this is unverified**. ctf_2fort's sky is a vertical gradient with four
/// identical sides, so no rotation is observable on it -- a wrong per-face
/// rotation would look exactly like a right one. Check it against a map with a
/// sun or a distinctive cloud before trusting it.
const FACES: [&str; 6] = ["lf", "rt", "up", "dn", "ft", "bk"];

pub struct SkyReport {
    pub load_time: std::time::Duration,
    pub name: String,
    /// Edge length of one cube face.
    pub size: u32,
    /// Distinct textures behind the six faces. Usually 3, not 6.
    pub textures: usize,
    pub bytes: usize,
}

/// `worldspawn`'s `skyname`. Absent on a map with no sky.
pub fn name(bsp: &vbsp::Bsp) -> Option<String> {
    bsp.entities
        .iter()
        .find(|e| e.prop("classname") == Some("worldspawn"))
        .and_then(|e| e.prop("skyname"))
        .map(str::to_string)
}

pub fn load(vfs: &Vfs, skyname: &str) -> Result<(Image, SkyReport), String> {
    let mut faces = Vec::with_capacity(6);
    let mut seen: Vec<String> = Vec::new();

    for suffix in FACES {
        let material = format!("skybox/{skyname}{suffix}");
        let def = vmt::load(vfs, &material)?;
        let base = def
            .base_texture
            .ok_or_else(|| format!("{material}: no $basetexture"))?;
        if !seen.contains(&base) {
            seen.push(base.clone());
        }

        let path = format!("materials/{}.vtf", base.trim_start_matches('/'));
        let bytes = vfs.load(&path).ok_or_else(|| format!("no {path}"))?;
        let texture = vtf::vtf::VTF::read(&bytes).map_err(|e| format!("{path}: {e}"))?;
        let decoded = texture
            .highres_image
            .decode(0)
            .map_err(|e| format!("{path}: {e}"))?
            .to_rgba8();

        // `$basetexturetransform` is not decoration here. The sides are 2:1 with
        // `scale 1 2`, so the texture covers the upper half of the face and the
        // rest clamps to its bottom row -- gradient above, flat haze below,
        // which is what the horizon should look like.
        let transform = def.transform.as_ref().map_or(Affine2::IDENTITY, |t| {
            let center = Vec2::from(t.center);
            Affine2::from_translation(center)
                * Affine2::from_scale_angle_translation(
                    Vec2::from(t.scale),
                    t.rotate.to_radians(),
                    Vec2::from(t.translate),
                )
                * Affine2::from_translation(-center)
        });

        faces.push((decoded, transform));
    }

    // One square size for all six. Take the largest edge present rather than a
    // fixed number, so a high-res sky is not thrown away and `dn`'s single pixel
    // does not drag everything down to 1x1.
    let size = faces
        .iter()
        .flat_map(|(img, _)| [img.width(), img.height()])
        .max()
        .unwrap_or(1)
        .max(1);

    let mut data = Vec::with_capacity((size * size * 4 * 6) as usize);
    for (img, transform) in &faces {
        resample_into(&mut data, img, *transform, size);
    }

    let bytes = data.len();
    let mut image = Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        data,
        // The sky art is authored in sRGB like every other texture here.
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );

    // Six layers alone give an array texture; the cube dimension is what makes
    // it samplable by direction, and `Skybox` will not accept anything else.
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });

    // ClampToEdge, deliberately, and the opposite of what `vtf.rs` wants: world
    // textures tile, but a sky face wrapping would drag the far edge of the
    // texture across the seam between faces.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        ..ImageSamplerDescriptor::linear()
    });

    Ok((
        image,
        SkyReport {
            load_time: Default::default(),
            name: skyname.to_string(),
            size,
            textures: seen.len(),
            bytes,
        },
    ))
}

/// Stretch one face into `size`x`size`, applying its texture transform.
///
/// Nearest-neighbour with a clamped source lookup. Bilinear would be better for
/// a detailed sky, but every face here is either a smooth gradient or a flat
/// colour, and the clamp is the part that matters: it is what turns the sides'
/// `scale 1 2` into "gradient on top, horizon colour below" instead of a second
/// copy of the gradient tiled underneath.
fn resample_into(out: &mut Vec<u8>, src: &RgbaImage, transform: Affine2, size: u32) {
    let (sw, sh) = (src.width(), src.height());
    for y in 0..size {
        for x in 0..size {
            let uv = Vec2::new(
                (x as f32 + 0.5) / size as f32,
                (y as f32 + 0.5) / size as f32,
            );
            let uv = transform.transform_point2(uv);
            let sx = ((uv.x * sw as f32) as i32).clamp(0, sw as i32 - 1) as u32;
            let sy = ((uv.y * sh as f32) as i32).clamp(0, sh as i32 - 1) as u32;
            out.extend_from_slice(&src.get_pixel(sx, sy).0);
        }
    }
}

use bevy::prelude::default;
