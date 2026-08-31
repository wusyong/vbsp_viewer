//! Inspect BSP lump directories, and sweep a whole maps directory.
//!
//! ```text
//! lumpdump <map.bsp>              # header + per-lump table + a content probe
//! lumpdump --all-maps <tf dir>    # decompress every lump of every map
//! ```
//!
//! The sweep is the acceptance test for M1: it inflates all 64 lumps of every
//! map and asserts each fixed-stride lump is a whole number of elements, which
//! is what catches a struct transcribed with the wrong padding.

use bsp::{Bsp, LumpId};
use std::path::{Path, PathBuf};

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [flag, dir] if flag == "--all-maps" => sweep(Path::new(dir)),
        [path] if !path.starts_with("--") => dump(Path::new(path)),
        _ => {
            eprintln!("usage: lumpdump <map.bsp>");
            eprintln!("       lumpdump --all-maps <dir containing maps/, or maps/ itself>");
            return std::process::ExitCode::from(2);
        }
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

/// Print one map's directory and probe the lumps M2-M5 will consume.
fn dump(path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let bsp = Bsp::open(path)?;

    println!(
        "{}\n  version {}  revision {}  {} bytes",
        path.display(),
        bsp.version(),
        bsp.map_revision(),
        bsp.file_len(),
    );

    println!(
        "\n{:>2}  {:<30} {:>10} {:>10} {:>4} {:>10}  elements",
        "id", "lump", "fileofs", "ondisk", "ver", "inflated",
    );

    let mut compressed = 0usize;
    let mut on_disk = 0i64;
    let mut inflated = 0usize;

    for id in LumpId::ALL {
        let l = bsp.lump(id);
        if l.filelen == 0 {
            continue;
        }
        let is_comp = bsp.is_compressed(id);
        compressed += usize::from(is_comp);
        on_disk += l.filelen as i64;

        // Force the decode so a bad lump is reported here, not later.
        let len = match bsp.lump_bytes(id) {
            Ok(b) => b.len(),
            Err(e) => {
                println!("{:>2}  {:<30} FAILED: {e}", id.index(), id.name());
                continue;
            }
        };
        inflated += len;

        let elements = match id.stride(l.version) {
            Some(stride) if len % stride == 0 => format!("{} x {}B", len / stride, stride),
            Some(stride) => format!("!! {len} % {stride} = {}", len % stride),
            None => "-".to_string(),
        };

        println!(
            "{:>2}  {:<30} {:>10} {:>10} {:>4} {:>10}  {}",
            id.index(),
            id.name(),
            l.fileofs,
            l.filelen,
            l.version,
            if is_comp { len.to_string() } else { "-".into() },
            elements,
        );
    }

    println!(
        "\n  {compressed} of 64 lumps LZMA-compressed; \
         {on_disk} bytes on disk -> {inflated} bytes inflated"
    );

    probe(&bsp)?;
    Ok(true)
}

/// Read the data M2-M5 depend on, so the numbers can be eyeballed against the
/// real game before any of it reaches a renderer.
fn probe(bsp: &Bsp) -> Result<(), Box<dyn std::error::Error>> {
    let faces = bsp.faces()?;
    let models = bsp.models()?;
    let dispinfos = bsp.dispinfos()?;

    println!(
        "\n  geometry: {} verts, {} edges, {} surfedges, {} faces, {} models",
        bsp.vertices()?.len(),
        bsp.edges()?.len(),
        bsp.surfedges()?.len(),
        faces.len(),
        models.len(),
    );

    if let Some(world) = models.first() {
        println!(
            "  worldspawn: faces {}..{}, bounds {:?}..{:?}",
            world.firstface,
            world.firstface + world.numfaces,
            world.mins,
            world.maxs,
        );
    }

    // Displacement powers, to confirm the 2..=4 range the builder assumes.
    let mut powers = [0usize; 8];
    for d in dispinfos {
        if let Some(slot) = powers.get_mut(d.power.clamp(0, 7) as usize) {
            *slot += 1;
        }
    }
    let power_hist: Vec<String> = powers
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(p, n)| format!("power {p}: {n}"))
        .collect();
    println!(
        "  displacements: {} ({}), {} disp verts, {} disp tris",
        dispinfos.len(),
        power_hist.join(", "),
        bsp.disp_verts()?.len(),
        bsp.disp_tris()?.len(),
    );

    // Lightmap coverage, and the luxel budget the M5 atlas has to pack.
    let lit = faces.iter().filter(|f| f.styles[0] != 255 && f.lightofs >= 0);
    let (n_lit, luxels) = lit.fold((0usize, 0i64), |(n, sum), f| {
        (
            n + 1,
            sum + (f.lightmap_width() as i64 * f.lightmap_height() as i64),
        )
    });
    println!(
        "  lightmaps: {n_lit}/{} faces lit, {luxels} luxels, {} samples in LIGHTING",
        faces.len(),
        bsp.lighting()?.len(),
    );

    let names = bsp.texture_names()?;
    println!("  materials: {} unique", names.len());
    for name in names.iter().take(5) {
        println!("      {name}");
    }
    if names.len() > 5 {
        println!("      ... and {} more", names.len() - 5);
    }

    let ents = bsp.entities_str()?;
    let brush_ents = ents.matches("\"model\" \"*").count();
    println!(
        "  entities: {} bytes of KeyValues, {} brush-model entities",
        ents.len(),
        brush_ents,
    );

    let game = bsp.game_lumps()?;
    let ids: Vec<String> = game
        .iter()
        .map(|g| {
            let b = g.id.to_be_bytes();
            format!(
                "{}(v{})",
                String::from_utf8_lossy(&b).trim_matches('\0'),
                g.version
            )
        })
        .collect();
    println!("  game lumps: {}", ids.join(", "));

    Ok(())
}

/// Inflate every lump of every map under `dir`, asserting strides.
fn sweep(dir: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let maps_dir = if dir.join("maps").is_dir() {
        dir.join("maps")
    } else {
        dir.to_path_buf()
    };

    let mut paths: Vec<PathBuf> = std::fs::read_dir(&maps_dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("bsp")))
        .collect();
    paths.sort();

    if paths.is_empty() {
        return Err(format!("no .bsp files under {}", maps_dir.display()).into());
    }
    println!("sweeping {} maps in {}\n", paths.len(), maps_dir.display());

    let mut failures = 0usize;
    let mut versions = std::collections::BTreeMap::<i32, usize>::new();
    let mut powers = std::collections::BTreeSet::<i32>::new();
    let mut leaf_versions = std::collections::BTreeSet::<i32>::new();
    let mut total_inflated = 0u64;

    for path in &paths {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let bsp = match Bsp::open(path) {
            Ok(b) => b,
            Err(e) => {
                println!("FAIL {name}: {e}");
                failures += 1;
                continue;
            }
        };
        *versions.entry(bsp.version()).or_default() += 1;
        leaf_versions.insert(bsp.lump(LumpId::Leafs).version);

        let mut problems = Vec::new();
        for id in LumpId::ALL {
            if bsp.lump(id).filelen == 0 {
                continue;
            }
            match bsp.lump_bytes(id) {
                Ok(bytes) => {
                    total_inflated += bytes.len() as u64;
                    if let Some(stride) = id.stride(bsp.lump(id).version) {
                        let rem = bytes.len() % stride;
                        if rem != 0 {
                            problems.push(format!(
                                "{}: {} bytes % {stride} = {rem}",
                                id.name(),
                                bytes.len()
                            ));
                        }
                    }
                }
                Err(e) => problems.push(format!("{}: {e}", id.name())),
            }
        }

        // The displacement builder assumes power 2..=4; prove it per map.
        match bsp.dispinfos() {
            Ok(ds) => {
                for d in ds {
                    powers.insert(d.power);
                    if !(2..=4).contains(&d.power) {
                        problems.push(format!("DISPINFO: power {} out of range", d.power));
                        break;
                    }
                }
            }
            Err(e) => problems.push(format!("DISPINFO: {e}")),
        }

        if problems.is_empty() {
            println!("ok   {name}");
        } else {
            failures += 1;
            println!("FAIL {name}");
            for p in &problems {
                println!("       {p}");
            }
        }
    }

    println!("\n{} maps, {failures} failed", paths.len());
    println!("  bsp versions seen:   {versions:?}");
    println!("  LEAFS lump versions: {leaf_versions:?}");
    println!("  displacement powers: {powers:?}");
    println!("  total inflated:      {:.1} MiB", total_inflated as f64 / (1024.0 * 1024.0));
    Ok(failures == 0)
}
