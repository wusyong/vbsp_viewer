//! Source-engine BSP maps as Bevy scenes.
//!
//! Reads a Team Fortress 2 `.bsp` straight from a Steam install — brush
//! geometry, displacements, VPK textures, baked lightmaps, the 2D sky cubemap
//! and the 3D skybox — with no export step in between.
//!
//! ```ignore
//! App::new()
//!     .add_plugins(DefaultPlugins)
//!     .add_plugins(bevy_vbsp::bsp::BspPlugin)
//!     .run();
//! ```
//!
//! # Shape of the crate
//!
//! [`bsp::geometry`] and [`bsp::lightmap`] are pure conversions — a parsed
//! `vbsp::Bsp` in, meshes and an atlas out — and touch no Bevy world state, so
//! they can be timed and tested on their own. [`bsp`] itself is the plugin: it
//! reads the map, builds those, and spawns the result. [`vfs`], [`vmt`], [`vtf`]
//! and [`sky`] are the Source-format side and are useful independently.
//!
//! The viewer that was used to build all of this lives in `main.rs` beside this
//! file — a fly camera, an A/B toggle against a reference glTF export, and a HUD
//! of the counters that prove the conversion was faithful.
//!
//! # What this does not do yet
//!
//! Loading is synchronous, in a `Startup` system, and the map it opens is chosen
//! by `TF2_BSP` rather than by the caller — see `bsp::bsp_path`. There are no
//! static props (Phase 2, and wants `vmdl`), no water beyond a flat translucent
//! stand-in, no PVS culling, and the 3D skybox does not parallax. The crate's
//! README has the current list.
//!
//! # Assets
//!
//! Nothing Valve ships is redistributed here. Maps, textures and sounds are read
//! from the player's own installation at runtime, located by `tf-asset-loader`.

pub mod bsp;
pub mod sky;
pub mod vfs;
pub mod vmt;
pub mod vtf;

pub use bsp::{
    BatchMaterials, BspGeometry, BspPlugin, BspReport, DEFAULT_LIGHTMAP_EXPOSURE, HAMMER_UNIT,
    LightmapStats, MaterialStats, REFERENCE_YAW, SkyBoxRoot, Stats, bsp_path, lightmap_exposure,
    set_culling,
};
pub use sky::SkyReport;
pub use vfs::Vfs;
