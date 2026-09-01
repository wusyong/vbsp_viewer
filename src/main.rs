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

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::pbr::wireframe::{WireframeConfig, WireframePlugin};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::{CursorGrabMode, CursorOptions, PresentMode, PrimaryWindow};
use bevy_bsp::{
    DisplacementGeometry, MapGeometry, MapInfo, SourceMaterial, SourceMaterialPlugin, SurfaceInfo,
};
use bsp::geometry::ModelGeometry;
use clap::Parser;
use std::path::{Path, PathBuf};

/// Where TF2 lives if `--game-dir` and the environment say nothing.
const DEFAULT_GAME_DIR: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\tf";
const GAME_DIR_ENV: &str = "BEVY_TF2_GAME_DIR";

/// Base fly speed and sprint multiplier, in Source units per second. TF2's
/// base run speed is 300 u/s, so this is a little faster than running.
const FLY_SPEED_UNITS: f32 = 500.0;
const SPRINT_MULTIPLIER: f32 = 6.0;

/// Radians of rotation per pixel of mouse motion.
const LOOK_SENSITIVITY: f32 = 0.0022;

#[derive(Parser, Debug, Resource, Clone)]
#[command(about = "View Team Fortress 2 BSP maps in Bevy")]
struct Args {
    /// Map name (e.g. `cp_badlands`) or a path to a .bsp file.
    #[arg(short, long, default_value = "ctf_2fort")]
    map: String,

    /// TF2 `tf/` directory. Defaults to $BEVY_TF2_GAME_DIR, then the usual
    /// Steam location.
    #[arg(short, long)]
    game_dir: Option<PathBuf>,

    /// Start with the mouse already captured.
    #[arg(long)]
    grab: bool,

    /// Uncap the frame rate. Bevy defaults to `PresentMode::Fifo` (strict
    /// vsync), which pins the frame rate to the display refresh and hides how
    /// expensive a frame really is. Pass this to measure actual headroom.
    #[arg(long)]
    no_vsync: bool,

    /// Render a few frames, save a PNG here, and exit. Used to verify
    /// rendering without a human at the keyboard, and to diff against the
    /// real game.
    #[arg(long)]
    screenshot: Option<PathBuf>,

    /// Camera position in Source units, as `x,y,z`. Matches `cl_showpos`, so a
    /// screenshot can be framed identically to one from TF2.
    /// Negative coordinates are common, so hyphens are values here, not flags.
    #[arg(long, value_parser = parse_source_vec, allow_hyphen_values = true)]
    pos: Option<Vec3>,

    /// Camera yaw and pitch in degrees, as `yaw,pitch`.
    #[arg(long, value_parser = parse_angles, allow_hyphen_values = true)]
    angles: Option<Vec2>,

    /// Skip the baked lightmaps and light the map with a debug directional
    /// light instead — the M3/M4 look, useful for judging geometry alone.
    #[arg(long)]
    no_lightmap: bool,

    /// Skip `$basetexture` and paint each material a flat debug colour — the
    /// M5 look, for telling a lighting problem from a texture problem.
    #[arg(long)]
    no_textures: bool,

    /// Override the lightmap brightness. Defaults to the value that maps a
    /// luxel of 1.0 onto full exposure; see `bevy_bsp::lightmap_exposure`.
    #[arg(long)]
    lightmap_exposure: Option<f32>,
}

/// Parse `x,y,z` in Source units into a Bevy-space position.
fn parse_source_vec(text: &str) -> Result<Vec3, String> {
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() != 3 {
        return Err("expected three comma-separated numbers, e.g. -1200,900,300".into());
    }
    let mut v = [0.0f32; 3];
    for (slot, part) in v.iter_mut().zip(parts) {
        *slot = part.trim().parse().map_err(|_| format!("bad number {part:?}"))?;
    }
    Ok(bevy_bsp::src_to_bevy(v))
}

fn parse_angles(text: &str) -> Result<Vec2, String> {
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() != 2 {
        return Err("expected `yaw,pitch` in degrees".into());
    }
    let yaw: f32 = parts[0].trim().parse().map_err(|_| "bad yaw".to_string())?;
    let pitch: f32 = parts[1].trim().parse().map_err(|_| "bad pitch".to_string())?;
    Ok(Vec2::new(yaw.to_radians(), pitch.to_radians()))
}

impl Args {
    fn game_dir(&self) -> PathBuf {
        self.game_dir
            .clone()
            .or_else(|| std::env::var_os(GAME_DIR_ENV).map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_GAME_DIR))
    }
}

/// Resolve `--map` to a file: an explicit path wins, otherwise look under
/// `<game-dir>/maps/`.
fn resolve_map(map: &str, game_dir: &Path) -> Result<PathBuf, String> {
    let direct = Path::new(map);
    if direct.is_file() {
        return Ok(direct.to_path_buf());
    }

    let file_name = if map.to_ascii_lowercase().ends_with(".bsp") {
        map.to_string()
    } else {
        format!("{map}.bsp")
    };
    let candidate = game_dir.join("maps").join(&file_name);
    if candidate.is_file() {
        return Ok(candidate);
    }

    let maps_dir = game_dir.join("maps");
    if !maps_dir.is_dir() {
        return Err(format!(
            "no maps directory at {}\nPass --game-dir or set {GAME_DIR_ENV} to your TF2 tf/ folder.",
            maps_dir.display()
        ));
    }
    // A typo in a map name is the likely case, so suggest near misses.
    let needle = map.to_ascii_lowercase();
    let mut hits: Vec<String> = std::fs::read_dir(&maps_dir)
        .map_err(|e| format!("{}: {e}", maps_dir.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()))
        .filter(|stem| stem.to_ascii_lowercase().contains(&needle))
        .collect();
    hits.sort();
    hits.truncate(10);

    if hits.is_empty() {
        Err(format!("no map matching {map:?} under {}", maps_dir.display()))
    } else {
        Err(format!(
            "no map named {map:?}. Did you mean:\n  {}",
            hits.join("\n  ")
        ))
    }
}

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

#[derive(Resource)]
struct MapPath(PathBuf);

/// Smoothed frame timing.
///
/// Under the default `PresentMode::Fifo` this reports the display refresh rate
/// rather than the cost of a frame, so `frame_ms` is the number worth reading;
/// run with `--no-vsync` to make `fps` meaningful.
#[derive(Resource, Default)]
struct FrameRate {
    fps: f32,
    frame_ms: f32,
    samples: u32,
}

#[derive(Component)]
struct FlyCamera {
    yaw: f32,
    pitch: f32,
}

#[derive(Component)]
struct HudText;

/// Visibility of brush geometry and of terrain, as two provably disjoint
/// queries.
///
/// Each filter excludes the *other* marker as well as the HUD. Without that,
/// Bevy cannot prove two `&mut Visibility` queries do not overlap — an entity
/// could in principle carry both markers — and panics with B0001.
type BrushVisibility<'w, 's> = Query<
    'w,
    's,
    &'static mut Visibility,
    (
        With<MapGeometry>,
        Without<DisplacementGeometry>,
        Without<HudText>,
    ),
>;

type TerrainVisibility<'w, 's> = Query<
    'w,
    's,
    &'static mut Visibility,
    (
        With<DisplacementGeometry>,
        Without<MapGeometry>,
        Without<HudText>,
    ),
>;

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

/// Read the BSP, build worldspawn, and spawn one entity per material.
///
/// This loads synchronously rather than through a Bevy `AssetLoader`: the file
/// is memory-mapped and lives outside the asset directory, and a map builds in
/// well under a frame budget's worth of patience. Async loading and
/// hot-reloading are worth revisiting once there is more than one map in play.
// Bevy systems take their dependencies as parameters, so the count here is
// the injection list, not a signature that wants shortening.
#[allow(clippy::too_many_arguments)]
fn load_map(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SourceMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut info: ResMut<MapInfo>,
    mut camera: Query<(&mut Transform, &mut FlyCamera)>,
    path: Res<MapPath>,
    args: Res<Args>,
) {
    let started = std::time::Instant::now();

    let bsp = match bsp::Bsp::open(&path.0) {
        Ok(b) => b,
        Err(e) => {
            error!("failed to open {}: {e}", path.0.display());
            return;
        }
    };
    // The search path stack, with this map's own pakfile on top — that is
    // where its cubemap-patched materials live.
    let vfs = match open_vfs(&bsp, &args) {
        Ok(vfs) => Some(vfs),
        Err(e) => {
            // Worth continuing: the geometry and the bake are still worth
            // looking at, and the debug palette says plainly that no materials
            // were found.
            error!("no material search paths, rendering untextured: {e}");
            None
        }
    };

    let mut world = match bsp::geometry::build_worldspawn(&bsp) {
        Ok(g) => g,
        Err(e) => {
            error!("failed to build geometry: {e}");
            return;
        }
    };
    let names = match bsp.texture_names() {
        Ok(n) => n,
        Err(e) => {
            error!("failed to read material names: {e}");
            return;
        }
    };

    let map_name = path
        .0
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut displacements = match bsp::displacement::build_displacements(&bsp) {
        Ok(d) => d,
        Err(e) => {
            error!("failed to tessellate displacements: {e}");
            return;
        }
    };

    // Brush entities: doors, gates, and the func_brush detail that makes up a
    // lot of a TF2 map. Each is a BSP model placed at the entity's origin, and
    // finding them needs the ENTITIES lump's KeyValues — which is why this
    // waited for the `vfs` crate rather than shipping with M2.
    let mut brush_models = match brush_entity_models(&bsp) {
        Ok(models) => models,
        Err(e) => {
            // Worth continuing without: the world still renders, and missing
            // doors is a far smaller problem than no map at all.
            error!("failed to build brush entities: {e}");
            Vec::new()
        }
    };

    // Pack the bake, then rewrite UV_1 in place. Every builder records its
    // source face per chunk, so world, terrain and brush entities go through
    // the same atlas — a displacement's lighting lives on its parent face.
    let atlas = if args.no_lightmap {
        None
    } else {
        let faces: Vec<u32> = bsp::lightmap::faces_of(&world.surfaces)
            .chain(bsp::lightmap::faces_of(&displacements.surfaces))
            .chain(
                brush_models
                    .iter()
                    .flat_map(|entity| bsp::lightmap::faces_of(&entity.geometry.surfaces)),
            )
            .collect();
        match bsp::lightmap::build(&bsp, faces, bsp::lightmap::Lighting::Ldr) {
            Ok(atlas) => {
                atlas.remap(&mut world.surfaces);
                atlas.remap(&mut displacements.surfaces);
                for entity in &mut brush_models {
                    atlas.remap(&mut entity.geometry.surfaces);
                }
                Some(atlas)
            }
            Err(e) => {
                // Worth continuing without: the geometry is still useful, and
                // a silent fallback to fullbright would be indistinguishable
                // from a black bake.
                error!("failed to pack lightmaps, rendering unlit: {e}");
                None
            }
        }
    };
    let lightmap = atlas.as_ref().map(|atlas| bevy_bsp::MapLightmap {
        image: images.add(bevy_bsp::lightmap_image(atlas)),
        exposure: args.lightmap_exposure.unwrap_or_else(|| {
            // Source's overbright of 2 exists to be cancelled by a real
            // texture's reflectance; against the flat debug palette it would
            // only clip every sunlit surface.
            let overbright = if args.no_textures {
                1.0
            } else {
                bevy_bsp::SOURCE_OVERBRIGHT
            };
            bevy_bsp::lightmap_exposure(overbright)
        }),
    });

    // One material per texdata, resolved once. With no VFS there is nothing to
    // resolve and everything falls back to the debug palette.
    // Only the texdatas that survived face filtering: tool and trigger
    // materials are in the lump but never drawn.
    let used_texdata: Vec<i32> = world
        .surfaces
        .iter()
        .chain(displacements.surfaces.iter())
        .chain(
            brush_models
                .iter()
                .flat_map(|entity| entity.geometry.surfaces.iter()),
        )
        .map(|surface| surface.texdata)
        .collect();

    let map_materials = bevy_bsp::load_materials(
        &bevy_bsp::MaterialContext {
            vfs: vfs.as_ref(),
            material_names: &names,
            lightmap_exposure: lightmap.as_ref().map_or(0.0, |l| l.exposure),
            debug_palette: args.no_textures,
        },
        used_texdata,
        &mut materials,
        &mut images,
    );

    let surfaces = bevy_bsp::MapSurfaces {
        material_names: &names,
        materials: &map_materials,
        lightmap: lightmap.as_ref(),
    };
    let draw_calls = bevy_bsp::spawn_model(
        &mut commands,
        &mut meshes,
        &world,
        surfaces,
        [0.0; 3],
        "worldspawn",
    );
    let disp_draw_calls = bevy_bsp::spawn_displacements(
        &mut commands,
        &mut meshes,
        &displacements,
        surfaces,
    );
    let mut entity_draw_calls = 0usize;
    for entity in &brush_models {
        entity_draw_calls += bevy_bsp::spawn_model(
            &mut commands,
            &mut meshes,
            &entity.geometry,
            surfaces,
            entity.origin,
            &entity.name,
        );
    }

    *info = bevy_bsp::map_info(
        &map_name,
        bsp.version(),
        &world,
        &displacements,
        atlas.as_ref(),
    );
    *info = info.clone().with_materials(&map_materials.stats);
    info.brush_entities = brush_models.len();
    info.brush_entity_triangles = brush_models
        .iter()
        .map(|e| e.geometry.triangle_count())
        .sum();

    for missing in map_materials.stats.missing.iter().take(10) {
        warn!("no material for {missing}");
    }
    for missing in map_materials.stats.missing_textures.iter().take(10) {
        warn!("no texture at {missing}");
    }

    let start = default_viewpoint(&world);
    if let Ok((mut transform, mut fly)) = camera.single_mut() {
        transform.translation = args
            .pos
            .unwrap_or_else(|| bevy_bsp::src_to_bevy(start.position));
        let angles = args.angles.unwrap_or(Vec2::new(start.yaw, start.pitch));
        fly.yaw = angles.x;
        fly.pitch = angles.y;
    }

    info!(
        "{map_name}: {} materials, {} verts, {} tris ({draw_calls} draw calls); \n         {} disps, {} verts, {} tris ({disp_draw_calls} draw calls); \n         {} brush entities, {} tris ({entity_draw_calls} draw calls); {:.0} ms",
        info.materials,
        info.vertices,
        info.triangles,
        info.displacements_built,
        info.displacement_vertices,
        info.displacement_triangles,
        info.brush_entities,
        info.brush_entity_triangles,
        started.elapsed().as_secs_f32() * 1000.0,
    );
}

/// Mount the game's search paths with this map's pakfile on top.
///
/// The two error types do not convert into one another, so this is a function
/// rather than a `?` chain inside `load_map`.
fn open_vfs(bsp: &bsp::Bsp, args: &Args) -> Result<vfs::Vfs, Box<dyn std::error::Error>> {
    let mut vfs = vfs::Vfs::from_game_dir(args.game_dir())?;
    vfs.mount_pakfile(bsp.lump_bytes(bsp::LumpId::PakFile)?.to_vec())?;
    Ok(vfs)
}

/// One brush entity, ready to spawn.
struct BrushEntity {
    geometry: ModelGeometry,
    /// Source units; the entity's `origin` key.
    origin: [f32; 3],
    /// `classname` and `targetname`, for the entity's `Name` in the scene.
    name: String,
}

/// Build the geometry of every entity that draws a BSP model.
///
/// Model 0 is worldspawn and is excluded by `vfs::entities::brush_entities`.
/// A model index the MODELS lump does not have is skipped rather than failing
/// the map — one broken entity should not cost the whole level.
fn brush_entity_models(bsp: &bsp::Bsp) -> Result<Vec<BrushEntity>, Box<dyn std::error::Error>> {
    let entities = vfs::entities::parse(bsp.entities_str()?)?;
    let model_count = bsp.models()?.len();

    let mut out = Vec::new();
    for (model, entity) in vfs::entities::brush_entities(&entities) {
        if model >= model_count {
            warn!(
                "{} references model *{model}, but the map has {model_count}",
                entity.classname
            );
            continue;
        }
        let geometry = match bsp::geometry::build_model(bsp, model) {
            Ok(g) => g,
            Err(e) => {
                warn!("{}: model *{model} failed to build: {e}", entity.classname);
                continue;
            }
        };
        // An entity with no visible faces — a trigger volume, a clip brush —
        // would spawn nothing but still cost an entity and a HUD line.
        if geometry.surfaces.is_empty() {
            continue;
        }
        let name = match entity.targetname() {
            Some(target) => format!("{}[{target}]", entity.classname),
            None => format!("{}*{model}", entity.classname),
        };
        out.push(BrushEntity {
            geometry,
            origin: entity.origin(),
            name,
        });
    }
    Ok(out)
}

/// Where the camera starts when `--pos`/`--angles` are not given.
struct Viewpoint {
    /// Source units.
    position: [f32; 3],
    /// Radians, matching [`FlyCamera`].
    yaw: f32,
    pitch: f32,
}

/// Frame the whole map from outside it.
///
/// Two traps make the obvious approaches fail:
///
/// * The MODELS-lump bounding box includes 3D-skybox brushes and pits, so on
///   `cp_badlands` it spans 16 500 units vertically and its centre is
///   thousands of units below the floor. Percentile-trimmed bounds ignore
///   those outliers.
/// * Any point sampled *inside* the level — a median, say — lands in solid
///   geometry. On `ctf_2fort` the horizontal median is inside the central
///   tower, so the view is a wall whatever height it is used at.
///
/// So: take trimmed bounds, then stand back along a diagonal and look at their
/// centre, at a distance scaled to the map's horizontal size.
fn default_viewpoint(geometry: &ModelGeometry) -> Viewpoint {
    let total: usize = geometry.surfaces.iter().map(|s| s.vertices.len()).sum();
    if total == 0 {
        return Viewpoint {
            position: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
        };
    }

    // Trimmed extents per axis: the 2nd and 98th percentiles.
    let mut lo = [0.0f32; 3];
    let mut hi = [0.0f32; 3];
    let mut values = Vec::with_capacity(total);
    for axis in 0..3 {
        values.clear();
        values.extend(
            geometry
                .surfaces
                .iter()
                .flat_map(|s| s.vertices.iter().map(|v| v.position[axis])),
        );
        let last = values.len() - 1;
        // `total_cmp`: a NaN would make a `partial_cmp` comparator inconsistent.
        let at = |values: &mut Vec<f32>, q: f32| {
            let n = (last as f32 * q) as usize;
            values.select_nth_unstable_by(n, |a, b| a.total_cmp(b));
            values[n]
        };
        lo[axis] = at(&mut values, 0.02);
        hi[axis] = at(&mut values, 0.98);
    }

    let centre = [
        (lo[0] + hi[0]) * 0.5,
        (lo[1] + hi[1]) * 0.5,
        (lo[2] + hi[2]) * 0.5,
    ];
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1.0);

    // Stand off along a diagonal and above, so the shot reads as an overview
    // rather than an elevation.
    let offset = Vec3::new(-0.55, -0.70, 0.45).normalize();
    let distance = span * 0.85;
    let position = [
        centre[0] + offset.x * distance,
        centre[1] + offset.y * distance,
        centre[2] + offset.z * distance,
    ];

    // Aim back at the centre. Invert the yaw/pitch composition used by
    // `fly_camera`: forward = (-sin y * cos p, sin p, -cos y * cos p).
    let look = bevy_bsp::src_to_bevy_dir([-offset.x, -offset.y, -offset.z]).normalize();
    Viewpoint {
        position,
        yaw: (-look.x).atan2(-look.z),
        pitch: look.y.clamp(-1.0, 1.0).asin(),
    }
}

/// Click to capture the mouse, Escape to release.
fn grab_cursor(
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    args: Res<Args>,
    mut initialised: Local<bool>,
) {
    let Ok(mut cursor) = cursor.single_mut() else {
        return;
    };

    if !*initialised {
        *initialised = true;
        if args.grab {
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        }
    }

    if mouse.just_pressed(MouseButton::Left) {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

fn fly_camera(
    mut camera: Query<(&mut Transform, &mut FlyCamera)>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    motion: Res<AccumulatedMouseMotion>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
) {
    let Ok((mut transform, mut fly)) = camera.single_mut() else {
        return;
    };
    let grabbed = cursor
        .single()
        .is_ok_and(|c| c.grab_mode != CursorGrabMode::None);

    if grabbed && motion.delta != Vec2::ZERO {
        fly.yaw -= motion.delta.x * LOOK_SENSITIVITY;
        // Stop just short of vertical: looking exactly along the up axis makes
        // the yaw/pitch basis degenerate.
        const LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;
        fly.pitch = (fly.pitch - motion.delta.y * LOOK_SENSITIVITY).clamp(-LIMIT, LIMIT);
    }
    transform.rotation = Quat::from_euler(EulerRot::YXZ, fly.yaw, fly.pitch, 0.0);

    let mut direction = Vec3::ZERO;
    let forward = *transform.forward();
    let right = *transform.right();
    for (key, delta) in [
        (KeyCode::KeyW, forward),
        (KeyCode::KeyS, -forward),
        (KeyCode::KeyD, right),
        (KeyCode::KeyA, -right),
        (KeyCode::KeyE, Vec3::Y),
        (KeyCode::KeyQ, -Vec3::Y),
    ] {
        if keys.pressed(key) {
            direction += delta;
        }
    }

    if direction != Vec3::ZERO {
        let sprint = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            SPRINT_MULTIPLIER
        } else {
            1.0
        };
        let speed = FLY_SPEED_UNITS * bevy_bsp::SOURCE_UNIT_METRES * sprint;
        transform.translation += direction.normalize() * speed * time.delta_secs();
    }
}

fn toggle_debug_views(
    keys: Res<ButtonInput<KeyCode>>,
    mut wireframe: ResMut<WireframeConfig>,
    mut hud: Query<&mut Visibility, With<HudText>>,
    mut geometry: BrushVisibility,
    mut terrain: TerrainVisibility,
) {
    fn toggled(current: Visibility) -> Visibility {
        match current {
            Visibility::Hidden => Visibility::Inherited,
            _ => Visibility::Hidden,
        }
    }

    if keys.just_pressed(KeyCode::F1) {
        for mut visibility in &mut hud {
            *visibility = match *visibility {
                Visibility::Hidden => Visibility::Inherited,
                _ => Visibility::Hidden,
            };
        }
    }
    if keys.just_pressed(KeyCode::F2) {
        wireframe.global = !wireframe.global;
    }
    // Isolating brush geometry from terrain is the fastest way to tell which
    // builder is at fault when something looks wrong.
    if keys.just_pressed(KeyCode::F3) {
        for mut visibility in &mut geometry {
            *visibility = toggled(*visibility);
        }
    }
    if keys.just_pressed(KeyCode::F4) {
        for mut visibility in &mut terrain {
            *visibility = toggled(*visibility);
        }
    }
}

/// Whether surfaces render with their `$basetexture` or a flat colour per
/// material.
///
/// The palette view is how you tell a lighting problem from a texture problem,
/// and how a map's material grouping — one draw call each — becomes visible.
#[derive(Resource, Default, PartialEq, Eq, Clone, Copy)]
enum AlbedoMode {
    #[default]
    Textured,
    DebugPalette,
}

/// The `$basetexture` each material had before the palette was switched on, so
/// it can be put back.
#[derive(Resource, Default)]
struct SavedTextures(std::collections::HashMap<AssetId<SourceMaterial>, Option<Handle<Image>>>);

/// Swap between textured and flat-colour albedo on F5.
///
/// Editing the material assets in place rather than swapping handles keeps one
/// material per texdata, so the draw call count in the HUD stays honest.
fn swap_albedo(
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<AlbedoMode>,
    mut saved: ResMut<SavedTextures>,
    mut materials: ResMut<Assets<SourceMaterial>>,
    surfaces: Query<(&SurfaceInfo, &MeshMaterial3d<SourceMaterial>)>,
    args: Res<Args>,
) {
    // `--no-textures` already renders the palette; there is nothing to go back
    // to, because no texture was ever loaded.
    if !keys.just_pressed(KeyCode::F5) || args.no_textures {
        return;
    }

    *mode = match *mode {
        AlbedoMode::Textured => AlbedoMode::DebugPalette,
        AlbedoMode::DebugPalette => AlbedoMode::Textured,
    };
    for (surface, handle) in &surfaces {
        let id = handle.0.id();
        let Some(mut material) = materials.get_mut(&handle.0) else {
            continue;
        };
        match *mode {
            AlbedoMode::DebugPalette => {
                saved
                    .0
                    .entry(id)
                    .or_insert_with(|| material.base.base_color_texture.clone());
                material.base.base_color_texture = None;
                material.base.base_color = bevy_bsp::debug_material_color(&surface.material);
                material.extension.params.blend = 0;
            }
            AlbedoMode::Textured => {
                material.base.base_color_texture =
                    saved.0.get(&id).cloned().unwrap_or_default();
                material.base.base_color = Color::WHITE;
                material.extension.params.blend =
                    u32::from(material.extension.base_texture2.is_some());
            }
        }
    }
}

/// Frames to ignore before measuring. Window creation and shader pipeline
/// compilation make the first frames wildly unrepresentative, and an average
/// seeded from them reads high for a long time — high enough to report a frame
/// rate above the vsync cap, which is how this was caught.
const FRAME_RATE_WARMUP: u32 = 10;

fn tick_frame_rate(mut rate: ResMut<FrameRate>, time: Res<Time>) {
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
fn update_hud(
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

/// The HUD's lightmap line: atlas size, how much of it holds real samples, and
/// how many faces the compiler left unlit.
fn lightmap_summary(info: &MapInfo, mode: AlbedoMode) -> String {
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
fn material_summary(info: &MapInfo) -> String {
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
