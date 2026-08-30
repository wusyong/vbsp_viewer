//! TF2 → Bevy, phase 1.
//!
//! Two ways to look at the same map, switchable at runtime with `G`:
//!
//! - **Reference**: `ctf_2fort.glb`, produced by `vbsp-to-gltf`. Known-good, and
//!   what step 0 established. Textured, but static and unlit.
//! - **BSP**: our own `vbsp` → `Mesh` path (milestone 1.1). Untextured so far.
//!
//! Keeping both in one scene is the point: a geometry bug in the BSP path shows
//! up instantly as a difference from the reference, which is far easier than
//! deciding in the abstract whether a wall is in the right place.

mod bsp;
mod vfs;
mod vmt;
mod vtf;

use std::f32::consts::FRAC_PI_2;

use bevy::{
    input::mouse::MouseMotion,
    prelude::*,
    render::view::{
        NoIndirectDrawing,
        screenshot::{Screenshot, save_to_disk},
    },
    window::{PresentMode, WindowResolution},
};

use bsp::{BatchMaterials, BspGeometry, BspPlugin, BspReport, HAMMER_UNIT};

const MAP_GLB: &str = "maps/ctf_2fort.glb";

#[derive(Resource, PartialEq, Eq, Clone, Copy)]
enum View {
    Bsp,
    Reference,
    Both,
}

impl View {
    fn label(self) -> &'static str {
        match self {
            View::Bsp => "our bsp loader",
            View::Reference => "reference glb",
            View::Both => "both, overlaid",
        }
    }

    fn next(self) -> Self {
        match self {
            View::Bsp => View::Reference,
            View::Reference => View::Both,
            View::Both => View::Bsp,
        }
    }
}

#[derive(Resource)]
struct Toggles {
    /// Whether to force backface culling on every batch. Materials set their own
    /// `cull_mode` from `$nocull`, so this overrides them wholesale; useful for
    /// bringing geometry up, not something to leave on.
    cull_override: Option<bool>,
    surface: Surface,
    lightmap_exposure: f32,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Surface {
    Textured,
    /// Per-texture colour. A face batched under the wrong texture is invisible
    /// under a real texture and obvious here.
    DebugColor,
    /// Isolates geometry problems from material ones.
    Plain,
}

impl Surface {
    fn label(self) -> &'static str {
        match self {
            Surface::Textured => "textured",
            Surface::DebugColor => "per-texture colour",
            Surface::Plain => "plain",
        }
    }

    fn next(self) -> Self {
        match self {
            Surface::Textured => Surface::DebugColor,
            Surface::DebugColor => Surface::Plain,
            Surface::Plain => Surface::Textured,
        }
    }
}

#[derive(Component)]
struct Reference;

fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v != "0" && !v.is_empty())
}

fn main() {
    App::new()
        .insert_resource(ClearColor(Color::linear_rgb(0.35, 0.55, 0.75)))
        .insert_resource(View::Bsp)
        // Overridable so `--shot` can capture any combination without a human at
        // the keyboard: `TF2_CULL=1 TF2_PLAIN=1 cargo run -- --shot`.
        .insert_resource(Toggles {
            cull_override: flag("TF2_CULL").then_some(true),
            surface: if flag("TF2_PLAIN") {
                Surface::Plain
            } else if flag("TF2_DEBUG_COLOR") {
                Surface::DebugColor
            } else {
                Surface::Textured
            },
            lightmap_exposure: bsp::lightmap_exposure(),
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "TF2 → Bevy · ctf_2fort".into(),
                resolution: WindowResolution::new(1600, 900),
                present_mode: PresentMode::AutoNoVsync,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(BspPlugin)
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                fly_cam,
                input,
                apply_view,
                apply_toggles,
                apply_lightmap_exposure,
                fix_reference_materials,
                report,
                headless_capture,
            ),
        )
        .run();
}

/// Fly camera state. Yaw/pitch are tracked separately so mouse look does not
/// accumulate roll.
#[derive(Component)]
struct FlyCam {
    yaw: f32,
    pitch: f32,
    speed: f32,
}

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn((
        Reference,
        WorldAssetRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(MAP_GLB))),
        // The glb is in raw Hammer units, so the scale lives here rather than
        // baked in. Our own loader applies the same constant per vertex instead,
        // which is why its root carries no scale.
        Transform::from_scale(Vec3::splat(HAMMER_UNIT)),
        Visibility::Hidden,
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: light_consts::lux::OVERCAST_DAY,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(50.0, 100.0, 50.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // 2fort's long axis (blue base -> red base). `TF2_CAM=x,y,z` overrides.
    let eye = std::env::var("TF2_CAM")
        .ok()
        .and_then(|s| {
            let v: Vec<f32> = s.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
        })
        .unwrap_or(Vec3::new(0.0, 45.0, 90.0));
    let start = Transform::from_translation(eye).looking_at(Vec3::new(0.0, 5.0, 0.0), Vec3::Y);
    let (yaw, pitch, _) = start.rotation.to_euler(EulerRot::YXZ);
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            far: 10_000.0,
            ..default()
        }),
        start,
        FlyCam {
            yaw,
            pitch,
            speed: 12.0,
        },
        // A few hundred static draw calls; indirect drawing buys nothing here
        // and is a common source of blank-screen surprises.
        NoIndirectDrawing,
    ));

    commands.spawn((
        Text::new("loading…"),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(8.0),
            ..default()
        },
    ));
}

fn input(keys: Res<ButtonInput<KeyCode>>, mut view: ResMut<View>, mut toggles: ResMut<Toggles>) {
    if keys.just_pressed(KeyCode::KeyG) {
        *view = view.next();
    }
    if keys.just_pressed(KeyCode::KeyC) {
        // None (materials decide) -> forced on -> forced off -> None.
        toggles.cull_override = match toggles.cull_override {
            None => Some(true),
            Some(true) => Some(false),
            Some(false) => None,
        };
    }
    if keys.just_pressed(KeyCode::KeyT) {
        toggles.surface = toggles.surface.next();
    }
    // Lightmap brightness has no principled value — it is the conversion between
    // Source's light units and Bevy's, found by eye. Keep the knob to hand.
    if keys.just_pressed(KeyCode::Minus) {
        toggles.lightmap_exposure /= 1.5;
    }
    if keys.just_pressed(KeyCode::Equal) {
        toggles.lightmap_exposure *= 1.5;
    }
}

fn apply_lightmap_exposure(
    toggles: Res<Toggles>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    batches: Query<&BatchMaterials>,
) {
    if !toggles.is_changed() {
        return;
    }
    for batch in &batches {
        if let Some(mut material) = materials.get_mut(&batch.textured) {
            material.lightmap_exposure = toggles.lightmap_exposure;
        }
    }
}

fn apply_view(
    view: Res<View>,
    mut reference: Query<&mut Visibility, (With<Reference>, Without<BspGeometry>)>,
    mut ours: Query<&mut Visibility, (With<BspGeometry>, Without<Reference>)>,
) {
    if !view.is_changed() {
        return;
    }
    fn show(v: &mut Visibility, on: bool) {
        *v = if on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    for mut v in &mut reference {
        show(&mut v, matches!(*view, View::Reference | View::Both));
    }
    for mut v in &mut ours {
        show(&mut v, matches!(*view, View::Bsp | View::Both));
    }
}

fn apply_toggles(
    toggles: Res<Toggles>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut batches: Query<(&BatchMaterials, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    if !toggles.is_changed() {
        return;
    }
    for (batch, mut current) in &mut batches {
        let wanted = match toggles.surface {
            Surface::Textured => &batch.textured,
            Surface::DebugColor => &batch.debug,
            Surface::Plain => &batch.plain,
        };
        if current.0 != *wanted {
            current.0 = wanted.clone();
        }
        if let Some(cull) = toggles.cull_override {
            bsp::set_culling(
                &mut materials,
                &[
                    batch.textured.clone(),
                    batch.plain.clone(),
                    batch.debug.clone(),
                ],
                cull,
            );
        }
    }
}

/// `vbsp-to-gltf` writes no PBR factors, so glTF's defaults apply: metallic 1.0.
/// Fully metallic means no diffuse response, which is why the reference renders
/// almost black. Ours are built with metallic 0 already, so this is a no-op for
/// them.
fn fix_reference_materials(
    mut events: MessageReader<AssetEvent<StandardMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for event in events.read() {
        let AssetEvent::Added { id } = event else {
            continue;
        };
        if let Some(mut material) = materials.get_mut(*id)
            && material.metallic == 1.0
        {
            material.metallic = 0.0;
            material.perceptual_roughness = 1.0;
        }
    }
}

fn fly_cam(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut motion: MessageReader<MouseMotion>,
    cam: Single<(&mut Transform, &mut FlyCam)>,
) {
    let (mut transform, mut cam) = cam.into_inner();

    if buttons.pressed(MouseButton::Right) {
        let delta: Vec2 = motion.read().map(|m| m.delta).sum();
        cam.yaw -= delta.x * 0.003;
        cam.pitch = (cam.pitch - delta.y * 0.003).clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01);
        transform.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);
    } else {
        motion.clear();
    }

    // The map is ~120 m across, so a fixed speed is either too slow to cross it
    // or too fast to inspect a doorway.
    if keys.just_pressed(KeyCode::BracketRight) {
        cam.speed *= 1.5;
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        cam.speed /= 1.5;
    }

    let mut dir = Vec3::ZERO;
    for (key, axis) in [
        (KeyCode::KeyW, *transform.forward()),
        (KeyCode::KeyS, *transform.back()),
        (KeyCode::KeyA, *transform.left()),
        (KeyCode::KeyD, *transform.right()),
        (KeyCode::KeyE, Vec3::Y),
        (KeyCode::KeyQ, Vec3::NEG_Y),
    ] {
        if keys.pressed(key) {
            dir += axis;
        }
    }

    if dir != Vec3::ZERO {
        let boost = if keys.pressed(KeyCode::ShiftLeft) {
            5.0
        } else {
            1.0
        };
        transform.translation += dir.normalize() * cam.speed * boost * time.delta_secs();
    }
}

fn report(
    cam: Single<(&Transform, &FlyCam)>,
    view: Res<View>,
    toggles: Res<Toggles>,
    bsp_report: Res<BspReport>,
    time: Res<Time>,
    mut text: Single<&mut Text>,
) {
    let (transform, cam) = *cam;
    let p = transform.translation;
    let s = &bsp_report.stats;

    let mut out = String::new();
    if let Some(error) = &bsp_report.error {
        out.push_str(&format!("BSP LOAD FAILED: {error}\n"));
    }
    let m = &bsp_report.materials;
    out.push_str(&format!(
        "[G] view: {}   [C] cull: {}   [T] surface: {}\n",
        view.label(),
        match toggles.cull_override {
            None => "per-material",
            Some(true) => "forced back",
            Some(false) => "forced off",
        },
        toggles.surface.label(),
    ));
    out.push_str(&format!(
        "[- =] lightmap exposure {:.1}\n",
        toggles.lightmap_exposure
    ));
    out.push_str(&format!(
        "materials: {} ok, {} no texture, {} failed, {} water · {} BC + {} RGBA = {:.0} MB · {:.0} ms\n",
        m.resolved,
        m.missing_texture,
        m.failed,
        m.water,
        m.bc_uploaded,
        m.rgba_uploaded,
        m.bytes as f32 / (1024.0 * 1024.0),
        m.load_time.as_secs_f32() * 1000.0,
    ));
    for line in &bsp_report.missing {
        out.push_str(&format!("    ! {line}\n"));
    }
    out.push_str(&format!(
        "{} batches · {} tris · {} verts · built in {:.0} ms\n",
        bsp_report.batches,
        s.triangles,
        s.vertices,
        s.build_time.as_secs_f32() * 1000.0,
    ));
    out.push_str(&format!(
        "{} faces drawn · {} skipped (nodraw/sky/trigger) · {} displaced · {} lit\n",
        s.faces_drawn, s.faces_skipped, s.faces_displaced, s.faces_lit,
    ));
    let l = &bsp_report.lightmaps;
    match l.error {
        Some(e) => out.push_str(&format!("lightmaps: {e}\n")),
        None => out.push_str(&format!(
            "lightmap atlas {}x{} · {} styles · {} patches, {} without · {:.0} MB · {:.0} ms\n",
            l.atlas.x,
            l.atlas.y,
            l.styles,
            l.faces_with_patch,
            l.faces_without,
            l.bytes as f32 / (1024.0 * 1024.0),
            l.build_time.as_secs_f32() * 1000.0,
        )),
    }
    out.push_str(&format!(
        "{} models used, {} orphaned · side set on {} · mismatches {} ({} strong)\n",
        s.models_used,
        s.models_orphaned,
        s.faces_side_set,
        s.normal_mismatches,
        s.normal_mismatches_strong,
    ));
    for (name, tris) in &bsp_report.largest {
        out.push_str(&format!("    {tris:>7} tris  {name}\n"));
    }
    let classes: Vec<String> = bsp_report
        .brush_entities
        .iter()
        .map(|(name, n)| format!("{name} x{n}"))
        .collect();
    out.push_str(&format!("brush models: {}\n", classes.join(", ")));
    out.push_str(&format!(
        "pos {:.1}, {:.1}, {:.1} m  ({:.0}, {:.0}, {:.0} hu) · {:.1} m/s · {:.0} fps\n",
        p.x,
        p.y,
        p.z,
        p.x / HAMMER_UNIT,
        p.y / HAMMER_UNIT,
        p.z / HAMMER_UNIT,
        cam.speed,
        1.0 / time.delta_secs().max(1e-6),
    ));
    out.push_str("RMB look · WASD/QE move · Shift sprint · [ ] speed");
    text.0 = out;
}

/// `cargo run -- --shot` waits for the scene to settle, writes `shot.png` and
/// quits, so a change can be verified without a human at the keyboard.
fn headless_capture(
    mut commands: Commands,
    time: Res<Time>,
    mut shot: Local<bool>,
    mut quit: MessageWriter<AppExit>,
) {
    if !std::env::args().any(|a| a == "--shot") {
        return;
    }
    let t = time.elapsed_secs();
    if !*shot && t > 8.0 {
        *shot = true;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk("shot.png"));
    }
    if *shot && t > 10.0 {
        quit.write(AppExit::Success);
    }
}
