//! TF2 map viewer.
//!
//! ```text
//! bevy-tf2                       # defaults to ctf_2fort
//! bevy-tf2 --map cp_badlands
//! bevy-tf2 --map "C:\path\to\some.bsp"
//! ```
//!
//! Controls: WASD to move, Q/E down/up, hold Shift to sprint, click to capture
//! the mouse and Escape to release. F1 toggles the overlay, F2 wireframe,
//! F3 hides brush geometry, F4 terrain, and F5 swaps textured albedo for the
//! per-material debug palette.
//!
//! This file is app wiring only — argument parsing lives in [`cli`], map
//! loading in [`map`], camera placement in [`viewpoint`], camera control in
//! [`camera`], the debug toggles in [`debug`], and the overlay in [`hud`].

mod camera;
mod cli;
mod debug;
mod hud;
mod map;
mod viewpoint;

use crate::camera::{fly_camera, grab_cursor, FlyCamera};
use crate::cli::{resolve_map, Args};
use crate::debug::{swap_albedo, toggle_debug_views, AlbedoMode, SavedTextures};
use crate::hud::{tick_frame_rate, update_hud, FrameRate, HudText};
use crate::map::{load_map, MapPath};
use bevy::pbr::wireframe::WireframePlugin;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::PresentMode;
use bevy_bsp::{MapInfo, SourceMaterialPlugin};
use clap::Parser;

fn main() -> AppExit {
    let args = Args::parse();

    // Resolve and load before opening a window: a bad map name should print a
    // useful message to the terminal, not flash an empty window.
    let path = match resolve_map(&args.map, &args.game_dir()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return AppExit::error();
        }
    };

    App::new()
        .add_plugins(
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: format!("bevy-tf2 — {}", args.map),
                    present_mode: if args.no_vsync {
                        PresentMode::AutoNoVsync
                    } else {
                        PresentMode::default()
                    },
                    ..default()
                }),
                ..default()
            }),
        )
        .add_plugins(WireframePlugin::default())
        // Registers the material and embeds its shader; see
        // `bevy_bsp::material`.
        .add_plugins(SourceMaterialPlugin)
        .insert_resource(MapPath(path))
        .insert_resource(args)
        .init_resource::<MapInfo>()
        .init_resource::<FrameRate>()
        .init_resource::<AlbedoMode>()
        .init_resource::<SavedTextures>()
        // Chained, not a bare tuple: `load_map` repositions the camera that
        // `setup_scene` spawns, and an ordering edge is what makes Bevy insert
        // the sync point that flushes the spawn command first. Unordered, the
        // camera query silently finds nothing and the view stays at the origin.
        .add_systems(Startup, (setup_scene, load_map).chain())
        .add_systems(
            Update,
            (
                grab_cursor,
                fly_camera,
                toggle_debug_views,
                swap_albedo,
                (tick_frame_rate, update_hud).chain(),
                capture_screenshot,
            ),
        )
        .run()
}

fn setup_scene(mut commands: Commands, args: Res<Args>) {
    commands.spawn((
        Camera3d::default(),
        // TF2 maps span ~16 000 Source units (~400 m); the default far plane
        // would clip most of one.
        Projection::Perspective(PerspectiveProjection {
            near: 0.05,
            far: 2000.0,
            ..default()
        }),
        Transform::from_translation(Vec3::ZERO),
        FlyCamera {
            yaw: 0.0,
            pitch: 0.0,
        },
    ));

    // With lightmaps on, the scene is lit *only* by the bake — no directional
    // light, no ambient. Any extra light source would paper over a wrong
    // lightmap, which is exactly what this milestone has to be able to see.
    // `--no-lightmap` restores the flat debug lighting from M3/M4.
    if args.no_lightmap {
        commands.spawn((
            DirectionalLight {
                illuminance: 6_000.0,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, -0.6, -0.9, 0.0)),
        ));
    }
    // `AmbientLight` is a per-camera component in Bevy 0.19; the scene-wide
    // default is the `GlobalAmbientLight` resource.
    commands.insert_resource(GlobalAmbientLight {
        brightness: if args.no_lightmap { 1_200.0 } else { 0.0 },
        // Bevy adds ambient on top of a lightmap by default, which would lift
        // every shadow off the floor.
        affects_lightmapped_meshes: false,
        ..default()
    });

    commands.spawn((
        Text::new("loading..."),
        TextFont {
            // `FontSize` is a unit-carrying enum in 0.19, not a bare f32.
            font_size: FontSize::Px(13.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(10.0),
            ..default()
        },
        HudText,
    ));

    if args.grab {
        // Applied by `grab_cursor` on the first frame.
    }
}

/// Save a screenshot and quit, once the renderer has had a few frames to warm
/// up. Waiting matters: capturing on frame 0 catches an empty swapchain before
/// any mesh has been uploaded.
fn capture_screenshot(
    mut commands: Commands,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
    args: Res<Args>,
) {
    let Some(path) = args.screenshot.clone() else {
        return;
    };

    *frames += 1;
    // Long enough for the frame-time average to be meaningful, not merely
    // for the first mesh to appear.
    const WARMUP_FRAMES: u32 = 90;
    match *frames {
        WARMUP_FRAMES => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path.clone()));
            info!("screenshot -> {}", path.display());
        }
        // Give the save observer a few frames to finish writing the file.
        f if f > WARMUP_FRAMES + 20 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
