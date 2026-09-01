// Source's world shaders, as an extension to Bevy's StandardMaterial.
//
// Everything a Source `LightmappedGeneric` needs — albedo x lightmap x
// overbright — already falls out of the standard PBR path: `$basetexture`
// becomes `base_color_texture`, and the baked lightmap arrives through Bevy's
// own `Lightmap` component, which `pbr_input_from_standard_material` folds in
// as indirect diffuse.
//
// The one thing that does *not* fit is `WorldVertexTransition`: two albedo
// textures blended per-vertex, which is how every dirt-to-grass edge on a
// displacement is authored (2 073 of TF2's 47 694 material references). That is
// the only reason this file exists.
//
// The blend weight rides the vertex colour's **alpha**, with RGB left at 1 so
// the standard path's `base_color *= in.color` stays neutral. `in.color.a` is
// read here rather than `pbr_input.material.base_color.a`, because by the time
// the standard path returns, that has already been multiplied by the material
// and texture alpha.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}

struct SourceParams {
    // 1 when `$basetexture2` should be blended in by vertex alpha.
    blend: u32,
    _padding: vec3<u32>,
}

// Slots 0-99 belong to StandardMaterial; extensions start at 100.
@group(3) @binding(100) var base_texture2: texture_2d<f32>;
@group(3) @binding(101) var base_texture2_sampler: sampler;
@group(3) @binding(102) var<uniform> source_params: SourceParams;

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    if source_params.blend != 0u {
#ifdef VERTEX_UVS
    #ifdef VERTEX_COLORS
        let weight = clamp(in.color.a, 0.0, 1.0);
        let second = textureSample(base_texture2, base_texture2_sampler, in.uv);
        pbr_input.material.base_color = mix(
            pbr_input.material.base_color,
            second,
            weight,
        );
        // Terrain blends are opaque; the weight lives in vertex alpha, so
        // leaving it multiplied into base_color would make the surface fade.
        pbr_input.material.base_color.a = 1.0;
    #endif
#endif
    }

    pbr_input.material.base_color =
        alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
