//! The Source engine's virtual file system, enough of it to load a map's
//! materials.
//!
//! A Source game does not read files from a directory; it reads them from an
//! ordered stack of *search paths* declared in `gameinfo.txt` — VPK archives,
//! loose directories, and (while a map is loaded) the zip embedded in the BSP
//! itself. The **order is the semantics**: `tf/custom/` overrides the shipped
//! VPKs, which override `hl2/`, and a map's own pakfile overrides everything.
//! Getting the order wrong does not fail loudly, it just quietly loads the
//! wrong texture.
//!
//! ```no_run
//! # let pakfile_lump: Vec<u8> = Vec::new();
//! let mut vfs = vfs::Vfs::from_game_dir("C:/.../Team Fortress 2/tf")?;
//! vfs.mount_pakfile(pakfile_lump)?;
//! let vmt = vfs.read("materials/concrete/concretewall001.vmt")?;
//! # Ok::<(), vfs::VfsError>(())
//! ```
//!
//! Paths are normalised on the way in — lowercased, backslashes turned to
//! forward slashes — because BSP material names arrive as `CONCRETE/WALL001`
//! and VPK trees are stored lowercase.

pub mod entities;
pub mod gameinfo;
pub mod keyvalues;
pub mod pakfile;
pub mod vmt;
pub mod vpk;

pub use entities::Entity;
pub use gameinfo::{GameInfo, SearchPath};
pub use keyvalues::{KeyValues, KvError, Value};
pub use pakfile::{PakError, Pakfile};
pub use vmt::Material;
pub use vpk::{Vpk, VpkError};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("no gameinfo.txt in {dir} — is this a game's `tf/` directory?")]
    NoGameInfo { dir: PathBuf },

    #[error("{path}: {source}")]
    GameInfo {
        path: PathBuf,
        #[source]
        source: KvError,
    },

    #[error(transparent)]
    Vpk(#[from] VpkError),

    #[error(transparent)]
    Pakfile(#[from] PakError),

    #[error("`{path}` not found in any of the {mounts} mounted search paths")]
    NotFound { path: String, mounts: usize },
}

pub type Result<T> = std::result::Result<T, VfsError>;

/// Normalise a virtual path: lowercase, forward slashes, no leading separator
/// or `./`, no repeated separators.
///
/// Every lookup goes through this, so `MATERIALS\Concrete\Wall.VMT` and
/// `materials/concrete/wall.vmt` are the same file.
pub fn normalise_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut last_was_sep = true; // treat the start as a separator, dropping leading ones
    for ch in path.chars() {
        let ch = if ch == '\\' { '/' } else { ch };
        if ch == '/' {
            if !last_was_sep {
                out.push('/');
                last_was_sep = true;
            }
            continue;
        }
        last_was_sep = false;
        out.extend(ch.to_lowercase());
    }
    // `./x` and a trailing slash are both noise.
    let trimmed = out.trim_end_matches('/');
    let trimmed = trimmed.strip_prefix("./").unwrap_or(trimmed);
    trimmed.to_string()
}

/// One entry in the search path stack.
enum Mount {
    /// A loose directory on disk.
    Loose(PathBuf),
    /// A VPK archive set.
    Vpk(Box<Vpk>),
    /// The zip embedded in a BSP's `LUMP_PAKFILE`.
    Pakfile(Box<Pakfile>),
}

impl Mount {
    fn describe(&self) -> String {
        match self {
            Mount::Loose(dir) => dir.display().to_string(),
            Mount::Vpk(vpk) => vpk.path().display().to_string(),
            Mount::Pakfile(_) => "<bsp pakfile>".to_string(),
        }
    }
}

/// An ordered stack of search paths. Earlier mounts win.
pub struct Vfs {
    mounts: Vec<Mount>,
    game_dir: PathBuf,
    /// Search paths gameinfo asked for that are not on disk. Not an error —
    /// `tf/tf2_lv.vpk` is listed but only shipped for low-violence installs —
    /// but worth surfacing when a texture goes missing.
    pub missing: Vec<PathBuf>,
}

impl Vfs {
    /// Build the search path stack from `<game_dir>/gameinfo.txt`.
    pub fn from_game_dir(game_dir: impl AsRef<Path>) -> Result<Vfs> {
        let game_dir = game_dir.as_ref().to_path_buf();
        let path = game_dir.join("gameinfo.txt");
        if !path.is_file() {
            return Err(VfsError::NoGameInfo { dir: game_dir });
        }
        let text = std::fs::read_to_string(&path).map_err(|source| VfsError::Io {
            path: path.clone(),
            source,
        })?;
        let info = GameInfo::parse(&text).map_err(|source| VfsError::GameInfo { path, source })?;

        let mut vfs = Vfs {
            mounts: Vec::new(),
            game_dir: game_dir.clone(),
            missing: Vec::new(),
        };
        for resolved in info.resolve(&game_dir) {
            vfs.mount(&resolved)?;
        }
        Ok(vfs)
    }

    /// Mount one resolved search path, skipping it if it is not on disk.
    fn mount(&mut self, path: &Path) -> Result<()> {
        if path.is_dir() {
            // Dedupe: gameinfo lists `tf` twice, once as `|gameinfo_path|.`
            // and once bare.
            let already = self.mounts.iter().any(|m| match m {
                Mount::Loose(dir) => same_dir(dir, path),
                _ => false,
            });
            if !already {
                self.mounts.push(Mount::Loose(path.to_path_buf()));
            }
            return Ok(());
        }
        if path.is_file() {
            self.mounts.push(Mount::Vpk(Box::new(Vpk::open(path)?)));
            return Ok(());
        }
        self.missing.push(path.to_path_buf());
        Ok(())
    }

    /// Mount a BSP's `LUMP_PAKFILE` zip at the **top** of the stack.
    ///
    /// Community maps ship their custom materials this way, and the engine
    /// mounts them above everything else for exactly as long as the map is
    /// loaded — so a map can override a shipped texture without replacing it.
    pub fn mount_pakfile(&mut self, bytes: Vec<u8>) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let pak = Pakfile::parse(bytes)?;
        if pak.is_empty() {
            return Ok(());
        }
        self.mounts.insert(0, Mount::Pakfile(Box::new(pak)));
        Ok(())
    }

    /// Drop any mounted pakfile, for when a different map is loaded.
    pub fn unmount_pakfile(&mut self) {
        self.mounts.retain(|m| !matches!(m, Mount::Pakfile(_)));
    }

    pub fn game_dir(&self) -> &Path {
        &self.game_dir
    }

    pub fn mount_count(&self) -> usize {
        self.mounts.len()
    }

    /// Human-readable search path stack, in priority order.
    pub fn mount_descriptions(&self) -> Vec<String> {
        self.mounts.iter().map(Mount::describe).collect()
    }

    /// Which mount would serve `path`, as a display string.
    pub fn source_of(&self, path: &str) -> Option<String> {
        let path = normalise_path(path);
        self.mounts
            .iter()
            .find(|m| self.mount_has(m, &path))
            .map(Mount::describe)
    }

    pub fn exists(&self, path: &str) -> bool {
        let path = normalise_path(path);
        self.mounts.iter().any(|m| self.mount_has(m, &path))
    }

    fn mount_has(&self, mount: &Mount, path: &str) -> bool {
        match mount {
            Mount::Loose(dir) => dir.join(path).is_file(),
            Mount::Vpk(vpk) => vpk.contains(path),
            Mount::Pakfile(pak) => pak.contains(path),
        }
    }

    /// Read a file from the highest-priority mount that has it.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let path = normalise_path(path);
        for mount in &self.mounts {
            match mount {
                Mount::Loose(dir) => {
                    let full = dir.join(&path);
                    match std::fs::read(&full) {
                        Ok(bytes) => return Ok(bytes),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        // A directory in the way, or a permissions problem, is
                        // worth reporting rather than silently falling through
                        // to a stale copy further down the stack.
                        Err(source) => return Err(VfsError::Io { path: full, source }),
                    }
                }
                Mount::Vpk(vpk) => {
                    if vpk.contains(&path) {
                        return Ok(vpk.read(&path)?);
                    }
                }
                Mount::Pakfile(pak) => {
                    if pak.contains(&path) {
                        return Ok(pak.read(&path)?);
                    }
                }
            }
        }
        Err(VfsError::NotFound {
            path,
            mounts: self.mounts.len(),
        })
    }

    /// Read a file as text, replacing invalid UTF-8 rather than failing.
    ///
    /// Some shipped VMTs contain stray high bytes in comments; lossy decoding
    /// keeps one bad byte from losing a whole material.
    pub fn read_to_string(&self, path: &str) -> Result<String> {
        let bytes = self.read(path)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Every path under `prefix`, deduplicated, with the mount that serves it.
    ///
    /// Ordered by mount priority, so the first entry for a path is the one
    /// [`Vfs::read`] would return. Loose directories are walked, so a broad
    /// prefix over `tf/` is slow — this is a debugging tool.
    pub fn list(&self, prefix: &str) -> Vec<(String, String)> {
        let prefix = normalise_path(prefix);
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut out = Vec::new();

        for mount in &self.mounts {
            let description = mount.describe();
            let mut push = |path: String| {
                if path.starts_with(&prefix) && seen.insert(path.clone(), ()).is_none() {
                    out.push((path, description.clone()));
                }
            };
            match mount {
                Mount::Vpk(vpk) => {
                    for path in vpk.paths() {
                        push(path.to_string());
                    }
                }
                Mount::Pakfile(pak) => {
                    for name in pak.names() {
                        push(name.to_string());
                    }
                }
                Mount::Loose(dir) => {
                    // Start the walk at the prefix rather than at the mount
                    // root: `tf/` holds gigabytes of loose files.
                    let start = dir.join(&prefix);
                    let base = dir.clone();
                    walk(&start, &mut |file| {
                        if let Ok(rel) = file.strip_prefix(&base) {
                            push(normalise_path(&rel.to_string_lossy()));
                        }
                    });
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

/// Whether two directory paths name the same place, canonicalising when
/// possible so `tf/.` and `tf` dedupe.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Depth-first walk, calling `visit` for every file. Silently skips anything
/// unreadable — a search path is allowed to be partly inaccessible.
fn walk(dir: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // `dir` may simply be a file, which is a valid single-item listing.
        if dir.is_file() {
            visit(dir);
        }
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(&path, visit),
            Ok(t) if t.is_file() => visit(&path),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation_matches_bsp_material_names_to_vpk_entries() {
        // The exact transformation a BSP texdata name needs.
        assert_eq!(
            normalise_path("MATERIALS\\Concrete\\ConcreteWall001.VMT"),
            "materials/concrete/concretewall001.vmt"
        );
        assert_eq!(normalise_path("/materials//x/"), "materials/x");
        assert_eq!(normalise_path("./materials/x"), "materials/x");
        assert_eq!(normalise_path(""), "");
    }

    #[test]
    fn normalisation_is_idempotent() {
        // `list` normalises paths that came out of an already-normalised tree.
        for path in [
            "MATERIALS\\A\\B.VMT",
            "a/b/c",
            "//a//b//",
            "./x",
            "Models/Props/X.mdl",
        ] {
            let once = normalise_path(path);
            assert_eq!(normalise_path(&once), once, "{path}");
        }
    }
}
