//! The F1 overlay: frame timing, and what the map actually loaded.
use crate::camera::FlyCamera;
use bevy::pbr::wireframe::WireframeConfig;
use crate::cli::Args;
use crate::debug::AlbedoMode;
use bevy::prelude::*;
use bevy_bsp::{MapInfo, SurfaceInfo};

/// Smoothed frame timing.
///
/// Under the default `PresentMode::Fifo` this reports the display refresh rate
/// rather than the cost of a frame, so `frame_ms` is the number worth reading;
/// run with `--no-vsync` to make `fps` meaningful.
#[derive(Resource, Default)]
pub struct FrameRate {
    pub fps: f32,
    pub frame_ms: f32,
    pub samples: u32,
}

#[derive(Component)]
pub struct HudText;

/// Frames to ignore before measuring. Window creation and shader pipeline
/// compilation make the first frames wildly unrepresentative, and an average
/// seeded from them reads high for a long time — high enough to report a frame
/// rate above the vsync cap, which is how this was caught.
pub const FRAME_RATE_WARMUP: u32 = 10;

pub fn tick_frame_rate(mut rate: ResMut<FrameRate>, time: Res<Time>) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    rate.samples += 1;
    if rate.samples <= FRAME_RATE_WARMUP {
        return;
    }

    // Adaptive weight: average the early samples so the value converges
    // immediately, then settle into a smooth ~1 second window.
    let n = (rate.samples - FRAME_RATE_WARMUP).min(60) as f32;
    let alpha = 1.0 / n;
    rate.frame_ms += (dt * 1000.0 - rate.frame_ms) * alpha;
    rate.fps += (1.0 / dt - rate.fps) * alpha;
}

// Bevy systems take their dependencies as parameters, so the count here is
// the injection list, not a signature that wants shortening.
#[allow(clippy::too_many_arguments)]
pub fn update_hud(
    mut hud: Query<&mut Text, With<HudText>>,
    camera: Query<&Transform, With<FlyCamera>>,
    surfaces: Query<&SurfaceInfo>,
    info: Res<MapInfo>,
    rate: Res<FrameRate>,
    wireframe: Res<WireframeConfig>,
    mode: Res<AlbedoMode>,
    args: Res<Args>,
) {
    let vsync = !args.no_vsync;
    let Ok(mut text) = hud.single_mut() else {
        return;
    };
    let Ok(transform) = camera.single() else {
        return;
    };

    // Report the camera in Source units so it can be compared directly with
    // `cl_showpos` in the real game.
    let [x, y, z] = bevy_bsp::bevy_to_src(transform.translation);
    let draw_calls = surfaces.iter().count();

    *text = Text::new(format!(
        "{} (bsp v{})\n\
         {} materials  {} verts  {} tris  {} draw calls\n\
         terrain {} disps  {} verts  {} tris\n\
         brush entities {}  {} tris\n\
         faces {}/{}  ({} on displacements)\n\
         lightmap {}\n\
         materials {}\n\
         pos {x:.0} {y:.0} {z:.0} (Source units)\n\
         {:.1} ms/frame  ({:.0} fps{}){}\n\
         \n\
         WASD/QE move, Shift sprint, click to look, Esc release\n\
         F1 overlay  F2 wireframe  F3 brushes  F4 terrain  F5 textures",
        info.name,
        info.bsp_version,
        info.materials,
        info.vertices,
        info.triangles,
        draw_calls,
        info.displacements_built,
        info.displacement_vertices,
        info.displacement_triangles,
        info.brush_entities,
        info.brush_entity_triangles,
        info.faces_built,
        info.faces_total,
        info.displacement_faces,
        lightmap_summary(&info, *mode),
        material_summary(&info),
        rate.frame_ms,
        rate.fps,
        if vsync { ", vsync" } else { "" },
        if wireframe.global { "  [wireframe]" } else { "" },
    ));
}

/// The HUD's lightmap line: atlas size, how much of it holds real samples, and
/// how many faces the compiler left unlit.
pub fn lightmap_summary(info: &MapInfo, mode: AlbedoMode) -> String {
    if info.lightmap_faces == 0 {
        return "off".to_string();
    }
    format!(
        "{}x{} atlas  {:.0}% packed  {} faces  {} unlit  peak {:.1}  albedo {}",
        info.lightmap_size.0,
        info.lightmap_size.1,
        info.lightmap_occupancy * 100.0,
        info.lightmap_faces,
        info.lightmap_unlit,
        info.lightmap_peak,
        match mode {
            AlbedoMode::Textured => "textured",
            AlbedoMode::DebugPalette => "palette",
        },
    )
}

/// The HUD's material line: what resolved, and how the textures reached the GPU.
pub fn material_summary(info: &MapInfo) -> String {
    if info.materials_resolved == 0 && info.textures == 0 {
        return "off".to_string();
    }
    format!(
        "{} materials  {} missing  {} textures ({} BC, {} decoded, {:.1} MiB)           {} blend  {} alpha-test  {} translucent",
        info.materials_resolved,
        info.materials_missing,
        info.textures,
        info.bcn_passthrough,
        info.cpu_decoded,
        info.texture_bytes as f64 / 1_048_576.0,
        info.vertex_blend,
        info.alpha_tested,
        info.translucent,
    )
}
