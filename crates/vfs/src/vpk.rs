//! VPK version 2 archives.
//!
//! There is **no VPK code in the Source 2013 SDK** — `src/filesystem/` is
//! absent — so this is written from the documented format and validated against
//! the real files shipped with TF2.
//!
//! # Layout
//!
//! ```text
//! header  28 bytes: signature 0x55aa1234, version 2, tree_size,
//!                   file_data_section_size, archive_md5_section_size,
//!                   other_md5_section_size, signature_section_size
//! tree    tree_size bytes, three nested levels of NUL-terminated strings:
//!           extension → directory → filename, each level ended by an empty
//!           string, with an 18-byte record per file followed by its preload
//!           bytes
//! data    file_data_section_size bytes — payloads stored in the _dir file
//! ```
//!
//! The per-file record is
//!
//! ```text
//! crc u32, preload_bytes u16, archive_index u16, entry_offset u32,
//! entry_length u32, terminator u16 = 0xffff
//! ```
//!
//! **The 18th and 19th bytes are a `0xffff` terminator.** Leaving it out — an
//! easy mistake, since every other field is self-describing — desynchronises
//! the tree from the second file onward. It is checked on every entry here, so
//! a drift fails loudly instead of producing plausible garbage.
//!
//! A file's bytes can live in three places, and may be split across two of
//! them: `preload_bytes` inline in the tree, the `_dir` file's own data section
//! (`archive_index == 0x7fff`), or a numbered sibling archive
//! (`tf2_textures_000.vpk`). [`Vpk::read`] concatenates the preload and the
//! body, which is the case a reader that assumes one or the other gets wrong.

use memmap2::Mmap;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const VPK_SIGNATURE: u32 = 0x55aa_1234;
pub const VPK_VERSION: u32 = 2;

/// `archive_index` meaning "in the `_dir` file's own data section".
pub const ARCHIVE_INDEX_DIR: u16 = 0x7fff;

const HEADER_LEN: usize = 28;
const RECORD_LEN: usize = 18;
const RECORD_TERMINATOR: u16 = 0xffff;

/// Valve writes a single space where a name component is empty — a file at the
/// archive root, or one with no extension.
const EMPTY_COMPONENT: &str = " ";

#[derive(Debug, thiserror::Error)]
pub enum VpkError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("{path}: not a VPK (signature {found:#010x}, expected {VPK_SIGNATURE:#010x})")]
    BadSignature { path: PathBuf, found: u32 },

    #[error("{path}: VPK version {found} is not supported (this reader handles {VPK_VERSION})")]
    BadVersion { path: PathBuf, found: u32 },

    #[error("{path}: truncated — {what} needs bytes up to {end} but the file is {len}")]
    Truncated {
        path: PathBuf,
        what: &'static str,
        end: usize,
        len: usize,
    },

    #[error("{path}: directory tree ran past its {tree_size} bytes while reading {what}")]
    TreeOverrun {
        path: PathBuf,
        what: &'static str,
        tree_size: usize,
    },

    #[error(
        "{path}: entry `{entry}` record terminator is {found:#06x}, expected \
         {RECORD_TERMINATOR:#06x} — the tree is out of sync"
    )]
    BadTerminator {
        path: PathBuf,
        entry: String,
        found: u16,
    },

    #[error("{path}: entry `{entry}` name is not UTF-8")]
    BadName { path: PathBuf, entry: String },

    #[error("`{path}` is not in this VPK")]
    NotFound { path: String },

    #[error("entry `{entry}` needs archive {index:03} at {archive}, which is missing")]
    MissingArchive {
        entry: String,
        index: u16,
        archive: PathBuf,
    },

    #[error("entry `{entry}` claims bytes {ofs}..{end} of {archive}, which is {len} bytes")]
    EntryOutOfBounds {
        entry: String,
        archive: PathBuf,
        ofs: usize,
        end: usize,
        len: usize,
    },
}

pub type Result<T> = std::result::Result<T, VpkError>;

/// Where one file's bytes are.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub crc: u32,
    /// Byte range of the inline preload data inside the `_dir` file.
    preload_offset: u32,
    pub preload_bytes: u16,
    /// [`ARCHIVE_INDEX_DIR`] for the `_dir` file's data section, else the
    /// numbered sibling archive.
    pub archive_index: u16,
    pub entry_offset: u32,
    pub entry_length: u32,
}

impl Entry {
    /// Total size of the file: preload plus body.
    pub fn size(&self) -> u64 {
        u64::from(self.preload_bytes) + u64::from(self.entry_length)
    }
}

/// One VPK archive set: a `_dir.vpk` plus any numbered siblings.
///
/// `Debug` prints the archive's path and entry count, not its mapped bytes.
pub struct Vpk {
    dir_path: PathBuf,
    dir: Mmap,
    /// Start of the `_dir` file's own data section.
    data_offset: usize,
    entries: HashMap<String, Entry>,
    /// Lazily mapped, indexed by `archive_index`. `None` inside the slot means
    /// the file was tried and is missing.
    archives: Vec<OnceLock<Option<Mmap>>>,
}

impl std::fmt::Debug for Vpk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vpk")
            .field("path", &self.dir_path)
            .field("entries", &self.entries.len())
            .field("archives", &self.archives.len())
            .finish()
    }
}

impl Vpk {
    /// Open a `*_dir.vpk` and read its directory tree.
    ///
    /// The numbered archives are not touched until a file inside one is read,
    /// so mounting the whole game costs a few directory parses rather than
    /// three gigabytes of mapping.
    pub fn open(dir_path: impl AsRef<Path>) -> Result<Vpk> {
        let dir_path = dir_path.as_ref().to_path_buf();
        let file = std::fs::File::open(&dir_path).map_err(|source| VpkError::Io {
            path: dir_path.clone(),
            source,
        })?;
        // SAFETY: same contract as elsewhere in this workspace — the game's
        // own files are not expected to be truncated underneath us.
        let dir = unsafe { Mmap::map(&file) }.map_err(|source| VpkError::Io {
            path: dir_path.clone(),
            source,
        })?;

        if dir.len() < HEADER_LEN {
            return Err(VpkError::Truncated {
                path: dir_path,
                what: "header",
                end: HEADER_LEN,
                len: dir.len(),
            });
        }
        let signature = u32::from_le_bytes(dir[0..4].try_into().unwrap());
        if signature != VPK_SIGNATURE {
            return Err(VpkError::BadSignature {
                path: dir_path,
                found: signature,
            });
        }
        let version = u32::from_le_bytes(dir[4..8].try_into().unwrap());
        if version != VPK_VERSION {
            return Err(VpkError::BadVersion {
                path: dir_path,
                found: version,
            });
        }
        let tree_size = u32::from_le_bytes(dir[8..12].try_into().unwrap()) as usize;
        let tree_end = HEADER_LEN + tree_size;
        if tree_end > dir.len() {
            return Err(VpkError::Truncated {
                path: dir_path,
                what: "directory tree",
                end: tree_end,
                len: dir.len(),
            });
        }

        let mut vpk = Vpk {
            dir_path,
            dir,
            data_offset: tree_end,
            entries: HashMap::new(),
            archives: Vec::new(),
        };
        vpk.read_tree(tree_size)?;
        Ok(vpk)
    }

    fn read_tree(&mut self, tree_size: usize) -> Result<()> {
        let tree = &self.dir[HEADER_LEN..HEADER_LEN + tree_size];
        let mut cursor = Cursor { bytes: tree, pos: 0 };
        let mut highest_archive = 0u16;
        let mut entries = HashMap::new();

        loop {
            let extension = self.take_string(&mut cursor, "extension")?;
            if extension.is_empty() {
                break;
            }
            loop {
                let directory = self.take_string(&mut cursor, "directory")?;
                if directory.is_empty() {
                    break;
                }
                loop {
                    let name = self.take_string(&mut cursor, "file name")?;
                    if name.is_empty() {
                        break;
                    }
                    let path = join_name(&directory, &name, &extension);
                    let record = cursor.take(RECORD_LEN).ok_or_else(|| VpkError::TreeOverrun {
                        path: self.dir_path.clone(),
                        what: "file record",
                        tree_size,
                    })?;

                    let terminator = u16::from_le_bytes(record[16..18].try_into().unwrap());
                    if terminator != RECORD_TERMINATOR {
                        return Err(VpkError::BadTerminator {
                            path: self.dir_path.clone(),
                            entry: path,
                            found: terminator,
                        });
                    }
                    let preload_bytes = u16::from_le_bytes(record[4..6].try_into().unwrap());
                    let archive_index = u16::from_le_bytes(record[6..8].try_into().unwrap());
                    let preload_offset = (HEADER_LEN + cursor.pos) as u32;
                    if cursor.take(preload_bytes as usize).is_none() {
                        return Err(VpkError::TreeOverrun {
                            path: self.dir_path.clone(),
                            what: "preload data",
                            tree_size,
                        });
                    }

                    if archive_index != ARCHIVE_INDEX_DIR {
                        highest_archive = highest_archive.max(archive_index);
                    }
                    entries.insert(
                        path,
                        Entry {
                            crc: u32::from_le_bytes(record[0..4].try_into().unwrap()),
                            preload_offset,
                            preload_bytes,
                            archive_index,
                            entry_offset: u32::from_le_bytes(record[8..12].try_into().unwrap()),
                            entry_length: u32::from_le_bytes(record[12..16].try_into().unwrap()),
                        },
                    );
                }
            }
        }

        self.entries = entries;
        self.archives = (0..=highest_archive as usize).map(|_| OnceLock::new()).collect();
        Ok(())
    }

    fn take_string(&self, cursor: &mut Cursor<'_>, what: &'static str) -> Result<String> {
        let bytes = cursor.take_cstr().ok_or_else(|| VpkError::TreeOverrun {
            path: self.dir_path.clone(),
            what,
            tree_size: cursor.bytes.len(),
        })?;
        // Names are ASCII in practice; be explicit rather than lossy so a
        // misaligned tree shows up as an error instead of replacement chars.
        String::from_utf8(bytes.to_vec()).map_err(|_| VpkError::BadName {
            path: self.dir_path.clone(),
            entry: String::from_utf8_lossy(bytes).into_owned(),
        })
    }

    /// The `_dir.vpk` this was opened from.
    pub fn path(&self) -> &Path {
        &self.dir_path
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up a normalised path (lowercase, `/`-separated).
    pub fn entry(&self, path: &str) -> Option<&Entry> {
        self.entries.get(path)
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// Every path in the archive, in arbitrary order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Read a file, joining its preload bytes to its body.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let entry = self.entry(path).ok_or_else(|| VpkError::NotFound {
            path: path.to_string(),
        })?;
        let mut out = Vec::with_capacity(entry.size() as usize);

        if entry.preload_bytes > 0 {
            let start = entry.preload_offset as usize;
            let end = start + entry.preload_bytes as usize;
            out.extend_from_slice(&self.dir[start..end]);
        }
        if entry.entry_length > 0 {
            let start = entry.entry_offset as usize;
            let end = start + entry.entry_length as usize;
            if entry.archive_index == ARCHIVE_INDEX_DIR {
                let start = self.data_offset + start;
                let end = self.data_offset + end;
                if end > self.dir.len() {
                    return Err(VpkError::EntryOutOfBounds {
                        entry: path.to_string(),
                        archive: self.dir_path.clone(),
                        ofs: start,
                        end,
                        len: self.dir.len(),
                    });
                }
                out.extend_from_slice(&self.dir[start..end]);
            } else {
                let archive = self.archive(entry.archive_index, path)?;
                if end > archive.len() {
                    return Err(VpkError::EntryOutOfBounds {
                        entry: path.to_string(),
                        archive: self.archive_path(entry.archive_index),
                        ofs: start,
                        end,
                        len: archive.len(),
                    });
                }
                out.extend_from_slice(&archive[start..end]);
            }
        }
        Ok(out)
    }

    /// Path of a numbered sibling archive: `tf2_textures_dir.vpk` → `..._007.vpk`.
    fn archive_path(&self, index: u16) -> PathBuf {
        let stem = self
            .dir_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let base = stem.strip_suffix("_dir").unwrap_or(&stem);
        self.dir_path
            .with_file_name(format!("{base}_{index:03}.vpk"))
    }

    fn archive(&self, index: u16, entry: &str) -> Result<&Mmap> {
        let slot = self
            .archives
            .get(index as usize)
            .ok_or_else(|| VpkError::MissingArchive {
                entry: entry.to_string(),
                index,
                archive: self.archive_path(index),
            })?;
        let mapped = slot.get_or_init(|| {
            let path = self.archive_path(index);
            let file = std::fs::File::open(&path).ok()?;
            // SAFETY: as above.
            unsafe { Mmap::map(&file) }.ok()
        });
        mapped.as_ref().ok_or_else(|| VpkError::MissingArchive {
            entry: entry.to_string(),
            index,
            archive: self.archive_path(index),
        })
    }
}

/// Assemble `dir/name.ext`, normalised, with Valve's single-space stand-in for
/// an empty component removed.
fn join_name(directory: &str, name: &str, extension: &str) -> String {
    let mut path = String::with_capacity(directory.len() + name.len() + extension.len() + 2);
    if directory != EMPTY_COMPONENT && !directory.is_empty() {
        path.push_str(directory);
        path.push('/');
    }
    path.push_str(name);
    if extension != EMPTY_COMPONENT && !extension.is_empty() {
        path.push('.');
        path.push_str(extension);
    }
    crate::normalise_path(&path)
}

/// A byte cursor over the directory tree.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let slice = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    /// Read a NUL-terminated string, consuming the NUL.
    fn take_cstr(&mut self) -> Option<&'a [u8]> {
        let rest = self.bytes.get(self.pos..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        self.pos += end + 1;
        Some(&rest[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_components_use_valves_single_space() {
        assert_eq!(join_name("materials/concrete", "wall", "vmt"), "materials/concrete/wall.vmt");
        // A file at the archive root.
        assert_eq!(join_name(" ", "readme", "txt"), "readme.txt");
        // A file with no extension.
        assert_eq!(join_name("cfg", "motd", " "), "cfg/motd");
        assert_eq!(join_name(" ", "noext", " "), "noext");
    }

    #[test]
    fn names_are_normalised_for_lookup() {
        // BSP material names arrive uppercase with mixed separators.
        assert_eq!(
            join_name("MATERIALS\\Concrete", "ConcreteWall001", "VMT"),
            "materials/concrete/concretewall001.vmt"
        );
    }

    #[test]
    fn archive_path_replaces_the_dir_suffix() {
        // Build a Vpk by hand; `open` needs a real file, but this is pure
        // path arithmetic.
        let stem = Path::new("/game/tf/tf2_textures_dir.vpk");
        let base = stem
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .strip_suffix("_dir")
            .unwrap()
            .to_string();
        assert_eq!(base, "tf2_textures");
        assert_eq!(
            stem.with_file_name(format!("{base}_{:03}.vpk", 7)),
            Path::new("/game/tf/tf2_textures_007.vpk")
        );
    }

    #[test]
    fn cursor_stops_at_the_end_instead_of_panicking() {
        let mut cursor = Cursor { bytes: b"ab\0cd", pos: 0 };
        assert_eq!(cursor.take_cstr(), Some(&b"ab"[..]));
        // "cd" has no NUL, so this is a truncated tree, not an empty string.
        assert_eq!(cursor.take_cstr(), None);
        assert_eq!(cursor.take(99), None);
    }

    /// Build a minimal but byte-exact VPK v2 in memory, so the tree walk is
    /// tested without depending on a game install.
    fn synthetic_vpk(payload: &[u8]) -> Vec<u8> {
        let mut tree = Vec::new();
        // extension → directory → filename
        tree.extend_from_slice(b"vmt\0");
        tree.extend_from_slice(b"materials/test\0");
        tree.extend_from_slice(b"wall\0");
        tree.extend_from_slice(&0xdead_beefu32.to_le_bytes()); // crc
        tree.extend_from_slice(&3u16.to_le_bytes()); // preload_bytes
        tree.extend_from_slice(&ARCHIVE_INDEX_DIR.to_le_bytes());
        tree.extend_from_slice(&0u32.to_le_bytes()); // entry_offset
        tree.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        tree.extend_from_slice(&RECORD_TERMINATOR.to_le_bytes());
        tree.extend_from_slice(b"pre"); // the preload bytes
        tree.extend_from_slice(b"\0"); // end of filenames
        tree.extend_from_slice(b"\0"); // end of directories
        tree.extend_from_slice(b"\0"); // end of extensions

        let mut out = Vec::new();
        out.extend_from_slice(&VPK_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&VPK_VERSION.to_le_bytes());
        out.extend_from_slice(&(tree.len() as u32).to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        for _ in 0..3 {
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out.extend_from_slice(&tree);
        out.extend_from_slice(payload);
        out
    }

    fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, bytes).expect("write temp vpk");
        path
    }

    #[test]
    fn reads_a_synthetic_archive_joining_preload_and_body() {
        let path = write_temp("vfs_test_synthetic_dir.vpk", &synthetic_vpk(b"body"));
        let vpk = Vpk::open(&path).expect("open");
        assert_eq!(vpk.len(), 1);
        assert!(vpk.contains("materials/test/wall.vmt"), "{:?}", vpk.paths().collect::<Vec<_>>());

        let entry = vpk.entry("materials/test/wall.vmt").expect("entry");
        assert_eq!(entry.crc, 0xdead_beef);
        assert_eq!(entry.size(), 7, "preload 3 + body 4");

        // The join is the part a reader that handles only one storage mode
        // gets wrong.
        let bytes = vpk.read("materials/test/wall.vmt").expect("read");
        assert_eq!(bytes, b"prebody");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_wrong_terminator_is_reported_rather_than_desynchronising() {
        let mut bytes = synthetic_vpk(b"body");
        // Corrupt the terminator: find the 0xffff that follows entry_length.
        let at = bytes
            .windows(2)
            .position(|w| w == RECORD_TERMINATOR.to_le_bytes())
            .expect("terminator present");
        bytes[at] = 0;
        let path = write_temp("vfs_test_badterm_dir.vpk", &bytes);
        let err = Vpk::open(&path).expect_err("must reject");
        assert!(matches!(err, VpkError::BadTerminator { .. }), "{err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_bad_signature_or_version_is_rejected() {
        let mut bytes = synthetic_vpk(b"x");
        bytes[0] = 0;
        let path = write_temp("vfs_test_badsig_dir.vpk", &bytes);
        assert!(matches!(
            Vpk::open(&path).expect_err("must reject"),
            VpkError::BadSignature { .. }
        ));
        let _ = std::fs::remove_file(&path);

        let mut bytes = synthetic_vpk(b"x");
        bytes[4] = 1; // version 1
        let path = write_temp("vfs_test_badver_dir.vpk", &bytes);
        assert!(matches!(
            Vpk::open(&path).expect_err("must reject"),
            VpkError::BadVersion { .. }
        ));
        let _ = std::fs::remove_file(&path);
    }
}
