//! Pack a map's lightmaps and report — or write the atlas out as an image.
//!
//! ```text
//! lmdump <map.bsp>                  # atlas size, occupancy, per-face stats
//! lmdump <map.bsp> --bmp out.bmp    # ...and a tonemapped BMP to eyeball
//! lmdump --all-maps <tf dir>        # pack every map; the M5 acceptance gate
//! ```
//!
//! The sweep fails on any lump error, any face whose sample range falls
//! outside the lighting lump, and on any map that packs no faces at all.
//! Unlit faces (`TEX_SPECIAL`, `styles[0] == 255`) are expected and only
//! reported.
//!
//! BMP rather than PNG so this stays dependency-free; Windows and most viewers
//! open it directly. The dump is tonemapped because the atlas is linear float
//! with a peak well above 1.0 — see [`tonemap`].

use bsp::geometry::Surface;
use bsp::lightmap::{self, LightmapAtlas, Lighting};
use bsp::{displacement, geometry, Bsp};
use std::io::Write;
use std::path::{Path, PathBuf};

type Failure = Box<dyn std::error::Error>;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let strs: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match strs.as_slice() {
        ["--all-maps", dir] => sweep(Path::new(dir)),
        [path] if !path.starts_with("--") => dump(Path::new(path), None),
        [path, "--bmp", out] if !path.starts_with("--") => {
            dump(Path::new(path), Some(Path::new(out)))
        }
        _ => {
            eprintln!("usage: lmdump <map.bsp> [--bmp <out.bmp>]");
            eprintln!("       lmdump --all-maps <dir containing maps/, or maps/ itself>");
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

/// Build the same surfaces the viewer does, so the atlas holds exactly the
/// faces that will be drawn.
fn build(bsp: &Bsp) -> Result<(Vec<Surface>, LightmapAtlas), Failure> {
    let world = geometry::build_worldspawn(bsp)?;
    let terrain = displacement::build_displacements(bsp)?;
    let mut surfaces = world.surfaces;
    surfaces.extend(terrain.surfaces);

    let faces: Vec<u32> = lightmap::faces_of(&surfaces).collect();
    let atlas = lightmap::build(bsp, faces, Lighting::Ldr)?;
    Ok((surfaces, atlas))
}

/// Vertices whose remapped UV left the atlas. The clamp in `remap` should make
/// this impossible; a non-zero count means a face is sampling a neighbour's
/// lighting.
fn uvs_outside_atlas(surfaces: &[Surface]) -> usize {
    surfaces
        .iter()
        .flat_map(|s| &s.vertices)
        .filter(|v| {
            let [u, w] = v.lightmap_uv;
            !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&w)
        })
        .count()
}

fn dump(path: &Path, bmp: Option<&Path>) -> Result<bool, Failure> {
    let bsp = Bsp::open(path)?;
    let started = std::time::Instant::now();
    let (mut surfaces, atlas) = build(&bsp)?;
    atlas.remap(&mut surfaces);
    let elapsed = started.elapsed();

    let s = &atlas.stats;
    println!("{}\n", path.display());
    println!("  atlas          {} x {} ({:.1} MiB as Rgba16Float)",
        atlas.width,
        atlas.height,
        atlas.pixel_count() as f32 * 8.0 / (1024.0 * 1024.0),
    );
    println!("  occupancy      {:.1}%", s.occupancy(atlas.pixel_count()) * 100.0);
    println!("  faces packed   {}", s.faces_packed);
    println!("  faces unlit    {} (sky / nolight / no bake)", s.faces_unlit);
    println!("  faces bad      {}", s.faces_bad_range);
    println!("  luxels         {}", s.luxels);
    println!("  source         {:?}", s.source);
    println!("  peak channel   {:.2}", s.peak);
    println!("  built in       {:.0} ms", elapsed.as_secs_f32() * 1000.0);

    let outside = uvs_outside_atlas(&surfaces);
    println!("  remapped UVs outside the atlas: {outside}");

    if let Some(out) = bmp {
        write_bmp(&atlas, out)?;
        println!("\n  wrote {}", out.display());
    }
    Ok(outside == 0 && s.faces_bad_range == 0)
}

fn sweep(dir: &Path) -> Result<bool, Failure> {
    let maps = dir.join("maps");
    let maps = if maps.is_dir() { maps } else { dir.to_path_buf() };

    let mut paths: Vec<PathBuf> = std::fs::read_dir(&maps)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("bsp"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("no .bsp files in {}", maps.display()).into());
    }

    println!(
        "{:<34} {:>11} {:>7} {:>8} {:>7} {:>7} {:>6} {:>5}",
        "map", "atlas", "occ%", "packed", "unlit", "bad", "peak", "oobUV"
    );

    let (mut failures, mut packed, mut luxels, mut unlit) = (0usize, 0usize, 0usize, 0usize);
    let mut widest = (0u32, 0u32, String::new());

    for path in &paths {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let outcome = Bsp::open(path).map_err(Failure::from).and_then(|bsp| {
            let (mut surfaces, atlas) = build(&bsp)?;
            atlas.remap(&mut surfaces);
            let outside = uvs_outside_atlas(&surfaces);
            Ok((atlas, outside))
        });

        match outcome {
            Ok((atlas, outside)) => {
                let s = &atlas.stats;
                let bad = s.faces_bad_range > 0 || s.faces_packed == 0 || outside > 0;
                println!(
                    "{name:<34} {:>5}x{:<5} {:>7.1} {:>8} {:>7} {:>7} {:>6.1} {:>5}{}",
                    atlas.width,
                    atlas.height,
                    s.occupancy(atlas.pixel_count()) * 100.0,
                    s.faces_packed,
                    s.faces_unlit,
                    s.faces_bad_range,
                    s.peak,
                    outside,
                    if bad { "  FAIL" } else { "" },
                );
                packed += s.faces_packed;
                unlit += s.faces_unlit;
                luxels += s.luxels;
                if atlas.pixel_count() > (widest.0 as usize * widest.1 as usize) {
                    widest = (atlas.width, atlas.height, name.clone());
                }
                if bad {
                    failures += 1;
                }
            }
            Err(e) => {
                println!("{name:<34} FAIL  {e}");
                failures += 1;
            }
        }
    }

    println!(
        "\n{} maps, {failures} failures; {packed} faces packed, {unlit} unlit, {luxels} luxels",
        paths.len()
    );
    println!("largest atlas: {}x{} ({})", widest.0, widest.1, widest.2);
    Ok(failures == 0)
}

/// Reinhard plus a gamma curve, so a linear atlas with a peak of 30 is still
/// legible as an image. Exposure is fixed rather than auto so two dumps can be
/// compared.
fn tonemap(c: f32) -> u8 {
    let mapped = (c * 1.5) / (c * 1.5 + 1.0);
    (mapped.powf(1.0 / 2.2).clamp(0.0, 1.0) * 255.0).round() as u8
}

/// 24-bit BGR BMP: bottom-up rows padded to 4 bytes, no compression.
fn write_bmp(atlas: &LightmapAtlas, path: &Path) -> std::io::Result<()> {
    let (w, h) = (atlas.width as usize, atlas.height as usize);
    let row_bytes = w * 3;
    let padding = (4 - row_bytes % 4) % 4;
    let pixel_bytes = (row_bytes + padding) * h;
    let mut out = Vec::with_capacity(54 + pixel_bytes);

    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + pixel_bytes) as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER
    out.extend_from_slice(&(atlas.width as i32).to_le_bytes());
    out.extend_from_slice(&(atlas.height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&24u16.to_le_bytes()); // bpp
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    for _ in 0..4 {
        out.extend_from_slice(&0u32.to_le_bytes()); // ppm + palette counts
    }

    // BMP rows run bottom-up, so the atlas's first row is written last.
    for y in (0..h).rev() {
        for x in 0..w {
            let c = atlas.pixel(x as u32, y as u32);
            out.push(tonemap(c[2]));
            out.push(tonemap(c[1]));
            out.push(tonemap(c[0]));
        }
        out.extend(std::iter::repeat_n(0u8, padding));
    }

    let mut file = std::fs::File::create(path)?;
    file.write_all(&out)
}
