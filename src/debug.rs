//! The F2-F5 debug views.
//!
//! Wireframe, hiding brush geometry or terrain, and swapping textured albedo
//! for a flat colour per material. Each one isolates a layer of the render so
//! a fault can be attributed to it: the palette view is how you tell a
//! lighting problem from a texture problem.
use crate::cli::Args;
use crate::hud::HudText;
use bevy::pbr::wireframe::WireframeConfig;
use bevy::prelude::*;
use bevy_bsp::{DisplacementGeometry, MapGeometry, SourceMaterial, SurfaceInfo};

/// Visibility of brush geometry and of terrain, as two provably disjoint
/// queries.
///
/// Each filter excludes the *other* marker as well as the HUD. Without that,
/// Bevy cannot prove two `&mut Visibility` queries do not overlap — an entity
/// could in principle carry both markers — and panics with B0001.
pub type BrushVisibility<'w, 's> = Query<
    'w,
    's,
    &'static mut Visibility,
    (
        With<MapGeometry>,
        Without<DisplacementGeometry>,
        Without<HudText>,
    ),
>;

pub type TerrainVisibility<'w, 's> = Query<
    'w,
    's,
    &'static mut Visibility,
    (
        With<DisplacementGeometry>,
        Without<MapGeometry>,
        Without<HudText>,
    ),
>;

pub fn toggle_debug_views(
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
pub enum AlbedoMode {
    #[default]
    Textured,
    DebugPalette,
}

/// The `$basetexture` each material had before the palette was switched on, so
/// it can be put back.
#[derive(Resource, Default)]
pub struct SavedTextures(pub std::collections::HashMap<AssetId<SourceMaterial>, Option<Handle<Image>>>);

/// Swap between textured and flat-colour albedo on F5.
///
/// Editing the material assets in place rather than swapping handles keeps one
/// material per texdata, so the draw call count in the HUD stays honest.
pub fn swap_albedo(
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
