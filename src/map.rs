//! Reading a BSP off disk and turning it into spawned entities.
//!
//! Loading is synchronous rather than an `AssetLoader`: the file is
//! memory-mapped, lives outside the asset directory, and builds in well under
//! a frame budget's worth of patience.
use crate::camera::FlyCamera;
use crate::cli::Args;
use crate::viewpoint::default_viewpoint;
use bevy::prelude::*;
use bevy_bsp::{MapInfo, SourceMaterial};
use bsp::geometry::ModelGeometry;
use std::path::PathBuf;

#[derive(Resource)]
pub struct MapPath(pub PathBuf);

/// Read the BSP, build worldspawn, and spawn one entity per material.
///
/// This loads synchronously rather than through a Bevy `AssetLoader`: the file
/// is memory-mapped and lives outside the asset directory, and a map builds in
/// well under a frame budget's worth of patience. Async loading and
/// hot-reloading are worth revisiting once there is more than one map in play.
// Bevy systems take their dependencies as parameters, so the count here is
// the injection list, not a signature that wants shortening.
#[allow(clippy::too_many_arguments)]
pub fn load_map(
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
    let entities = match bsp
        .entities_str()
        .map_err(|e| e.to_string())
        .and_then(|text| vfs::entities::parse(text).map_err(|e| e.to_string()))
    {
        Ok(entities) => entities,
        Err(e) => {
            error!("failed to parse the ENTITIES lump: {e}");
            Vec::new()
        }
    };

    let mut brush_models = match brush_entity_models(&bsp, &entities) {
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

    let start = default_viewpoint(&world, &entities);
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
pub fn open_vfs(bsp: &bsp::Bsp, args: &Args) -> Result<vfs::Vfs, Box<dyn std::error::Error>> {
    let mut vfs = vfs::Vfs::from_game_dir(args.game_dir())?;
    vfs.mount_pakfile(bsp.lump_bytes(bsp::LumpId::PakFile)?.to_vec())?;
    Ok(vfs)
}

/// One brush entity, ready to spawn.
pub struct BrushEntity {
    pub geometry: ModelGeometry,
    /// Source units; the entity's `origin` key.
    pub origin: [f32; 3],
    /// `classname` and `targetname`, for the entity's `Name` in the scene.
    pub name: String,
}

/// Build the geometry of every entity that draws a BSP model.
///
/// Model 0 is worldspawn and is excluded by `vfs::entities::brush_entities`.
/// A model index the MODELS lump does not have is skipped rather than failing
/// the map — one broken entity should not cost the whole level.
pub fn brush_entity_models(
    bsp: &bsp::Bsp,
    entities: &[vfs::Entity],
) -> Result<Vec<BrushEntity>, Box<dyn std::error::Error>> {
    let model_count = bsp.models()?.len();

    let mut out = Vec::new();
    for (model, entity) in vfs::entities::brush_entities(entities) {
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
