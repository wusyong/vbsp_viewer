//! Inspect VTF textures, export one as an image, or decode every texture a map
//! uses.
//!
//! ```text
//! vtfdump <path in the VFS>                    # header, mips, flags
//! vtfdump <path> --bmp out.bmp                 # mip 0 as a tonemapped BMP
//! vtfdump --all-textures                       # decode every VTF in the VPKs
//! vtfdump --all-maps                           # ...every VTF all 233 maps use
//! ```
//!
//! `--all-maps` is the M7 acceptance gate: it walks each map's materials
//! through the VFS, resolves `$basetexture`/`$basetexture2`/`$bumpmap`, parses
//! every VTF header and **decodes mip 0**, so a wrong image offset or a broken
//! block decoder fails loudly instead of producing a plausible-looking texture.
//!
//! BMP rather than PNG to stay dependency-free, as with `lmdump`.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use vfs::{vmt, Vfs};
use vtf::Vtf;

type Failure = Box<dyn std::error::Error>;

const DEFAULT_GAME_DIR: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\tf";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut game_dir = std::env::var_os("BEVY_TF2_GAME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_GAME_DIR));
    let mut path: Option<String> = None;
    let mut bmp: Option<PathBuf> = None;
    let mut sweep: Option<&str> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--game-dir" if i + 1 < args.len() => {
                game_dir = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--bmp" if i + 1 < args.len() => {
                bmp = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            flag @ ("--all-textures" | "--all-maps") => {
                sweep = Some(if flag == "--all-maps" { "maps" } else { "textures" });
                i += 1;
            }
            other if !other.starts_with("--") => {
                path = Some(other.to_string());
                i += 1;
            }
            other => {
                eprintln!("unexpected argument {other:?}");
                usage();
                return std::process::ExitCode::from(2);
            }
        }
    }

    let result = match (sweep, &path) {
        (Some("maps"), _) => all_maps(&game_dir),
        (Some(_), _) => all_textures(&game_dir),
        (None, Some(path)) => one(&game_dir, path, bmp.as_deref()),
        (None, None) => {
            usage();
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

fn usage() {
    eprintln!("usage: vtfdump [--game-dir <tf>] <path> [--bmp <out.bmp>]");
    eprintln!("       vtfdump [--game-dir <tf>] --all-textures");
    eprintln!("       vtfdump [--game-dir <tf>] --all-maps");
}

fn one(game_dir: &Path, path: &str, bmp: Option<&Path>) -> Result<bool, Failure> {
    let vfs = Vfs::from_game_dir(game_dir)?;
    // Accept a bare material name as well as a full path.
    let full = if path.ends_with(".vtf") {
        path.to_string()
    } else {
        vmt::texture_path(path)
    };
    let bytes = vfs.read(&full)?;
    let vtf = Vtf::parse(&bytes)?;

    println!("{full}\n  from {}\n", vfs.source_of(&full).unwrap_or_default());
    println!("  version        {}.{}", vtf.major, vtf.minor);
    println!("  size           {}x{}x{}", vtf.width, vtf.height, vtf.depth);
    println!("  format         {} ({})", vtf.format.name(), vtf.format as i32);
    println!("  mips           {}", vtf.mip_count);
    println!("  frames         {} (first {})", vtf.frames, vtf.first_frame);
    println!("  faces          {}", vtf.face_count());
    println!("  flags          {:#010x} {:?}", vtf.flags, vtf::flags::describe(vtf.flags));
    println!("  reflectivity   {:?}", vtf.reflectivity);
    println!("  bump scale     {}", vtf.bump_scale);
    println!("  header size    {:#x}", vtf.header_size);
    match vtf.low_res {
        Some(low) => println!(
            "  low-res        {}x{} {} at {:#x}",
            low.width,
            low.height,
            low.format.name(),
            low.offset
        ),
        None => println!("  low-res        none"),
    }
    // The declared image size and the bytes actually present should agree;
    // a difference means the offset or the mip arithmetic is wrong.
    println!("  image bytes    {}", vtf.image_bytes());
    println!("  file bytes     {}", bytes.len());

    println!("\n  {:>4} {:>12} {:>12}", "mip", "size", "bytes");
    for mip in 0..vtf.mip_count as usize {
        let (w, h, d) = vtf.mip_dimensions(mip);
        let ok = vtf.surface(mip, 0, 0).map(|s| s.len()).unwrap_or(0);
        println!("  {mip:>4} {:>12} {ok:>12}", format!("{w}x{h}x{d}"));
    }

    // Decoding every mip is the real check: a wrong offset usually still
    // yields plausible bytes for mip 0 alone.
    for mip in 0..vtf.mip_count as usize {
        vtf.decode_rgba8(mip, 0, 0)?;
    }
    println!("\n  all {} mips decoded", vtf.mip_count);

    // Channel statistics, because eyeballing a dump can mislead: an HDR
    // cubemap whose alpha is 0 composites to a flat colour, which looks like a
    // plausible texture until the numbers say otherwise.
    let rgba = vtf.decode_rgba8(0, 0, 0)?;
    println!("\n  {:>8} {:>5} {:>5} {:>6}", "channel", "min", "max", "mean");
    for (i, name) in ["red", "green", "blue", "alpha"].iter().enumerate() {
        let (mut lo, mut hi, mut sum, mut n) = (255u8, 0u8, 0u64, 0u64);
        for &v in rgba.iter().skip(i).step_by(4) {
            lo = lo.min(v);
            hi = hi.max(v);
            sum += u64::from(v);
            n += 1;
        }
        println!("  {name:>8} {lo:>5} {hi:>5} {:>6.1}", sum as f64 / n.max(1) as f64);
    }

    if let Some(out) = bmp {
        let (w, h, _) = vtf.mip_dimensions(0);
        write_bmp(&rgba, w, h, out)?;
        println!("  wrote {}", out.display());
    }
    Ok(true)
}

#[derive(Default)]
struct Totals {
    files: usize,
    decoded: usize,
    header_errors: Vec<String>,
    decode_errors: Vec<String>,
    formats: BTreeMap<&'static str, usize>,
    versions: BTreeMap<String, usize>,
    /// Files whose declared image data runs past the end of the file.
    short: Vec<String>,
    bytes: u64,
}

impl Totals {
    /// Parse, check the image block fits, and decode every mip.
    fn check(&mut self, path: &str, bytes: &[u8]) {
        self.files += 1;
        let vtf = match Vtf::parse(bytes) {
            Ok(v) => v,
            Err(e) => {
                self.header_errors.push(format!("{path}: {e}"));
                return;
            }
        };
        *self.formats.entry(vtf.format.name()).or_default() += 1;
        *self
            .versions
            .entry(format!("{}.{}", vtf.major, vtf.minor))
            .or_default() += 1;

        // The last mip is the one furthest into the file, so if it is
        // addressable the whole chain is.
        if vtf.surface(0, vtf.frames as usize - 1, vtf.face_count() - 1).is_err() {
            self.short.push(path.to_string());
        }

        for mip in 0..vtf.mip_count as usize {
            match vtf.decode_rgba8(mip, 0, 0) {
                Ok(rgba) => self.bytes += rgba.len() as u64,
                Err(e) => {
                    self.decode_errors.push(format!("{path} mip {mip}: {e}"));
                    return;
                }
            }
        }
        self.decoded += 1;
    }

    fn report(&self, what: &str) -> bool {
        println!(
            "\n{} {what}: {} parsed and fully decoded, {} header errors, \
             {} decode errors, {} with a short image block",
            self.files,
            self.decoded,
            self.header_errors.len(),
            self.decode_errors.len(),
            self.short.len(),
        );
        println!("{:.1} MiB of RGBA8 produced", self.bytes as f64 / 1_048_576.0);

        println!("\nversions: {:?}", self.versions);
        println!("formats:");
        let mut formats: Vec<_> = self.formats.iter().collect();
        formats.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        for (name, count) in formats {
            println!("  {count:>7}  {name}");
        }

        for (label, list) in [
            ("header errors", &self.header_errors),
            ("decode errors", &self.decode_errors),
        ] {
            if list.is_empty() {
                continue;
            }
            println!("\n{label}:");
            for item in list.iter().take(20) {
                println!("  {item}");
            }
            if list.len() > 20 {
                println!("  ... and {} more", list.len() - 20);
            }
        }
        if !self.short.is_empty() {
            println!("\nshort image blocks:");
            for item in self.short.iter().take(20) {
                println!("  {item}");
            }
        }

        self.header_errors.is_empty() && self.decode_errors.is_empty() && self.short.is_empty()
    }
}

/// Every VTF in the mounted search paths.
fn all_textures(game_dir: &Path) -> Result<bool, Failure> {
    let vfs = Vfs::from_game_dir(game_dir)?;
    let entries = vfs.list("materials");
    let mut totals = Totals::default();
    for (path, _) in entries.iter().filter(|(p, _)| p.ends_with(".vtf")) {
        match vfs.read(path) {
            Ok(bytes) => totals.check(path, &bytes),
            Err(e) => totals.header_errors.push(format!("{path}: {e}")),
        }
    }
    Ok(totals.report("textures"))
}

/// Every VTF referenced by every map's materials, with each map's pakfile
/// mounted — which is where the cubemap textures live.
fn all_maps(game_dir: &Path) -> Result<bool, Failure> {
    let maps = game_dir.join("maps");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&maps)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("bsp"))
        .collect();
    paths.sort();

    println!(
        "{:<30} {:>7} {:>8} {:>8} {:>7}",
        "map", "vtfs", "decoded", "missing", "errors"
    );

    let mut totals = Totals::default();
    let mut failures = 0usize;
    let mut missing_total = 0usize;

    for path in &paths {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let bsp = bsp::Bsp::open(path)?;
        let mut vfs = Vfs::from_game_dir(game_dir)?;
        vfs.mount_pakfile(bsp.lump_bytes(bsp::LumpId::PakFile)?.to_vec())?;

        let before = (totals.files, totals.decoded, totals.header_errors.len() + totals.decode_errors.len());
        let mut seen: HashSet<String> = HashSet::new();
        let mut missing = 0usize;

        for material in bsp.texture_names()? {
            let Ok(material) = vmt::load(&vfs, material) else {
                continue;
            };
            for texture in material.textures() {
                if !seen.insert(texture.clone()) {
                    continue;
                }
                match vfs.read(&texture) {
                    Ok(bytes) => totals.check(&texture, &bytes),
                    // An absent VTF is a map bug, already counted by the M6
                    // gate; this milestone is about decoding what is there.
                    Err(_) => missing += 1,
                }
            }
        }

        let errors = totals.header_errors.len() + totals.decode_errors.len() - before.2;
        println!(
            "{name:<30} {:>7} {:>8} {:>8} {:>7}{}",
            totals.files - before.0,
            totals.decoded - before.1,
            missing,
            errors,
            if errors > 0 { "  FAIL" } else { "" },
        );
        if errors > 0 {
            failures += 1;
        }
        missing_total += missing;
    }

    let ok = totals.report("map textures");
    println!("\n{} maps, {failures} with decode failures", paths.len());
    println!("{missing_total} texture references absent from the game (see the M6 gate)");
    Ok(ok && failures == 0)
}

/// 24-bit BGR BMP, bottom-up, rows padded to 4 bytes.
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
            // Composite onto a checkerboard, not a flat colour: a fully
            // transparent surface then reads as transparent instead of as a
            // plausible flat texture.
            let backdrop = if ((x / 8) + (y / 8)) % 2 == 0 { 96u32 } else { 160u32 };
            let a = rgba[at + 3] as u32;
            let blend = |c: u8| (((c as u32) * a + backdrop * (255 - a)) / 255) as u8;
            out.push(blend(rgba[at + 2]));
            out.push(blend(rgba[at + 1]));
            out.push(blend(rgba[at]));
        }
        out.extend(std::iter::repeat_n(0u8, padding));
    }
    std::fs::File::create(path)?.write_all(&out)
}
