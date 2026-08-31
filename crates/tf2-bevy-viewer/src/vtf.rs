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

/// Concatenate frame 0's mip levels into the layout wgpu expects, largest first.
///
/// Returns the bytes and the level count, which is not always the header's
/// `mipmap_count`: wgpu rejects a chain that outlives its dimensions, and BC
/// blocks are 4x4, so levels below 4x4 are dropped rather than uploaded as
/// padded blocks. Stopping early is legal -- a partial chain just means the
/// sampler runs out of levels sooner.
fn mip_chain(source: &vtf::image::VTFImage) -> Result<(Vec<u8>, u32), String> {
    let mut data = Vec::new();
    let mut levels = 0;

    for mip in 0..source.mip_count() {
        let (w, h) = source.mip_size(mip);
        if mip > 0 && (w < 4 || h < 4) {
            break;
        }
        match source.get_mip(0, mip) {
            Ok(bytes) => {
                data.extend_from_slice(bytes);
                levels += 1;
            }
            // A truncated chain is common enough in shipped content that it is
            // not worth failing the whole texture over. Keep what we have.
            Err(_) => break,
        }
    }

    if levels == 0 {
        return Err("no readable mip levels".into());
    }
    Ok((data, levels))
}

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
            // Frame 0, every mip. Note `decode`/`get_frame` take a *frame*, not a
            // mip level, so animated textures still collapse to their first frame.
            let (data, mips) = mip_chain(source)?;
            let mut image = Image::new(
                size,
                TextureDimension::D2,
                data,
                format,
                RenderAssetUsages::RENDER_WORLD,
            );
            image.texture_descriptor.mip_level_count = mips;
            image
        }
        None => {
            // The uncompressed path decodes through `DynamicImage`, which only
            // ever gives us mip 0. One texture on ctf_2fort lands here, so it is
            // not worth a second decode loop -- it just renders unmipped.
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
        // Now that the mip chain is uploaded this does something: anisotropy
        // needs mips, and `linear()` alone leaves `mipmap_filter` at Nearest,
        // which pops visibly as levels change. Both are required.
        anisotropy_clamp: 16,
        mipmap_filter: bevy::image::ImageFilterMode::Linear,
        ..ImageSamplerDescriptor::linear()
    });

    Ok(image)
}
