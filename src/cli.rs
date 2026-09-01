//! Command-line arguments, and resolving `--map` to a file on disk.
use bevy::prelude::{Resource, Vec2, Vec3};
use clap::Parser;
use std::path::{Path, PathBuf};

/// Where TF2 lives if `--game-dir` and the environment say nothing.
pub const DEFAULT_GAME_DIR: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\tf";
pub const GAME_DIR_ENV: &str = "BEVY_TF2_GAME_DIR";

#[derive(Parser, Debug, Resource, Clone)]
#[command(about = "View Team Fortress 2 BSP maps in Bevy")]
pub struct Args {
    /// Map name (e.g. `cp_badlands`) or a path to a .bsp file.
    #[arg(short, long, default_value = "ctf_2fort")]
    pub map: String,

    /// TF2 `tf/` directory. Defaults to $BEVY_TF2_GAME_DIR, then the usual
    /// Steam location.
    #[arg(short, long)]
    pub game_dir: Option<PathBuf>,

    /// Start with the mouse already captured.
    #[arg(long)]
    pub grab: bool,

    /// Uncap the frame rate. Bevy defaults to `PresentMode::Fifo` (strict
    /// vsync), which pins the frame rate to the display refresh and hides how
    /// expensive a frame really is. Pass this to measure actual headroom.
    #[arg(long)]
    pub no_vsync: bool,

    /// Render a few frames, save a PNG here, and exit. Used to verify
    /// rendering without a human at the keyboard, and to diff against the
    /// real game.
    #[arg(long)]
    pub screenshot: Option<PathBuf>,

    /// Camera position in Source units, as `x,y,z`. Matches `cl_showpos`, so a
    /// screenshot can be framed identically to one from TF2.
    /// Negative coordinates are common, so hyphens are values here, not flags.
    #[arg(long, value_parser = parse_source_vec, allow_hyphen_values = true)]
    pub pos: Option<Vec3>,

    /// Camera yaw and pitch in degrees, as `yaw,pitch`.
    #[arg(long, value_parser = parse_angles, allow_hyphen_values = true)]
    pub angles: Option<Vec2>,

    /// Skip the baked lightmaps and light the map with a debug directional
    /// light instead — the M3/M4 look, useful for judging geometry alone.
    #[arg(long)]
    pub no_lightmap: bool,

    /// Do not draw the 2D skybox.
    #[arg(long)]
    pub no_sky: bool,

    /// Skip `$basetexture` and paint each material a flat debug colour — the
    /// M5 look, for telling a lighting problem from a texture problem.
    #[arg(long)]
    pub no_textures: bool,

    /// Override the lightmap brightness. Defaults to the value that maps a
    /// luxel of 1.0 onto full exposure; see `bevy_bsp::lightmap_exposure`.
    #[arg(long)]
    pub lightmap_exposure: Option<f32>,
}

/// Parse `x,y,z` in Source units into a Bevy-space position.
pub fn parse_source_vec(text: &str) -> Result<Vec3, String> {
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

pub fn parse_angles(text: &str) -> Result<Vec2, String> {
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() != 2 {
        return Err("expected `yaw,pitch` in degrees".into());
    }
    let yaw: f32 = parts[0].trim().parse().map_err(|_| "bad yaw".to_string())?;
    let pitch: f32 = parts[1].trim().parse().map_err(|_| "bad pitch".to_string())?;
    Ok(Vec2::new(yaw.to_radians(), pitch.to_radians()))
}

impl Args {
    pub fn game_dir(&self) -> PathBuf {
        self.game_dir
            .clone()
            .or_else(|| std::env::var_os(GAME_DIR_ENV).map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_GAME_DIR))
    }
}

/// Resolve `--map` to a file: an explicit path wins, otherwise look under
/// `<game-dir>/maps/`.
pub fn resolve_map(map: &str, game_dir: &Path) -> Result<PathBuf, String> {
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
