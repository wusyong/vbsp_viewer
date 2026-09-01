//! Inspect and verify the 2D skybox.
//!
//! ```text
//! skydump <skyname>                  # faces, sizes, formats, per-edge report
//! skydump --map <mapname>            # whichever sky that map names
//! skydump <skyname> --bmp <dir>      # each face at native size, named by slot
//! skydump --caps                     # measure the up/dn cap rotations
//! skydump --all-maps                 # the M9 acceptance gate, all 233 maps
//! skydump --all-maps --limit 10      # the same gate, first 10 maps
//! ```
//!
//! # The three gates
//!
//! `--all-maps` runs all three, because each catches a class the others cannot:
//!
//! 1. **Coverage** — every `skyname` across the 233 maps resolves six faces,
//!    with each map's own pakfile mounted. Most skies are *only* reachable that
//!    way.
//! 2. **Side continuity** — the four side faces are one continuous band, so
//!    sampling either side of each vertical join must agree. This is checked on
//!    real pixels against the axes in [`bevy_bsp::sky::SkyFace::axes`], not
//!    searched for: as a search it is worthless on TF2's smooth gradients, which
//!    is what sank the previous attempt.
//! 3. **Sun bearing** — `light_environment` says which way the sun is; the sky
//!    should be brightest in that direction. This is the only gate that checks
//!    the arrangement against *map data* rather than against the sky's own
//!    self-consistency, so it is the only one a uniformly wrong mapping cannot
//!    satisfy. That failure mode is exactly what made earlier iterations look
//!    plausible while being wrong, and it is not hypothetical: turning the whole
//!    sky 90 degrees leaves every side join scoring 0.81 — completely invisible
//!    to gate 2 — while gate 3 reports 99 to 162 degrees of error.
//!
//! BMP rather than PNG to stay dependency-free, as with `lmdump` and `vtfdump`.

use bevy_bsp::sky::{
    self, azimuth_profile, box_edges, cap_rotations, cap_scores, edge_report, edge_score,
    sun_direction, yaw_of, Sky, SkyFace,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use vfs::Vfs;

type Failure = Box<dyn std::error::Error>;

const DEFAULT_GAME_DIR: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\tf";

/// Worst acceptable *spread* on a side-to-side join — `EdgeScore::spread`, the
/// 20th percentile of the difference along the edge, not the mean.
///
/// Sides are authored as one continuous band, so a correct arrangement scores
/// near zero along essentially the whole join. Reading a low percentile rather
/// than the mean is what lets this be a tight limit: a mirrored face is wrong at
/// every point along the edge and blows straight through it, while a sky that
/// ships one face with a blacked-out lower half — `sky_stranded_01` does — is
/// still exact over the rest and passes. Widening the limit to accommodate that
/// sky would have hidden real errors of the same magnitude.
const SIDE_JOIN_LIMIT: f32 = 8.0;

/// Worst acceptable *coherent* bias, in degrees, across every sky checked.
///
/// # Why this is a fleet statistic and not a per-map pass
///
/// The first version failed any map whose sun was more than 60 degrees from the
/// sky's brightest bearing. That is not a sound test, and the data says so
/// plainly: `arena_nucleus` and `arena_offblast_final` use the **same**
/// `sky_goldrush_01` artwork and report 72 and 40 degrees. Identical pixels,
/// different answers — so the disagreement is in the maps, not the sky. Mappers
/// set `light_environment` by eye, against artwork they did not paint, and on
/// night maps it is a dim fill light pointed wherever.
///
/// What a *wrong arrangement* produces is different in kind: a **coherent** bias.
/// Turn the whole sky 90 degrees and every map's signed error moves by the same
/// 90 degrees at once. Mapper sloppiness is incoherent and cancels; a rotation
/// does not. So the statistic is the circular mean of the signed errors, which
/// stays near zero however sloppy the individual maps are, and lands on the
/// rotation angle the moment the arrangement is wrong.
const SUN_BIAS_LIMIT: f32 = 30.0;

/// How far the brightest compass bin must stand above the mean for the sky to
/// have a findable sun at all.
///
/// Below this the sky has no preferred direction — overcast, or night — and its
/// peak bin is noise. Such skies are counted and reported, never silently passed:
/// a gate that quietly excuses whatever it cannot measure is the same trap as a
/// check that cannot fail.
const SUN_PROMINENCE_FLOOR: f32 = 0.08;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool, Failure> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut game_dir = std::env::var_os("BEVY_TF2_GAME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_GAME_DIR));
    let mut skyname: Option<String> = None;
    let mut bmp: Option<PathBuf> = None;
    let mut mode = Mode::One;
    let mut limit = usize::MAX;
    let mut map: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--game-dir" => {
                i += 1;
                game_dir = PathBuf::from(&args[i]);
            }
            "--bmp" => {
                i += 1;
                bmp = Some(PathBuf::from(&args[i]));
            }
            "--map" => {
                i += 1;
                map = Some(args[i].clone());
            }
            "--limit" => {
                i += 1;
                limit = args[i].parse()?;
            }
            "--caps" => mode = Mode::Caps,
            "--all-maps" => mode = Mode::AllMaps,
            other if other.starts_with("--") => {
                return Err(format!("unknown option {other}").into());
            }
            other => skyname = Some(other.to_string()),
        }
        i += 1;
    }

    match mode {
        Mode::One => one(&game_dir, skyname.as_deref(), map.as_deref(), bmp.as_deref()),
        Mode::Caps => caps(&game_dir),
        Mode::AllMaps => all_maps(&game_dir, limit),
    }
}

enum Mode {
    One,
    Caps,
    AllMaps,
}

// ---------------------------------------------------------------------------
// One sky
// ---------------------------------------------------------------------------

fn one(
    game_dir: &Path,
    skyname: Option<&str>,
    map: Option<&str>,
    bmp: Option<&Path>,
) -> Result<bool, Failure> {
    // Most skies live only in a map's own pakfile — 46 of the 70, measured — so
    // `--map` is the usual way in, and a bare skyname only reaches the 24 that
    // are in the VPKs.
    let mut vfs = Vfs::from_game_dir(game_dir)?;
    let mut skyname = skyname.map(str::to_string);
    if let Some(map) = map {
        let path = game_dir.join("maps").join(format!("{map}.bsp"));
        let bsp = bsp::Bsp::open(&path)?;
        vfs.mount_pakfile(bsp.lump_bytes(bsp::LumpId::PakFile)?.to_vec())?;
        let entities = vfs::entities::parse(bsp.entities_str()?)?;
        let named = vfs::entities::worldspawn(&entities)
            .and_then(|w| w.get("skyname"))
            .map(str::to_string);
        println!("{map}: skyname {}", named.as_deref().unwrap_or("(none)"));
        skyname = skyname.or(named);
    }
    let Some(skyname) = skyname else {
        eprintln!("usage: skydump <skyname> | --map <mapname> [--bmp <dir>] | --caps | --all-maps");
        return Ok(false);
    };
    let skyname = skyname.as_str();
    let sky = sky::load(&vfs, skyname)?;

    println!("{skyname}\n");
    println!("{:<5} {:>11}  source", "face", "size");
    for face in SkyFace::ALL {
        let tex = sky.face(face);
        println!(
            "{:<5} {:>11}  {}",
            face.suffix(),
            format!("{}x{}", tex.width, tex.height),
            tex.source.as_deref().unwrap_or("($color, no texture)"),
        );
    }

    // Both figures, because their disagreement is the interesting part: a big
    // mean with a small spread is a defect in one patch of the artwork, and a
    // big spread is a face in the wrong place.
    println!("\nper-edge difference (0-255):");
    println!("  {:<6} {:>8} {:>8}", "edge", "mean", "spread");
    let report = edge_report(&sky, SkyFace::axes);
    let mut worst_side = 0.0f32;
    for score in &report {
        let side_join = !score.a.is_cap() && !score.b.is_cap();
        if side_join {
            worst_side = worst_side.max(score.spread());
        }
        println!(
            "  {:<2}-{:<2}  {:>8.2} {:>8.2}{}",
            score.a.suffix(),
            score.b.suffix(),
            score.mean_diff,
            score.spread(),
            if side_join { "   (side join)" } else { "" },
        );
    }
    println!("\nworst side-join spread {worst_side:.2}, limit {SIDE_JOIN_LIMIT:.2}");

    println!("\ncap rotations (mean difference against the four sides):");
    for cap in [SkyFace::Up, SkyFace::Dn] {
        let scores = cap_scores(&sky, cap);
        let best = argmin(&scores);
        print!("  {:<2}", cap.suffix());
        for (i, s) in scores.iter().enumerate() {
            print!("  rot{i} {s:>7.2}{}", if i == best { "*" } else { " " });
        }
        println!("   using rot {}", in_use(cap));
    }

    if let Some(dir) = bmp {
        std::fs::create_dir_all(dir)?;
        for face in SkyFace::ALL {
            let tex = sky.face(face);
            let path = dir.join(format!("{skyname}_{}.bmp", face.suffix()));
            write_bmp(&tex.rgba, tex.width, tex.height, &path)?;
            println!("{} -> {}", face.suffix(), path.display());
        }
    }

    Ok(worst_side <= SIDE_JOIN_LIMIT)
}

/// Which rotation the shipped table uses, found by comparing against
/// [`cap_rotations`] rather than by reading the constant — so the printout
/// cannot disagree with what the renderer actually does.
fn in_use(cap: SkyFace) -> usize {
    let axes = cap.axes();
    cap_rotations(cap)
        .iter()
        .position(|c| *c == axes)
        .expect("a cap's axes must be one of its four rotations")
}

// ---------------------------------------------------------------------------
// Cap measurement
// ---------------------------------------------------------------------------

/// Measure the `up` and `dn` rotations over every sky the game ships.
///
/// # Why most skies have to be discarded
///
/// A cap with no detail scores the same in all four orientations, so it votes
/// for whichever rotation the tie-break happens to pick. Averaging those in
/// would drown the few skies that carry a real signal. A sky only votes when its
/// best rotation beats the runner-up by [`CAP_MARGIN`].
fn caps(game_dir: &Path) -> Result<bool, Failure> {
    let skies = distinct_skynames(game_dir)?;
    println!("{} distinct skynames\n", skies.len());

    for cap in [SkyFace::Up, SkyFace::Dn] {
        println!("=== {} ===", cap.suffix());
        println!(
            "{:<26} {:>8} {:>8} {:>8} {:>8}  verdict",
            "sky", "rot0", "rot1", "rot2", "rot3"
        );

        let mut votes = [0usize; 4];
        let mut decisive = 0usize;
        for (skyname, map) in &skies {
            let Some(sky) = load_with_pakfile(game_dir, map, skyname) else {
                continue;
            };
            let scores = cap_scores(&sky, cap);
            let best = argmin(&scores);
            let runner_up = scores
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != best)
                .map(|(_, s)| *s)
                .fold(f32::MAX, f32::min);
            // A flat cap scores identically everywhere; only a real margin
            // counts as evidence.
            let verdict = if scores[best] > 0.0 && runner_up > scores[best] * CAP_MARGIN {
                votes[best] += 1;
                decisive += 1;
                format!("rot {best}")
            } else {
                "flat, no vote".to_string()
            };
            println!(
                "{:<26} {:>8.2} {:>8.2} {:>8.2} {:>8.2}  {verdict}",
                skyname, scores[0], scores[1], scores[2], scores[3],
            );
        }

        println!("\n{decisive} skies voted: {votes:?}");
        let winner = votes
            .iter()
            .enumerate()
            .max_by_key(|(_, v)| **v)
            .map(|(i, _)| i)
            .unwrap_or(0);
        if decisive == 0 {
            println!(
                "no sky carries enough detail in its {} face — the rotation is a \
                 convention, not a measurement",
                cap.suffix()
            );
        } else {
            println!(
                "winner rot {winner} with {}/{decisive}; the table uses rot {}",
                votes[winner],
                in_use(cap),
            );
        }
        println!();
    }
    Ok(true)
}

/// How much better the best cap rotation must be than the runner-up to count.
const CAP_MARGIN: f32 = 2.0;

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

fn all_maps(game_dir: &Path, limit: usize) -> Result<bool, Failure> {
    let mut maps = map_paths(game_dir)?;
    maps.truncate(limit);
    println!(
        "{:<28} {:<24} {:>7} {:>8}",
        "map", "sky", "join", "sun deg"
    );

    let mut resolved = 0usize;
    let mut unresolved: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut join_failures = Vec::new();
    let mut sun_errors: Vec<f32> = Vec::new();
    let mut sun_flat = 0usize;
    let mut no_sun_entity = 0usize;
    let mut worst_join = 0.0f32;
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for path in &maps {
        let name = stem(path);
        let bsp = bsp::Bsp::open(path)?;
        let entities = vfs::entities::parse(bsp.entities_str()?)?;
        let Some(skyname) = vfs::entities::worldspawn(&entities)
            .and_then(|w| w.get("skyname"))
            .map(str::to_string)
        else {
            println!("{name:<28} {:<24} {:>7} {:>8}   NO SKYNAME", "-", "-", "-");
            continue;
        };

        let mut vfs = Vfs::from_game_dir(game_dir)?;
        vfs.mount_pakfile(bsp.lump_bytes(bsp::LumpId::PakFile)?.to_vec())?;
        let sky = match sky::load(&vfs, &skyname) {
            Ok(sky) => sky,
            Err(e) => {
                unresolved
                    .entry(skyname.clone())
                    .or_default()
                    .push(format!("{name}: {e}"));
                println!("{name:<28} {skyname:<24} {:>7} {:>8}   UNRESOLVED", "-", "-");
                continue;
            }
        };
        resolved += 1;

        // Gate 2: the four sides are one continuous band.
        let join = worst_side_join(&sky);
        worst_join = worst_join.max(join);
        let join_bad = join > SIDE_JOIN_LIMIT;
        if join_bad && seen.insert(format!("join:{skyname}")) {
            join_failures.push((skyname.clone(), join));
        }

        // Gate 3: the sky is brightest toward the sun the map declares. One
        // signed error per map; the verdict is on their coherence, not on any
        // single map. See `SUN_BIAS_LIMIT`.
        let mut sun_text = "-".to_string();
        match sun_from_entities(&entities) {
            None => no_sun_entity += 1,
            Some(expected) => {
                let profile = azimuth_profile(&sky);
                if profile.prominence() < SUN_PROMINENCE_FLOOR {
                    sun_flat += 1;
                    sun_text = "flat".to_string();
                } else {
                    let signed = wrap180(profile.peak_yaw() - yaw_of(expected));
                    sun_errors.push(signed);
                    sun_text = format!("{signed:+.0}");
                }
            }
        }

        println!(
            "{name:<28} {skyname:<24} {join:>7.2} {sun_text:>8}{}",
            if join_bad { "   FAIL join" } else { "" },
        );
    }

    println!("\n--- gate 1: coverage ---");
    println!("{}/{} maps resolved a sky", resolved, maps.len());
    if unresolved.is_empty() {
        println!("every skyname resolved");
    } else {
        for (skyname, why) in &unresolved {
            println!("  {skyname}: {} map(s)", why.len());
            println!("    {}", why[0]);
        }
    }

    println!("\n--- gate 2: side continuity ---");
    println!("worst side-join spread {worst_join:.2}, limit {SIDE_JOIN_LIMIT:.2}");
    for (skyname, join) in &join_failures {
        println!("  {skyname}: {join:.2}");
    }

    println!("\n--- gate 3: sun bearing ---");
    println!(
        "{} maps checked, {sun_flat} with no findable sun, \
         {no_sun_entity} with no light_environment",
        sun_errors.len(),
    );
    let (bias, concentration) = circular_mean(&sun_errors);
    println!(
        "coherent bias {bias:+.1} deg (limit +-{SUN_BIAS_LIMIT:.0}), \
         concentration {concentration:.2}"
    );
    print_error_histogram(&sun_errors);

    // Coverage failures are reported, not fatal: `sky_black_01` is absent from
    // the game entirely, exactly like the 20 missing materials M6 found, and the
    // engine draws its checkerboard rather than refusing the map.
    let sun_ok = sun_errors.is_empty() || bias.abs() <= SUN_BIAS_LIMIT;
    let ok = join_failures.is_empty() && sun_ok;
    println!(
        "\n{}",
        if ok {
            "PASS"
        } else {
            "FAIL — see the gate sections above"
        }
    );
    Ok(ok)
}

/// The worst spread across the four side-to-side joins.
///
/// Only side pairs: a cap join scores against `up`'s measured rotation and
/// `dn`'s convention, so folding those in would let the one unverified face fail
/// the gate for every map.
fn worst_side_join(sky: &Sky) -> f32 {
    box_edges()
        .into_iter()
        .filter(|(a, b)| !a.is_cap() && !b.is_cap())
        .map(|(a, b)| edge_score(sky, a, a.axes(), b, b.axes()).spread())
        .fold(0.0, f32::max)
}

/// The first `light_environment`'s sun direction, in Source axes.
fn sun_from_entities(entities: &[vfs::Entity]) -> Option<[f32; 3]> {
    let e = entities
        .iter()
        .find(|e| e.get("classname") == Some("light_environment"))?;
    let pitch = e.get("pitch").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    let angle = e.get("angle").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    Some(sun_direction(e.angles(), pitch, angle))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn map_paths(game_dir: &Path) -> Result<Vec<PathBuf>, Failure> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(game_dir.join("maps"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("bsp"))
        .collect();
    paths.sort();
    Ok(paths)
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Every distinct `skyname`, with one map that names it — the map whose pakfile
/// has to be mounted to reach it.
fn distinct_skynames(game_dir: &Path) -> Result<Vec<(String, PathBuf)>, Failure> {
    let mut out: BTreeMap<String, PathBuf> = BTreeMap::new();
    for path in map_paths(game_dir)? {
        let Ok(bsp) = bsp::Bsp::open(&path) else {
            continue;
        };
        let Ok(text) = bsp.entities_str() else {
            continue;
        };
        let Ok(entities) = vfs::entities::parse(text) else {
            continue;
        };
        if let Some(name) = vfs::entities::worldspawn(&entities).and_then(|w| w.get("skyname")) {
            out.entry(name.to_string()).or_insert(path);
        }
    }
    Ok(out.into_iter().collect())
}

fn load_with_pakfile(game_dir: &Path, map: &Path, skyname: &str) -> Option<Sky> {
    let bsp = bsp::Bsp::open(map).ok()?;
    let mut vfs = Vfs::from_game_dir(game_dir).ok()?;
    vfs.mount_pakfile(bsp.lump_bytes(bsp::LumpId::PakFile).ok()?.to_vec())
        .ok()?;
    sky::load(&vfs, skyname).ok()
}

/// Signed angle in (-180, 180].
fn wrap180(degrees: f32) -> f32 {
    let d = degrees.rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// Circular mean of signed angles, in degrees, and the resultant length.
///
/// The resultant length is 0 when the errors point every which way and 1 when
/// they all agree, so it says how much the bias is worth believing. Averaging
/// angles arithmetically would be wrong here: -179 and +179 are two degrees
/// apart, not 358.
fn circular_mean(errors: &[f32]) -> (f32, f32) {
    if errors.is_empty() {
        return (0.0, 0.0);
    }
    let n = errors.len() as f32;
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    for e in errors {
        let r = e.to_radians();
        sx += r.cos();
        sy += r.sin();
    }
    let (sx, sy) = (sx / n, sy / n);
    (sy.atan2(sx).to_degrees(), (sx * sx + sy * sy).sqrt())
}

/// A coarse histogram of signed bearing errors.
///
/// Printed because the single bias figure hides the shape. A correct arrangement
/// gives a broad hump centred on zero — mapper noise. A rotated one gives a hump
/// centred on the rotation angle, which looks completely different even when a
/// handful of maps happen to land near zero either way.
fn print_error_histogram(errors: &[f32]) {
    if errors.is_empty() {
        return;
    }
    const EDGES: [f32; 6] = [15.0, 45.0, 75.0, 105.0, 135.0, 180.0];
    let mut buckets = [0usize; 11];
    for e in errors {
        let magnitude = e.abs();
        let band = EDGES.iter().position(|edge| magnitude <= *edge).unwrap_or(5);
        let index = if band == 0 {
            5
        } else if *e < 0.0 {
            5 - band
        } else {
            5 + band
        };
        buckets[index] += 1;
    }
    let labels = [
        "-180..-135", "-135..-105", "-105..-75", "-75..-45", "-45..-15",
        " -15..+15", " +15..+45", " +45..+75", " +75..+105", "+105..+135",
        "+135..+180",
    ];
    println!("  signed error distribution:");
    for (label, count) in labels.iter().zip(buckets) {
        if count > 0 {
            println!("    {label} {count:>4}  {}", "#".repeat(count.min(60)));
        }
    }
}

fn argmin(scores: &[f32; 4]) -> usize {
    scores
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn write_bmp(rgba: &[u8], width: u32, height: u32, path: &Path) -> std::io::Result<()> {
    let (w, h) = (width as usize, height as usize);
    let row_bytes = w * 3;
    let padding = (4 - row_bytes % 4) % 4;
    let pixel_bytes = (row_bytes + padding) * h;
    let mut out = Vec::with_capacity(54 + pixel_bytes);

    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + pixel_bytes) as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    for _ in 0..4 {
        out.extend_from_slice(&0u32.to_le_bytes());
    }

    for y in (0..h).rev() {
        for x in 0..w {
            let at = (y * w + x) * 4;
            out.push(rgba[at + 2]);
            out.push(rgba[at + 1]);
            out.push(rgba[at]);
        }
        out.extend(std::iter::repeat_n(0u8, padding));
    }
    std::fs::File::create(path)?.write_all(&out)
}
