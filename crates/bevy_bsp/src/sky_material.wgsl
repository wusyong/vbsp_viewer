// Source's `Sky` shader: one 2D texture, unlit, no fog.
//
// This is deliberately as thin as the original. `sky_dx9.cpp` binds one sampler,
// declares `VertexShaderVertexFormat( VERTEX_POSITION, 1, NULL, 0 )` — position
// plus a single 2D texcoord set — and its pixel shader is one `tex2D` times a
// colour constant:
//
//     HALF4 color = tex2D( BaseTextureSampler, i.baseTexCoord.xy );
//     color.rgb *= InputScale.rgb;
//     return FinalOutput( color, 0, PIXEL_FOG_TYPE_NONE, ... );
//
// There is no cubemap and no direction vector anywhere in it.

#import bevy_pbr::forward_io::VertexOutput

struct SkyParams {
    // Takes a 0..1 sky colour into Bevy's photometric range; see
    // `sky::sky_brightness`.
    brightness: f32,
    _padding: vec3<f32>,
}

@group(3) @binding(0) var sky_texture: texture_2d<f32>;
@group(3) @binding(1) var sky_sampler: sampler;
@group(3) @binding(2) var<uniform> sky_params: SkyParams;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // `textureSampleLevel` at 0 rather than `textureSample`: the sky VTFs are
    // flagged NOMIP | NOLOD, so the engine never leaves the base level. The
    // uploaded texture has only one level anyway, but saying so explicitly is
    // what makes an implicit LOD from screen-space derivatives — the thing that
    // draws a coarse-mip line along a face join — impossible rather than merely
    // unreachable.
    let colour = textureSampleLevel(sky_texture, sky_sampler, in.uv, 0.0);
    return vec4(colour.rgb * sky_params.brightness, 1.0);
}
