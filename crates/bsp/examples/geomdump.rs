//! Build brush geometry and report what came out.
//!
//! ```text
//! geomdump <map.bsp>              # per-material breakdown for one map
//! geomdump --all-maps <tf dir>    # build every model of every map
//! ```
//!
//! The sweep is the acceptance test for M2: it builds all models of all maps
//! and fails on any out-of-range edge index, any lump error, or a worldspawn
//! that yields no faces at all. Zero-area fan triangles are counted but are
//! expected map content, not a failure.

use bsp::displacement;
use bsp::geometry::{self, ModelGeometry};
use bsp::Bsp;
use std::path::{Path, PathBuf};

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [flag, dir] if flag == "--all-maps" => sweep(Path::new(dir)),
        [path] if !path.starts_with("--") => dump(Path::new(path)),
        _ => {
            eprintln!("usage: geomdump <map.bsp>");
            eprintln!("       geomdump --all-maps <dir containing maps/, or maps/ itself>");
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

fn dump(path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let bsp = Bsp::open(path)?;
    let world = geometry::build_worldspawn(&bsp)?;
    let names = bsp.texture_names()?;

    println!("{}\n", path.display());
    report(&world, "worldspawn");

    // Largest materials first — these are what dominate the frame.
    let mut surfaces: Vec<_> = world.surfaces.iter().enumerate().collect();
    surfaces.sort_by_key(|(_, s)| std::cmp::Reverse(s.triangle_count()));

    println!("\n  {:>6} {:>8} {:>8}  {:<44} flags", "tris", "verts", "faces", "material");
    for (_, s) in surfaces.iter().take(15) {
        let name = usize::try_from(s.texdata)
            .ok()
            .and_then(|i| names.get(i).copied())
            .unwrap_or("<bad texdata>");
        println!(
            "  {:>6} {:>8} {:>8}  {:<44} {:#06x}",
            s.triangle_count(),
            s.vertices.len(),
            s.chunks.len(),
            name,
            s.flags,
        );
    }
    if surfaces.len() > 15 {
        println!("  ... and {} more materials", surfaces.len() - 15);
    }

    let disp = displacement::build_displacements(&bsp)?;
    let d = &disp.stats;
    println!(
        "
  displacements: {}/{} built, {} materials, {} verts, {} tris",
        d.built,
        d.displacements,
        disp.surfaces.len(),
        disp.vertex_count(),
        disp.triangle_count(),
    );
    println!(
        "  skipped {}: {} bad power, {} bad face, {} no texinfo, {} short verts",
        d.skipped(),
        d.skipped_bad_power,
        d.skipped_bad_face,
        d.skipped_no_texinfo,
        d.skipped_short_verts,
    );
    println!("  {} triangles tagged REMOVE", d.removed_triangles);

    // Brush entities: models 1.. hold doors, gates and moving platforms. The
    // viewer needs the ENTITIES lump to place them, which is M6's KeyValues
    // parser; the geometry itself builds fine now.
    let model_count = bsp.models()?.len();
    let mut ent_tris = 0usize;
    let mut ent_verts = 0usize;
    for i in 1..model_count {
        let m = geometry::build_model(&bsp, i)?;
        ent_tris += m.triangle_count();
        ent_verts += m.vertex_count();
    }
    println!(
        "\n  brush entities: {} models, {ent_verts} verts, {ent_tris} tris \
         (placement needs the ENTITIES lump — M6)",
        model_count.saturating_sub(1),
    );

    Ok(true)
}

fn report(g: &ModelGeometry, label: &str) {
    let s = &g.stats;
    println!(
        "  {label}: {} materials, {} verts, {} tris from {}/{} faces",
        g.surfaces.len(),
        g.vertex_count(),
        g.triangle_count(),
        s.faces_built,
        s.faces_total,
    );
    println!(
        "  skipped {}: {} displacement, {} tool surface, {} no texinfo, \
         {} too few edges, {} bad index",
        s.faces_skipped(),
        s.skipped.displacement,
        s.skipped.tool_surface,
        s.skipped.no_texinfo,
        s.skipped.too_few_edges,
        s.skipped.bad_index,
    );
    if s.degenerate_triangles > 0 {
        let total = g.triangle_count() + s.degenerate_triangles;
        println!(
            "  {} zero-area fan triangles dropped ({:.1}% — collinear t-junction verts, expected)",
            s.degenerate_triangles,
            100.0 * s.degenerate_triangles as f64 / total as f64,
        );
    }
    println!("  bounds: {:?} .. {:?}", g.mins, g.maxs);
}

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
    println!("building {} maps in {}\n", paths.len(), maps_dir.display());

    let mut failures = 0usize;
    let mut tot_verts = 0u64;
    let mut tot_tris = 0u64;
    let mut tot_degenerate = 0u64;
    let mut tot_bad_index = 0u64;
    let mut tot_too_few = 0u64;
    let mut tot_disp = 0u64;
    let mut tot_disp_removed = 0u64;
    let mut worst_materials = (0usize, String::new());
    let mut worst_tris = (0usize, String::new());

    for path in &paths {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let bsp = match Bsp::open(path) {
            Ok(b) => b,
            Err(e) => {
                println!("FAIL {name}: {e}");
                failures += 1;
                continue;
            }
        };

        let model_count = match bsp.models() {
            Ok(m) => m.len(),
            Err(e) => {
                println!("FAIL {name}: {e}");
                failures += 1;
                continue;
            }
        };

        let mut problems: Vec<String> = Vec::new();
        let mut verts = 0usize;
        let mut tris = 0usize;
        let mut materials = 0usize;

        for i in 0..model_count {
            match geometry::build_model(&bsp, i) {
                Ok(g) => {
                    let s = &g.stats;
                    verts += g.vertex_count();
                    tris += g.triangle_count();
                    if i == 0 {
                        materials = g.surfaces.len();
                    }
                    tot_degenerate += s.degenerate_triangles as u64;
                    tot_bad_index += s.skipped.bad_index as u64;
                    tot_too_few += s.skipped.too_few_edges as u64;

                    // Zero-area fan triangles are expected map content
                    // (collinear t-junction vertices), so they are counted but
                    // never a failure. A bad index is a genuine misread.
                    if s.skipped.bad_index > 0 {
                        problems.push(format!(
                            "model {i}: {} faces with out-of-range edge indices",
                            s.skipped.bad_index
                        ));
                    }
                    // Worldspawn producing nothing would mean the walk is broken.
                    if i == 0 && s.faces_built == 0 && s.faces_total > 0 {
                        problems.push("model 0: built no faces at all".to_string());
                    }
                }
                Err(e) => problems.push(format!("model {i}: {e}")),
            }
        }

        match displacement::build_displacements(&bsp) {
            Ok(disp) => {
                let d = &disp.stats;
                verts += disp.vertex_count();
                tris += disp.triangle_count();
                tot_disp += d.built as u64;
                tot_disp_removed += d.removed_triangles as u64;
                if d.skipped() > 0 {
                    problems.push(format!(
                        "displacements: {} skipped ({} bad power, {} bad face,                          {} no texinfo, {} short verts)",
                        d.skipped(),
                        d.skipped_bad_power,
                        d.skipped_bad_face,
                        d.skipped_no_texinfo,
                        d.skipped_short_verts,
                    ));
                }
            }
            Err(e) => problems.push(format!("displacements: {e}")),
        }

        tot_verts += verts as u64;
        tot_tris += tris as u64;
        if materials > worst_materials.0 {
            worst_materials = (materials, name.clone());
        }
        if tris > worst_tris.0 {
            worst_tris = (tris, name.clone());
        }

        if problems.is_empty() {
            println!("ok   {name}  {materials} materials, {verts} verts, {tris} tris");
        } else {
            failures += 1;
            println!("FAIL {name}");
            for p in &problems {
                println!("       {p}");
            }
        }
    }

    println!("\n{} maps, {failures} failed", paths.len());
    println!("  total: {tot_verts} verts, {tot_tris} tris");
    println!("  zero-area triangles dropped:   {tot_degenerate} (expected: collinear t-junction verts)");
    println!("  faces with bad edge indices:  {tot_bad_index}");
    println!("  faces with < 3 edges:         {tot_too_few}");
    println!("  displacements built:           {tot_disp}");
    println!("  displacement tris tagged REMOVE: {tot_disp_removed}");
    println!(
        "  heaviest: {} tris ({}), most materials: {} ({})",
        worst_tris.0, worst_tris.1, worst_materials.0, worst_materials.1,
    );
    Ok(failures == 0)
}
