//! Inspect the mounted search paths, and resolve a map's materials.
//!
//! ```text
//! vfsls --game-dir <tf> --mounts             # the search path stack, in order
//! vfsls --game-dir <tf> --ls <prefix>        # entries under a prefix, and their source
//! vfsls --game-dir <tf> --cat <path>         # print a text file (VMT, gameinfo)
//! vfsls --game-dir <tf> --map <map.bsp>      # resolve every material a map uses
//! vfsls --game-dir <tf> --all-maps           # ...for all 233 maps: the M6 gate
//! ```
//!
//! `--map` is the real test of this crate: it takes the material names from a
//! BSP's TEXDATA lump, resolves each through the search path stack (with the
//! map's own pakfile mounted on top), follows `Patch` chains, and reports every
//! `$basetexture` that does not exist. A correct VFS resolves essentially all
//! of them; a wrong search path order or a broken VPK tree shows up as
//! hundreds of misses rather than as a crash.
//!
//! **A missing `.vmt` is not necessarily a VFS bug.** A handful of community
//! maps reference materials that were never shipped *and* never packed — the
//! real game draws those as the pink checkerboard. The gate therefore fails on
//! a *broken* material (one that is present but cannot be read or parsed) and
//! on a map where nothing resolves at all, and reports absent materials
//! separately.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use vfs::{vmt, Vfs};

type Failure = Box<dyn std::error::Error>;

const DEFAULT_GAME_DIR: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\tf";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut game_dir = std::env::var_os("BEVY_TF2_GAME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_GAME_DIR));
    let mut action: Option<(String, String)> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--game-dir" if i + 1 < args.len() => {
                game_dir = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--mounts" | "--all-maps" => {
                action = Some((args[i].clone(), String::new()));
                i += 1;
            }
            flag @ ("--ls" | "--cat" | "--map") if i + 1 < args.len() => {
                action = Some((flag.to_string(), args[i + 1].clone()));
                i += 2;
            }
            other => {
                eprintln!("unexpected argument {other:?}");
                usage();
                return std::process::ExitCode::from(2);
            }
        }
    }

    let Some((action, value)) = action else {
        usage();
        return std::process::ExitCode::from(2);
    };

    let result = match action.as_str() {
        "--mounts" => mounts(&game_dir),
        "--ls" => list(&game_dir, &value),
        "--cat" => cat(&game_dir, &value),
        "--map" => one_map(&game_dir, Path::new(&value)),
        "--all-maps" => all_maps(&game_dir),
        _ => unreachable!(),
    };

    match result {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!("usage: vfsls [--game-dir <tf>] --mounts");
    eprintln!("       vfsls [--game-dir <tf>] --ls <prefix>");
    eprintln!("       vfsls [--game-dir <tf>] --cat <path>");
    eprintln!("       vfsls [--game-dir <tf>] --map <map.bsp>");
    eprintln!("       vfsls [--game-dir <tf>] --all-maps");
}

fn open(game_dir: &Path) -> Result<Vfs, Failure> {
    let started = std::time::Instant::now();
    let vfs = Vfs::from_game_dir(game_dir)?;
    eprintln!(
        "mounted {} search paths in {:.0} ms",
        vfs.mount_count(),
        started.elapsed().as_secs_f32() * 1000.0
    );
    Ok(vfs)
}

fn mounts(game_dir: &Path) -> Result<bool, Failure> {
    let vfs = open(game_dir)?;
    println!("search path stack, highest priority first:\n");
    for (i, description) in vfs.mount_descriptions().iter().enumerate() {
        println!("  {:>2}. {description}", i + 1);
    }
    if !vfs.missing.is_empty() {
        println!("\nlisted in gameinfo but not on disk (normal for tf2_lv.vpk):");
        for path in &vfs.missing {
            println!("  {}", path.display());
        }
    }
    Ok(!vfs.mount_descriptions().is_empty())
}

fn list(game_dir: &Path, prefix: &str) -> Result<bool, Failure> {
    let vfs = open(game_dir)?;
    let entries = vfs.list(prefix);
    for (path, source) in &entries {
        // Trim the install prefix so the source column stays readable.
        let source = Path::new(source)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| source.clone());
        println!("{path:<70} {source}");
    }
    println!("\n{} entries under {prefix:?}", entries.len());
    Ok(!entries.is_empty())
}

fn cat(game_dir: &Path, path: &str) -> Result<bool, Failure> {
    let vfs = open(game_dir)?;
    match vfs.source_of(path) {
        Some(source) => eprintln!("from {source}\n"),
        None => eprintln!("not found in any mount\n"),
    }
    print!("{}", vfs.read_to_string(path)?);
    Ok(true)
}

/// What resolving one map's materials produced.
#[derive(Default)]
struct MapReport {
    materials: usize,
    patched: usize,
    /// Material names with no `.vmt` anywhere in the search paths.
    missing_vmt: Vec<String>,
    /// `$basetexture` values whose `.vtf` is missing.
    missing_vtf: Vec<String>,
    /// Materials that parsed but have no `$basetexture` — normal for tool and
    /// sky shaders.
    no_texture: usize,
    /// VMTs that failed to parse, as opposed to being absent.
    broken: Vec<String>,
    shaders: BTreeMap<String, usize>,
    from_pakfile: usize,
    /// Entities in the lump, and how many of them draw a BSP model.
    entities: usize,
    brush_entities: usize,
    /// Set when the ENTITIES lump would not parse at all.
    entity_error: Option<String>,
}

fn resolve_map(game_dir: &Path, path: &Path) -> Result<MapReport, Failure> {
    let bsp = bsp::Bsp::open(path)?;
    let mut vfs = Vfs::from_game_dir(game_dir)?;
    // The map's own pakfile goes on top, exactly as the engine mounts it.
    vfs.mount_pakfile(bsp.lump_bytes(bsp::LumpId::PakFile)?.to_vec())?;

    let mut report = MapReport::default();
    let mut seen_textures: HashSet<String> = HashSet::new();

    // The ENTITIES lump is KeyValues too, and 233 real lumps are a far better
    // test of the parser than any fixture.
    match bsp.entities_str().map_err(Failure::from).and_then(|text| {
        Ok(vfs::entities::parse(text)?)
    }) {
        Ok(entities) => {
            report.entities = entities.len();
            report.brush_entities = vfs::entities::brush_entities(&entities).len();
        }
        Err(e) => report.entity_error = Some(e.to_string()),
    }

    for name in bsp.texture_names()? {
        report.materials += 1;
        let vmt_path = vmt::material_path(name);
        if !vfs.exists(&vmt_path) {
            report.missing_vmt.push(name.to_string());
            continue;
        }
        if vfs
            .source_of(&vmt_path)
            .is_some_and(|s| s.contains("pakfile"))
        {
            report.from_pakfile += 1;
        }

        match vmt::load_reporting_patch(&vfs, name) {
            Ok((material, was_patch)) => {
                if was_patch {
                    report.patched += 1;
                }
                // Shipped materials spell the same shader `LightmappedGeneric`
                // and `LightMappedGeneric`; group them.
                *report
                    .shaders
                    .entry(material.shader.to_ascii_lowercase())
                    .or_default() += 1;

                let textures = material.textures();
                if textures.is_empty() {
                    report.no_texture += 1;
                }
                for texture in textures {
                    if seen_textures.insert(texture.clone()) && !vfs.exists(&texture) {
                        report.missing_vtf.push(texture);
                    }
                }
            }
            Err(e) => report.broken.push(format!("{name}: {e}")),
        }
    }
    Ok(report)
}

fn one_map(game_dir: &Path, path: &Path) -> Result<bool, Failure> {
    let report = resolve_map(game_dir, path)?;
    println!("{}\n", path.display());
    println!("  entities         {}", report.entities);
    println!("  brush entities   {} (draw a BSP model)", report.brush_entities);
    println!("  materials        {}", report.materials);
    println!("  via Patch        {}", report.patched);
    println!("  from pakfile     {}", report.from_pakfile);
    println!("  no $basetexture  {} (tool and sky shaders)", report.no_texture);
    println!("  missing .vmt     {}", report.missing_vmt.len());
    println!("  missing .vtf     {}", report.missing_vtf.len());
    println!("  broken .vmt      {}", report.broken.len());

    println!("\n  shaders in use:");
    let mut shaders: Vec<_> = report.shaders.iter().collect();
    shaders.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (shader, count) in shaders {
        println!("    {count:>5}  {shader}");
    }

    for (label, list) in [
        ("missing .vmt", &report.missing_vmt),
        ("missing .vtf", &report.missing_vtf),
        ("broken", &report.broken),
    ] {
        if list.is_empty() {
            continue;
        }
        println!("\n  {label}:");
        for item in list.iter().take(20) {
            println!("    {item}");
        }
        if list.len() > 20 {
            println!("    ... and {} more", list.len() - 20);
        }
    }

    Ok(report.broken.is_empty() && report.missing_vmt.is_empty())
}

fn all_maps(game_dir: &Path) -> Result<bool, Failure> {
    let maps = game_dir.join("maps");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&maps)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("bsp"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("no .bsp files in {}", maps.display()).into());
    }

    println!(
        "{:<30} {:>6} {:>7} {:>5} {:>7} {:>7} {:>6} {:>6} {:>6}",
        "map", "mats", "patched", "pak", "no-vmt", "no-vtf", "broken", "ents", "brush"
    );

    let mut totals = MapReport::default();
    let mut failures = 0usize;
    let mut worst: Vec<(usize, String)> = Vec::new();

    for path in &paths {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        match resolve_map(game_dir, path) {
            Ok(report) => {
                // Absent materials are a property of the map; a broken one,
                // or a map where nothing at all resolves, is a bug here.
                let resolved = report.materials - report.missing_vmt.len();
                let bad = !report.broken.is_empty()
                    || report.entity_error.is_some()
                    || (report.materials > 0 && resolved == 0);
                println!(
                    "{name:<30} {:>6} {:>7} {:>5} {:>7} {:>7} {:>6} {:>6} {:>6}{}",
                    report.materials,
                    report.patched,
                    report.from_pakfile,
                    report.missing_vmt.len(),
                    report.missing_vtf.len(),
                    report.broken.len(),
                    report.entities,
                    report.brush_entities,
                    if bad { "  FAIL" } else { "" },
                );
                if let Some(e) = &report.entity_error {
                    println!("    ENTITIES lump: {e}");
                }
                if !report.missing_vmt.is_empty() || !report.broken.is_empty() {
                    worst.push((
                        report.missing_vmt.len() + report.broken.len(),
                        format!(
                            "{name}: {}",
                            report
                                .missing_vmt
                                .iter()
                                .chain(report.broken.iter())
                                .take(3)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
                if bad {
                    failures += 1;
                }
                totals.materials += report.materials;
                totals.entities += report.entities;
                totals.brush_entities += report.brush_entities;
                totals.patched += report.patched;
                totals.from_pakfile += report.from_pakfile;
                totals.no_texture += report.no_texture;
                totals.missing_vmt.extend(report.missing_vmt);
                totals.missing_vtf.extend(report.missing_vtf);
                totals.broken.extend(report.broken);
                for (shader, count) in report.shaders {
                    *totals.shaders.entry(shader).or_default() += count;
                }
            }
            Err(e) => {
                println!("{name:<34} FAIL  {e}");
                failures += 1;
            }
        }
    }

    println!(
        "\n{} maps, {failures} failures\n\
         {} entities parsed, {} of them brush entities\n\
         {} material references, {} via Patch, {} served from a map pakfile\n\
         {} absent .vmt, {} absent .vtf, {} broken .vmt, {} with no $basetexture",
        paths.len(),
        totals.entities,
        totals.brush_entities,
        totals.materials,
        totals.patched,
        totals.from_pakfile,
        totals.missing_vmt.len(),
        totals.missing_vtf.len(),
        totals.broken.len(),
        totals.no_texture,
    );

    let mut shaders: Vec<_> = totals.shaders.into_iter().collect();
    shaders.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    println!("\nshaders across all maps:");
    for (shader, count) in shaders.iter().take(15) {
        println!("  {count:>7}  {shader}");
    }

    if !worst.is_empty() {
        worst.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
        println!(
            "\nmaps referencing materials that are not in the game — drawn as \
             the missing-material checkerboard by the real engine:"
        );
        for (_, line) in worst.iter().take(10) {
            println!("  {line}");
        }
    }

    Ok(failures == 0)
}
