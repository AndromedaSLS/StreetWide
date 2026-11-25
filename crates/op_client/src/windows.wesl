// Lit windows (ADR-0022, W1): the StandardMaterial's own shading, plus the
// glow of the windows lit at this hour, added after lighting in cd/m² times
// the exposure. Which window is lit follows only from the wall's seed (the
// second texture coordinates), the window's bay and storey (the first,
// floored) and the night: never from data about real homes. This mirrors
// `windows.rs` (`window_hash`, `window_light`), whose tests check the
// constants. Bindless like the StandardMaterial, where the device allows,
// so windowed walls batch with the rest.

import bevy_pbr::render::{
    mesh_bindings::mesh,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
};
import bevy_render::bindless::{bindless_samplers_filtering, bindless_textures_2d};

@if(PREPASS_PIPELINE)
import bevy_pbr::{
    prepass::io::{VertexOutput, FragmentOutput},
    deferred::functions::deferred_output,
};
@else
import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_view_bindings::view,
};

struct WindowGlow {
    share: f32,
    night: u32,
    odds: vec2<f32>,
    warm: vec4<f32>,
    neutral: vec4<f32>,
    cool: vec4<f32>,
    mean: vec4<f32>,
}

// Where a material's data and glass mask are in the bindless arrays.
struct WindowGlowIndices {
    glow: u32,
    glass_texture: u32,
    glass_sampler: u32,
}

@if(BINDLESS) {
@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage> glow_indices: array<WindowGlowIndices>;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<storage> glow_array: array<WindowGlow>;
} @else {
@group(constants::MATERIAL_BIND_GROUP) @binding(50) var<uniform> glow_uniform: WindowGlow;
@group(constants::MATERIAL_BIND_GROUP) @binding(51) var glass_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(52) var glass_sampler: sampler;
}

// PCG hash (Jarzynski and Olano, "Hash Functions for GPU Rendering", JCGT
// 9(3), 2020).
fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// Uniform in [0, 1) from a hash.
fn unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

fn window_hash(seed: vec2<u32>, cell: vec2<i32>, night: u32) -> u32 {
    var h = pcg(night);
    h = pcg(h ^ bitcast<u32>(cell.y));
    h = pcg(h ^ bitcast<u32>(cell.x));
    h = pcg(h ^ seed.y);
    return pcg(h ^ seed.x);
}

// The light of the window at `uv` (bays across, storeys down) on the wall
// whose seed is `seed_uv`, in cd/m². `slot` is the material's bindless slot.
fn window_light(uv: vec2<f32>, seed_uv: vec2<f32>, slot: u32) -> vec3<f32> {
    // Window cells per pixel: where windows are too small on screen to tell
    // apart, each glows with the average instead of shimmering.
    let footprint = max(fwidth(uv.x), fwidth(uv.y));
    var glow: WindowGlow;
    var glass = 0.0;
    @if(BINDLESS) {
        let at = glow_indices[slot];
        glow = glow_array[at.glow];
        glass = textureSample(
            bindless_textures_2d[at.glass_texture],
            bindless_samplers_filtering[at.glass_sampler],
            uv
        ).r;
    } @else {
        glow = glow_uniform;
        glass = textureSample(glass_texture, glass_sampler, uv).r;
    }
    if glow.share <= 0.0 {
        return vec3(0.0);
    }
    let h = window_hash(vec2<u32>(round(seed_uv)), vec2<i32>(floor(uv)), glow.night);
    let pick = unit(pcg(h ^ 0x68e31da4u));
    var colour = glow.cool.rgb;
    if pick < glow.odds.y {
        colour = glow.neutral.rgb;
    }
    if pick < glow.odds.x {
        colour = glow.warm.rgb;
    }
    let bright = 0.5 + unit(pcg(h ^ 0xb5297a4du));
    let lit = select(0.0, 1.0, unit(h) < glow.share);
    let near = colour * (bright * lit);
    let far = glow.mean.rgb * glow.share;
    return glass * mix(near, far, smoothstep(0.3, 0.7, footprint));
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color
    );

    var out: FragmentOutput;
@if(PREPASS_PIPELINE) {
    out = deferred_output(in, pbr_input);
} @else {
    out.color = apply_pbr_lighting(pbr_input);
    var light = vec3(0.0);
    @if(VERTEX_UVS_A && VERTEX_UVS_B) {
        let slot = mesh[in.instance_index].material_and_lightmap_bind_group_slot & 0xffffu;
        light = window_light(in.uv, in.uv_b, slot);
    }
    out.color = vec4(out.color.rgb + light * view.exposure, out.color.a);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
}
    return out;
}
