//! VTF → `bevy::Image`.
//!
//! Source ships nearly everything DXT-compressed, and DXT1/3/5 are bit-identical
//! to BC1/2/3, so the compressed blocks go to the GPU untouched wherever the
//! adapter supports it. That is not just tidiness: ctf_2fort's ~230 textures
//! decoded to RGBA8 would be most of a gigabyte of VRAM, against roughly an
//! eighth of that as BC. Both reference tools decode instead, and `vbsp-to-gltf`
//! then re-encodes to PNG, which is how 22 MB of BSP becomes a 172 MB glb.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use vtf::image::ImageFormat;
use vtf::vtf::VTF;

pub fn to_image(data: &[u8], supports_bc: bool) -> Result<Image, String> {
    let vtf = VTF::read(data).map_err(|e| e.to_string())?;
    let source = &vtf.highres_image;
    let size = Extent3d {
        width: source.width as u32,
        height: source.height as u32,
        depth_or_array_layers: 1,
    };

    // Dxt1Onebitalpha is DXT1 with the punch-through alpha bit; same blocks.
    let block_format = match source.format {
        ImageFormat::Dxt1 | ImageFormat::Dxt1Onebitalpha => Some(TextureFormat::Bc1RgbaUnormSrgb),
        ImageFormat::Dxt3 => Some(TextureFormat::Bc2RgbaUnormSrgb),
        ImageFormat::Dxt5 => Some(TextureFormat::Bc3RgbaUnormSrgb),
        _ => None,
    };

    let mut image = match block_format.filter(|_| supports_bc) {
        Some(format) => {
            // Frame 0, mip 0. Note `decode`/`get_frame` take a *frame*, not a mip
            // level, so animated textures silently collapse to their first frame.
            let blocks = source.get_frame(0).map_err(|e| e.to_string())?;
            Image::new(
                size,
                TextureDimension::D2,
                blocks.to_vec(),
                format,
                RenderAssetUsages::RENDER_WORLD,
            )
        }
        None => {
            let decoded = source.decode(0).map_err(|e| e.to_string())?;
            Image::new(
                size,
                TextureDimension::D2,
                decoded.to_rgba8().into_raw(),
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::RENDER_WORLD,
            )
        }
    };

    // Not optional. Source computes UVs as a dot product against world position
    // over texture size, so a long wall runs to u = 40 and beyond. With Bevy's
    // default ClampToEdge every surface becomes one smeared edge pixel.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        // Anisotropy needs a mip chain to do its job, and we upload only mip 0
        // so far -- harmless now, and correct once mips land.
        anisotropy_clamp: 16,
        ..ImageSamplerDescriptor::linear()
    });

    Ok(image)
}
