//! Milestone 1.1: brush geometry straight from the BSP, no glTF in the middle.
//!
//! Loading is synchronous in a startup system on purpose. The `AssetLoader` and
//! the VPK-backed `AssetSource` the roadmap describes only start paying for
//! themselves once materials need resolving, which is 1.2 -- and by then
//! `tf-asset-loader` (with its `bsp` feature, which mounts the map's own
//! pakfile as a source) has already done most of that job.

pub mod geometry;

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::render_resource::Face as CullFace;

pub use geometry::{HAMMER_UNIT, REFERENCE_YAW, Stats};

/// Where to find the map. Defaults to a stock Windows Steam install; override
/// with `TF2_BSP=/path/to/map.bsp` for custom maps or a different library.
const DEFAULT_BSP: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\tf\maps\ctf_2fort.bsp";

pub fn bsp_path() -> PathBuf {
    std::env::var_os("TF2_BSP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_BSP))
}

/// Marks everything spawned by our own loader, so the A/B toggle can hide it in
/// one query.
#[derive(Component)]
pub struct BspGeometry;

/// Two materials per batch: the plain one and a debug colour keyed on texture
/// name. Swapping between them is the fastest way to see whether faces landed in
/// the batch you expected.
#[derive(Component)]
pub struct BatchMaterials {
    pub plain: Handle<StandardMaterial>,
    pub debug: Handle<StandardMaterial>,
}

#[derive(Resource, Default)]
pub struct BspReport {
    pub stats: Stats,
    pub batches: usize,
    pub largest: Vec<(String, usize)>,
    /// Brush-entity classnames and how many models each owns. Worth having on
    /// screen: it is the direct evidence for whether the `*N` lookup is picking
    /// up more than the four classnames the reference tools hardcode.
    pub brush_entities: Vec<(String, usize)>,
    pub error: Option<String>,
}

pub struct BspPlugin;

impl Plugin for BspPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BspReport>()
            .add_systems(Startup, load_bsp);
    }
}

fn load_bsp(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut report: ResMut<BspReport>,
) {
    let path = bsp_path();
    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(e) => {
            report.error = Some(format!("{}: {e}", path.display()));
            error!("could not read {}: {e}", path.display());
            return;
        }
    };
    // vbsp handles the LZMA-compressed lumps TF2 ships. Worth knowing that it
    // does not expose the lighting lump at all, which is what blocks 1.4.
    let bsp = match vbsp::Bsp::read(&data) {
        Ok(bsp) => bsp,
        Err(e) => {
            report.error = Some(format!("{e}"));
            error!("could not parse {}: {e}", path.display());
            return;
        }
    };

    let (batches, stats) = geometry::build(&bsp);
    info!(
        "built {} batches, {} triangles from {} faces in {:?}",
        batches.len(),
        stats.triangles,
        stats.faces_drawn,
        stats.build_time
    );

    let mut by_class: std::collections::HashMap<String, usize> = default();
    for model in geometry::brush_models(&bsp) {
        *by_class.entry(model.classname).or_default() += 1;
    }
    report.brush_entities = by_class.into_iter().collect();
    report.brush_entities.sort_by_key(|e| std::cmp::Reverse(e.1));

    report.stats = stats;
    report.batches = batches.len();
    report.largest = batches
        .iter()
        .take(5)
        .map(|b| (b.texture.clone(), b.triangles))
        .collect();

    let root = commands
        .spawn((
            BspGeometry,
            Transform::from_rotation(Quat::from_rotation_y(REFERENCE_YAW)),
            Visibility::default(),
        ))
        .id();

    for batch in batches {
        let plain = materials.add(StandardMaterial {
            base_color: Color::srgb(0.72, 0.72, 0.72),
            perceptual_roughness: 1.0,
            metallic: 0.0,
            ..default()
        });
        let debug = materials.add(StandardMaterial {
            base_color: batch.debug_color,
            unlit: true,
            ..default()
        });
        commands.spawn((
            ChildOf(root),
            BspGeometry,
            Mesh3d(meshes.add(batch.mesh)),
            MeshMaterial3d(debug.clone()),
            BatchMaterials { plain, debug },
        ));
    }
}

/// Backface culling starts off, per the roadmap's advice: get something on
/// screen first, then turn culling on and see whether it survives.
pub fn set_culling(
    materials: &mut Assets<StandardMaterial>,
    handles: &[Handle<StandardMaterial>],
    on: bool,
) {
    for handle in handles {
        if let Some(mut material) = materials.get_mut(handle) {
            material.cull_mode = on.then_some(CullFace::Back);
            material.double_sided = !on;
        }
    }
}
