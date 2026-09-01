//! Bevy integration for Source BSP maps.
//!
//! This crate owns the boundary between Source's conventions and Bevy's:
//! the coordinate change, and turning [`bsp::geometry`] output into `Mesh`
//! assets. Everything below it (`bsp`) stays engine-free.
//!
//! # Coordinate systems
//!
//! Source is **Z-up**, right-handed, in units of roughly an inch. Bevy is
//! **Y-up**, right-handed, with −Z forward. [`src_to_bevy`] maps between them;
//! see its docs for why triangle winding survives untouched.

pub mod material;

pub use material::{
    load_materials, MapMaterials, MaterialContext, MaterialStats, SourceMaterial,
    SourceMaterialPlugin, SOURCE_OVERBRIGHT,
};

use bevy::asset::RenderAssetUsages;
use bevy::camera::Exposure;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::Lightmap;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bsp::displacement::DispGeometry;
use bsp::geometry::{ModelGeometry, Surface};
use bsp::lightmap::LightmapAtlas;
use half::f16;

/// Metres per Source unit.
///
/// A Source unit is nominally an inch — a 72-unit player is a six-foot human.
/// This only affects camera feel and the depth range, so it is a single knob
/// rather than something baked through the pipeline.
pub const SOURCE_UNIT_METRES: f32 = 0.0254;

/// Convert a Source-space position to Bevy space.
///
/// The basis map is `(x, y, z) → (x, z, −y)`. Its determinant is **+1**, so
/// handedness is preserved and triangle winding must *not* be reversed —
/// flipping indices here would invert every backface.
#[inline]
pub fn src_to_bevy(v: [f32; 3]) -> Vec3 {
    src_to_bevy_dir(v) * SOURCE_UNIT_METRES
}

/// Convert a Source-space direction (normal, axis) to Bevy space.
///
/// Same rotation as [`src_to_bevy`] without the unit scale, so normals stay
/// unit length.
#[inline]
pub fn src_to_bevy_dir(v: [f32; 3]) -> Vec3 {
    Vec3::new(v[0], v[2], -v[1])
}

/// Convert a Bevy-space position back to Source units.
///
/// Used by the debug HUD so a position can be pasted next to the real game's
/// `cl_showpos` output.
#[inline]
pub fn bevy_to_src(v: Vec3) -> [f32; 3] {
    let v = v / SOURCE_UNIT_METRES;
    [v.x, -v.z, v.y]
}

/// The `StandardMaterial::lightmap_exposure` that makes a Source lightmap read
/// correctly through Bevy's photographic pipeline.
///
/// Bevy grades PBR output by a camera exposure — the default is EV100 9.7, a
/// multiplier of about 1/1000 — because its light intensities are in lux.
/// Source's luxels are a plain reflectance factor around 1.0, so they have to
/// be scaled by the inverse of that exposure to land back at unity. Deriving it
/// rather than hardcoding 1000 keeps this correct if Bevy's default moves.
///
/// `overbright` is Source's own factor — [`SOURCE_OVERBRIGHT`] once a real
/// `$basetexture` is in place, since it exists to be cancelled by the texture's
/// reflectance, and 1.0 against a white placeholder, where it would only clip
/// every sunlit surface.
pub fn lightmap_exposure(overbright: f32) -> f32 {
    overbright / Exposure::default().exposure()
}

/// Upload a packed lightmap atlas as a texture.
///
/// `Rgba16Float`, because the atlas is **linear light with a peak above 1.0**
/// (5.2 on `cp_badlands`) and an 8-bit format would clip every sunlit surface
/// to white. Filtering is linear with no mips: mipping a lightmap atlas bleeds
/// one face's lighting into its neighbours, and `ClampToEdge` keeps a UV that
/// lands exactly on the far edge from wrapping to the opposite side.
pub fn lightmap_image(atlas: &LightmapAtlas) -> Image {
    let mut data = Vec::with_capacity(atlas.pixels.len() * 8);
    for p in &atlas.pixels {
        for c in p {
            data.extend_from_slice(&f16::from_f32(*c).to_le_bytes());
        }
        // Alpha is unused by the lightmap shader but the format demands it.
        data.extend_from_slice(&f16::from_f32(1.0).to_le_bytes());
    }

    let mut image = Image::new(
        Extent3d {
            width: atlas.width,
            height: atlas.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba16Float,
        // The atlas is never read back on the CPU after upload, and a 13 MiB
        // copy per map is worth not keeping.
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// Marker for entities belonging to the currently loaded map, so a reload can
/// despawn them wholesale.
#[derive(Component)]
pub struct MapGeometry;

/// Marker for tessellated displacement (terrain) entities, so the viewer can
/// show brush geometry and terrain independently while debugging.
#[derive(Component)]
pub struct DisplacementGeometry;

/// Which BSP surface an entity was built from, for the HUD and for picking.
#[derive(Component, Clone, Debug)]
pub struct SurfaceInfo {
    /// Index into the MODELS lump.
    pub model: usize,
    /// Index into the TEXDATA lump.
    pub texdata: i32,
    /// The material's path, e.g. `CONCRETE/CONCRETEWALL011`.
    pub material: String,
    /// `SURF_*` flags.
    pub flags: i32,
    pub triangles: usize,
}

/// Summary of what is currently loaded, for the debug overlay.
#[derive(Resource, Clone, Debug, Default)]
pub struct MapInfo {
    pub name: String,
    pub bsp_version: i32,
    pub materials: usize,
    pub vertices: usize,
    pub triangles: usize,
    /// Faces that produced geometry, and the total considered.
    pub faces_built: usize,
    pub faces_total: usize,
    /// Faces handled by the displacement tessellator rather than as brushes.
    pub displacement_faces: usize,
    pub displacements_built: usize,
    pub displacement_vertices: usize,
    pub displacement_triangles: usize,
    /// Model bounds in Source units.
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// Lightmap atlas dimensions, or zeroes when lightmaps are off.
    pub lightmap_size: (u32, u32),
    /// Faces given a rect in the atlas, and faces the compiler never lit.
    pub lightmap_faces: usize,
    pub lightmap_unlit: usize,
    /// Fraction of the atlas that holds real samples.
    pub lightmap_occupancy: f32,
    /// Brightest luxel channel in the map, in linear light.
    pub lightmap_peak: f32,
    /// Brush entities drawn — doors, gates, func_brush detail.
    pub brush_entities: usize,
    pub brush_entity_triangles: usize,
    /// Materials resolved, and how their textures reached the GPU.
    pub materials_resolved: usize,
    pub materials_missing: usize,
    pub textures: usize,
    pub textures_missing: usize,
    pub bcn_passthrough: usize,
    pub cpu_decoded: usize,
    pub texture_bytes: u64,
    pub vertex_blend: usize,
    pub translucent: usize,
    pub alpha_tested: usize,
}

/// Build a Bevy mesh from one material's worth of BSP geometry.
///
/// Positions and normals are converted to Bevy space; UVs pass through
/// unchanged. `UV_1` carries the per-face lightmap coordinate, which M5's
/// atlas packer will rescale in place.
pub fn surface_to_mesh(surface: &Surface) -> Mesh {
    let count = surface.vertices.len();
    let mut positions = Vec::with_capacity(count);
    let mut normals = Vec::with_capacity(count);
    let mut uvs = Vec::with_capacity(count);
    let mut lightmap_uvs = Vec::with_capacity(count);
    let mut colors = Vec::with_capacity(count);

    for v in &surface.vertices {
        positions.push(src_to_bevy(v.position).to_array());
        normals.push(src_to_bevy_dir(v.normal).to_array());
        uvs.push(v.uv);
        lightmap_uvs.push(v.lightmap_uv);
        // The `$basetexture2` blend weight rides vertex **alpha**, with RGB
        // left at 1: the PBR path multiplies `base_color` by the vertex colour,
        // so anything but 1 there would tint every surface. Brush faces leave
        // alpha at 0, which selects `$basetexture` alone.
        colors.push([1.0, 1.0, 1.0, v.alpha]);
    }

    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, lightmap_uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(surface.indices.clone()))
}

/// A stable, readable colour per material, until real textures arrive in M8.
///
/// Hashing the name rather than the index keeps a material the same colour
/// across maps, and stepping hue by the golden ratio keeps neighbouring
/// materials visually distinct instead of adjacent on the wheel.
pub fn debug_material_color(material: &str) -> Color {
    // FNV-1a: tiny, stable, and good enough to scatter hues.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in material.as_bytes() {
        hash ^= u64::from(byte.to_ascii_uppercase());
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // The hash is already uniform, so take hue straight from it — stepping by
    // the golden ratio only helps when walking a *sequence*, and here it would
    // squeeze every hue into 0..222 degrees. Jitter saturation and lightness
    // from independent bits so two materials that land on a similar hue still
    // separate visually.
    let hue = (hash % 360) as f32;
    let saturation = 0.35 + ((hash >> 11) % 41) as f32 / 100.0;
    let lightness = 0.45 + ((hash >> 23) % 31) as f32 / 100.0;
    Color::hsl(hue, saturation, lightness)
}

/// The baked lighting to attach to spawned surfaces.
///
/// `uv_rect` is deliberately the whole texture: [`LightmapAtlas::remap`] has
/// already rewritten `UV_1` into atlas space, and Bevy's own per-entity rect is
/// stored as two `unorm16` pairs, so letting it do the mapping would quantise
/// every rect to 1/65535 of the atlas.
#[derive(Clone)]
pub struct MapLightmap {
    pub image: Handle<Image>,
    pub exposure: f32,
}

impl MapLightmap {
    fn component(&self) -> Lightmap {
        Lightmap {
            image: self.image.clone(),
            uv_rect: Rect::new(0.0, 0.0, 1.0, 1.0),
            bicubic_sampling: false,
        }
    }
}

/// What a map contributes to every surface it spawns: the resolved materials,
/// their names, and the bake to attach.
///
/// These always travel together — a lightmap is packed for exactly the faces of
/// the geometry being spawned — so they are one parameter rather than three.
#[derive(Clone, Copy)]
pub struct MapSurfaces<'a> {
    /// Indexed by TEXDATA index; see [`bsp::Bsp::texture_names`].
    pub material_names: &'a [&'a str],
    /// One material per TEXDATA index, from [`load_materials`].
    pub materials: &'a MapMaterials,
    /// `None` leaves surfaces unlit by the bake.
    pub lightmap: Option<&'a MapLightmap>,
}

impl MapSurfaces<'_> {
    fn name(&self, texdata: i32) -> &str {
        usize::try_from(texdata)
            .ok()
            .and_then(|i| self.material_names.get(i).copied())
            .unwrap_or("<unknown>")
    }
}

/// Spawn one built model as a set of per-material mesh entities.
///
/// `origin` is the entity origin for a brush model; worldspawn passes zero.
/// Returns the number of entities spawned — one per material, which is the
/// draw call count for this model.
pub fn spawn_model(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    geometry: &ModelGeometry,
    surfaces: MapSurfaces,
    origin: [f32; 3],
    parent_name: &str,
) -> usize {
    let translation = src_to_bevy(origin);

    let lightmap = surfaces.lightmap;
    for surface in &geometry.surfaces {
        let name = surfaces.name(surface.texdata);
        // Materials are resolved once per map, not once per surface: a texdata
        // shared by twenty surfaces is one material and one draw call's worth
        // of state. Backface culling stays on per material unless `$nocull`
        // asks otherwise — it is the only cheap check that winding and normals
        // agree with the BSP.
        let Some(material) = surfaces.materials.get(surface.texdata) else {
            continue;
        };

        let mut entity = commands.spawn((
            Mesh3d(meshes.add(surface_to_mesh(surface))),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(translation),
            MapGeometry,
            SurfaceInfo {
                model: geometry.model,
                texdata: surface.texdata,
                material: name.to_string(),
                flags: surface.flags,
                triangles: surface.triangle_count(),
            },
            Name::new(format!("{parent_name}:{name}")),
        ));
        if let Some(lightmap) = lightmap {
            entity.insert(lightmap.component());
        }
    }

    geometry.surfaces.len()
}

/// Spawn tessellated terrain, one entity per material.
///
/// Vertex alpha is written to `ATTRIBUTE_COLOR` so the `WorldVertexTransition`
/// blend in M8 can read it; until then it is visible as a subtle tint, which
/// makes blend zones legible rather than invisible.
pub fn spawn_displacements(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    geometry: &DispGeometry,
    surfaces: MapSurfaces,
) -> usize {
    let lightmap = surfaces.lightmap;
    for surface in &geometry.surfaces {
        let name = surfaces.name(surface.texdata);
        let Some(material) = surfaces.materials.get(surface.texdata) else {
            continue;
        };

        let mut entity = commands.spawn((
            Mesh3d(meshes.add(surface_to_mesh(surface))),
            MeshMaterial3d(material.clone()),
            Transform::IDENTITY,
            DisplacementGeometry,
            SurfaceInfo {
                model: 0,
                texdata: surface.texdata,
                material: name.to_string(),
                flags: surface.flags,
                triangles: surface.triangle_count(),
            },
            Name::new(format!("displacement:{name}")),
        ));
        if let Some(lightmap) = lightmap {
            entity.insert(lightmap.component());
        }
    }
    geometry.surfaces.len()
}

/// Build [`MapInfo`] from a model's build stats and the terrain build.
pub fn map_info(
    name: &str,
    bsp_version: i32,
    geometry: &ModelGeometry,
    displacements: &DispGeometry,
    atlas: Option<&LightmapAtlas>,
) -> MapInfo {
    MapInfo {
        name: name.to_string(),
        bsp_version,
        materials: geometry.surfaces.len(),
        vertices: geometry.vertex_count(),
        triangles: geometry.triangle_count(),
        faces_built: geometry.stats.faces_built,
        faces_total: geometry.stats.faces_total,
        displacement_faces: geometry.stats.skipped.displacement,
        displacements_built: displacements.stats.built,
        displacement_vertices: displacements.vertex_count(),
        displacement_triangles: displacements.triangle_count(),
        mins: geometry.mins,
        maxs: geometry.maxs,
        lightmap_size: atlas.map_or((0, 0), |a| (a.width, a.height)),
        lightmap_faces: atlas.map_or(0, |a| a.stats.faces_packed),
        lightmap_unlit: atlas.map_or(0, |a| a.stats.faces_unlit),
        lightmap_occupancy: atlas.map_or(0.0, |a| a.stats.occupancy(a.pixel_count())),
        lightmap_peak: atlas.map_or(0.0, |a| a.stats.peak),
        // Filled in by the caller: brush entities are placed from the ENTITIES
        // lump, which this crate does not read, and materials are loaded
        // separately so a map can be built without a VFS at all.
        brush_entities: 0,
        brush_entity_triangles: 0,
        materials_resolved: 0,
        materials_missing: 0,
        textures: 0,
        textures_missing: 0,
        bcn_passthrough: 0,
        cpu_decoded: 0,
        texture_bytes: 0,
        vertex_blend: 0,
        translucent: 0,
        alpha_tested: 0,
    }
}

impl MapInfo {
    /// Fold in what [`load_materials`] reported.
    pub fn with_materials(mut self, stats: &MaterialStats) -> MapInfo {
        self.materials_resolved = stats.resolved;
        self.materials_missing = stats.missing.len();
        self.textures = stats.textures;
        self.textures_missing = stats.missing_textures.len();
        self.bcn_passthrough = stats.bcn_passthrough;
        self.cpu_decoded = stats.cpu_decoded;
        self.texture_bytes = stats.texture_bytes;
        self.vertex_blend = stats.vertex_blend;
        self.translucent = stats.translucent;
        self.alpha_tested = stats.alpha_tested;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_to_bevy_is_a_rotation_not_a_reflection() {
        // Source +X stays right, +Z becomes up, +Y becomes into the screen.
        assert_eq!(src_to_bevy_dir([1.0, 0.0, 0.0]), Vec3::X);
        assert_eq!(src_to_bevy_dir([0.0, 0.0, 1.0]), Vec3::Y);
        assert_eq!(src_to_bevy_dir([0.0, 1.0, 0.0]), -Vec3::Z);

        // The determinant must be +1: a reflection here would silently invert
        // every face's winding and require flipping indices to compensate.
        let m = Mat3::from_cols(
            src_to_bevy_dir([1.0, 0.0, 0.0]),
            src_to_bevy_dir([0.0, 1.0, 0.0]),
            src_to_bevy_dir([0.0, 0.0, 1.0]),
        );
        assert!((m.determinant() - 1.0).abs() < 1e-6, "det = {}", m.determinant());
    }

    #[test]
    fn direction_conversion_preserves_length() {
        let n = src_to_bevy_dir([0.577_35, 0.577_35, 0.577_35]);
        assert!((n.length() - 1.0).abs() < 1e-5, "len = {}", n.length());
    }

    #[test]
    fn bevy_to_src_round_trips() {
        for v in [
            [0.0, 0.0, 0.0],
            [1.0, 2.0, 3.0],
            [-14924.0, -4992.0, 1792.0],
        ] {
            let back = bevy_to_src(src_to_bevy(v));
            for i in 0..3 {
                assert!(
                    (back[i] - v[i]).abs() < 0.01,
                    "{v:?} -> {back:?} differs at {i}"
                );
            }
        }
    }

    #[test]
    fn debug_colors_are_stable_and_case_insensitive() {
        let a = debug_material_color("CONCRETE/CONCRETEWALL011");
        assert_eq!(a, debug_material_color("CONCRETE/CONCRETEWALL011"));
        // BSP material names vary in case between maps; the colour should not.
        assert_eq!(a, debug_material_color("concrete/concretewall011"));
        assert_ne!(a, debug_material_color("WOOD/WOOD_BEAM03"));
    }

    #[test]
    fn debug_colors_use_the_whole_hue_circle() {
        // Regression: an earlier version multiplied an already-uniform hash by
        // the golden ratio, compressing every hue into 0..222 degrees.
        let names: Vec<String> = (0..400).map(|i| format!("MAT/SURFACE{i:03}")).collect();
        let mut seen_low = false;
        let mut seen_high = false;
        for name in &names {
            let hsl = Hsla::from(debug_material_color(name));
            assert!((0.0..=360.0).contains(&hsl.hue), "hue {} out of range", hsl.hue);
            seen_low |= hsl.hue < 60.0;
            seen_high |= hsl.hue > 300.0;
        }
        assert!(seen_low && seen_high, "hues do not span the circle");
    }
}
