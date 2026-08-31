//! Reader for Valve Source engine BSP (`VBSP`) map files, targeting the
//! version 20 maps that ship with Team Fortress 2.
//!
//! The file is memory-mapped and lumps are reinterpreted in place, so opening
//! a map costs almost nothing until a lump is actually touched. The one
//! unavoidable copy is decompression: **most lumps in a modern TF2 map are
//! LZMA-compressed** (48 of 64 in `cp_badlands.bsp`), signalled by a non-zero
//! [`raw::Lump::uncompressed_size`]. Those are inflated on first access and
//! cached for the lifetime of the [`Bsp`].
//!
//! ```no_run
//! use bsp::{Bsp, LumpId};
//! let bsp = Bsp::open("cp_badlands.bsp")?;
//! println!("{} faces", bsp.faces()?.len());
//! # Ok::<(), bsp::BspError>(())
//! ```

pub mod displacement;
pub mod flags;
pub mod geometry;
pub mod lump;
pub mod raw;

pub use lump::LumpId;

use bytemuck::Pod;
use memmap2::Mmap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Little-endian `"VBSP"`.
pub const VBSP_IDENT: i32 = i32::from_le_bytes(*b"VBSP");

/// `BSPVERSION` in `bspfile.h`. TF2 ships version 20.
pub const BSP_VERSION: i32 = 20;
/// `MINBSPVERSION` — the oldest version the engine will load.
pub const MIN_BSP_VERSION: i32 = 19;
/// Newest version accepted here. 21 covers Portal 2 / CS:GO era maps.
pub const MAX_BSP_VERSION: i32 = 21;

/// Size of Valve's `lzma_header_t`: magic, `actualSize`, `lzmaSize`, 5 props.
const LZMA_HEADER_LEN: usize = 17;

#[derive(Debug, thiserror::Error)]
pub enum BspError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("not a VBSP file: expected ident {VBSP_IDENT:#010x}, found {found:#010x}")]
    BadIdent { found: i32 },

    #[error(
        "unsupported BSP version {found} (this reader handles \
         {MIN_BSP_VERSION}..={MAX_BSP_VERSION})"
    )]
    UnsupportedVersion { found: i32 },

    #[error("file is {len} bytes, too small to hold a {expected}-byte BSP header")]
    TooSmall { len: usize, expected: usize },

    #[error(
        "lump {lump} claims bytes {ofs}..{end} but the file is only {len} bytes"
    )]
    LumpOutOfBounds {
        lump: &'static str,
        ofs: i64,
        end: i64,
        len: usize,
    },

    #[error("lump {lump} is marked compressed but has no LZMA header")]
    MissingLzmaHeader { lump: &'static str },

    #[error("lump {lump}: LZMA decode failed: {source}")]
    Lzma {
        lump: &'static str,
        #[source]
        source: lzma_rs::error::Error,
    },

    #[error(
        "lump {lump}: inflated to {got} bytes but the header promised {want}"
    )]
    LzmaSizeMismatch {
        lump: &'static str,
        got: usize,
        want: usize,
    },

    #[error(
        "lump {lump}: {len} bytes is not a whole number of {ty} \
         ({stride} bytes each) — leftover {rem}"
    )]
    BadStride {
        lump: &'static str,
        ty: &'static str,
        len: usize,
        stride: usize,
        rem: usize,
    },

    /// `bytemuck::PodCastError` does not implement `std::error::Error` unless
    /// bytemuck's std feature is on, so it is kept as text.
    #[error("lump {lump}: cannot reinterpret as {ty}: {reason}")]
    BadCast {
        lump: &'static str,
        ty: &'static str,
        reason: String,
    },

    #[error("lump {lump}: index {index} out of range (len {len})")]
    IndexOutOfRange {
        lump: &'static str,
        index: i64,
        len: usize,
    },

    #[error("lump {lump}: string at offset {ofs} is not NUL-terminated")]
    UnterminatedString { lump: &'static str, ofs: usize },
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, BspError>;

/// A byte buffer guaranteed to be 8-byte aligned, so lumps materialised on the
/// heap can still be cast to `Pod` slices without a second copy.
///
/// `Vec<u8>` only promises alignment 1; in practice the allocator returns
/// something wider, but "in practice" is not a good foundation for a
/// zero-copy cast, so the backing store is `Vec<u64>`.
struct AlignedBuf {
    words: Vec<u64>,
    len: usize,
}

impl AlignedBuf {
    fn zeroed(len: usize) -> Self {
        Self {
            words: vec![0u64; len.div_ceil(8)],
            len,
        }
    }

    fn from_slice(src: &[u8]) -> Self {
        let mut buf = Self::zeroed(src.len());
        buf.as_bytes_mut().copy_from_slice(src);
        buf
    }

    fn as_bytes(&self) -> &[u8] {
        &bytemuck::cast_slice(&self.words)[..self.len]
    }

    fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut bytemuck::cast_slice_mut(&mut self.words)[..self.len]
    }
}

/// `io::Write` into a fixed-size buffer, so LZMA output lands directly in its
/// final allocation and an over-long stream is caught rather than reallocated.
struct FixedWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl Write for FixedWriter<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let room = self.buf.len() - self.pos;
        if data.len() > room {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "LZMA stream is longer than the declared uncompressed size",
            ));
        }
        self.buf[self.pos..self.pos + data.len()].copy_from_slice(data);
        self.pos += data.len();
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// An opened BSP file.
pub struct Bsp {
    path: PathBuf,
    mmap: Mmap,
    header: raw::Header,
    /// Lazily materialised lump bodies: decompressed lumps, plus any lump that
    /// needed realigning for a `Pod` cast.
    cache: [OnceLock<AlignedBuf>; raw::HEADER_LUMPS],
}

impl Bsp {
    /// Memory-map and validate a BSP file.
    ///
    /// Only the header is read here; lump bodies are decoded on demand.
    pub fn open(path: impl AsRef<Path>) -> Result<Bsp> {
        let path = path.as_ref().to_path_buf();
        let io_err = |source| BspError::Io {
            path: path.clone(),
            source,
        };

        let file = std::fs::File::open(&path).map_err(io_err)?;
        // SAFETY: the map is read-only and we never hand out a `&mut` to it.
        // If another process truncates the file while it is mapped, reads can
        // fault — the same caveat the engine lives with.
        let mmap = unsafe { Mmap::map(&file) }.map_err(io_err)?;

        let want = size_of::<raw::Header>();
        if mmap.len() < want {
            return Err(BspError::TooSmall {
                len: mmap.len(),
                expected: want,
            });
        }

        // Copy the header out rather than borrowing: it is 1 KiB, and an owned
        // copy keeps `Bsp` free of a self-referential borrow.
        let header: raw::Header = bytemuck::pod_read_unaligned(&mmap[..want]);

        if header.ident != VBSP_IDENT {
            return Err(BspError::BadIdent {
                found: header.ident,
            });
        }
        if !(MIN_BSP_VERSION..=MAX_BSP_VERSION).contains(&header.version) {
            return Err(BspError::UnsupportedVersion {
                found: header.version,
            });
        }

        Ok(Bsp {
            path,
            mmap,
            header,
            cache: std::array::from_fn(|_| OnceLock::new()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn header(&self) -> &raw::Header {
        &self.header
    }

    pub fn version(&self) -> i32 {
        self.header.version
    }

    pub fn map_revision(&self) -> i32 {
        self.header.map_revision
    }

    /// Total file size in bytes.
    pub fn file_len(&self) -> usize {
        self.mmap.len()
    }

    /// The directory entry for one lump.
    pub fn lump(&self, id: LumpId) -> &raw::Lump {
        &self.header.lumps[id.index()]
    }

    /// Whether this lump's body is LZMA-compressed on disk.
    pub fn is_compressed(&self, id: LumpId) -> bool {
        self.lump(id).uncompressed_size != 0
    }

    /// Inflated length of a lump, without decoding it.
    pub fn lump_len(&self, id: LumpId) -> usize {
        let l = self.lump(id);
        if l.uncompressed_size != 0 {
            l.uncompressed_size.max(0) as usize
        } else {
            l.filelen.max(0) as usize
        }
    }

    /// The raw bytes of one lump slice of the mapped file, bounds-checked.
    fn file_slice(&self, id: LumpId) -> Result<&[u8]> {
        let l = self.lump(id);
        let (ofs, len) = (l.fileofs as i64, l.filelen as i64);
        let end = ofs.saturating_add(len);
        if ofs < 0 || len < 0 || end as usize > self.mmap.len() {
            return Err(BspError::LumpOutOfBounds {
                lump: id.name(),
                ofs,
                end,
                len: self.mmap.len(),
            });
        }
        Ok(&self.mmap[ofs as usize..end as usize])
    }

    /// Lump body, decompressing on first access if needed.
    ///
    /// The returned slice points either into the memory map or into this
    /// `Bsp`'s decompression cache, so it costs nothing to call repeatedly.
    pub fn lump_bytes(&self, id: LumpId) -> Result<&[u8]> {
        // A cached body always wins: it is either the inflated data or an
        // aligned copy made for a `Pod` cast.
        if let Some(buf) = self.cache[id.index()].get() {
            return Ok(buf.as_bytes());
        }

        let l = self.lump(id);
        if l.filelen == 0 {
            return Ok(&[]);
        }

        let raw_bytes = self.file_slice(id)?;
        if l.uncompressed_size == 0 {
            return Ok(raw_bytes);
        }

        let inflated = self.inflate(id, raw_bytes)?;
        let _ = self.cache[id.index()].set(inflated);
        // `set` only fails if another thread won the race, in which case its
        // value is byte-identical to ours.
        Ok(self.cache[id.index()].get().expect("just set").as_bytes())
    }

    /// Decode Valve's LZMA lump wrapper.
    ///
    /// The body is `lzma_header_t` — `"LZMA"`, `actualSize`, `lzmaSize`, five
    /// property bytes — followed by a raw LZMA1 stream with **no end marker**,
    /// so the output length has to be supplied out of band. Prepending the
    /// props and size in the standard 13-byte "LZMA alone" order lets a
    /// stock decoder read it.
    fn inflate(&self, id: LumpId, body: &[u8]) -> Result<AlignedBuf> {
        if body.len() < LZMA_HEADER_LEN || &body[..4] != b"LZMA" {
            return Err(BspError::MissingLzmaHeader { lump: id.name() });
        }

        let actual_size = u32::from_le_bytes(body[4..8].try_into().unwrap()) as usize;
        let lzma_size = u32::from_le_bytes(body[8..12].try_into().unwrap()) as usize;
        let props: [u8; 5] = body[12..17].try_into().unwrap();

        // Trust `lzmaSize` over `filelen`, which can include tail padding.
        let payload_end = (LZMA_HEADER_LEN + lzma_size).min(body.len());
        let payload = &body[LZMA_HEADER_LEN..payload_end];

        let mut alone = [0u8; 13];
        alone[..5].copy_from_slice(&props);
        alone[5..].copy_from_slice(&(actual_size as u64).to_le_bytes());

        let mut out = AlignedBuf::zeroed(actual_size);
        let mut sink = FixedWriter {
            buf: out.as_bytes_mut(),
            pos: 0,
        };
        let mut src = io::Read::chain(&alone[..], payload);

        lzma_rs::lzma_decompress(&mut src, &mut sink).map_err(|source| BspError::Lzma {
            lump: id.name(),
            source,
        })?;

        if sink.pos != actual_size {
            return Err(BspError::LzmaSizeMismatch {
                lump: id.name(),
                got: sink.pos,
                want: actual_size,
            });
        }
        Ok(out)
    }

    /// Reinterpret a lump as a slice of `T`.
    ///
    /// Errors if the length is not a whole multiple of `size_of::<T>()`, which
    /// is the check that catches a struct transcribed with wrong padding.
    pub fn lump_slice<T: Pod>(&self, id: LumpId) -> Result<&[T]> {
        // An absent lump is an empty slice, not an error — plenty of maps have
        // no displacements, no cubemaps, or no HDR data. Returning early also
        // keeps `cast_slice` away from the dangling pointer behind an empty
        // `&[u8]`, which is only 1-aligned and would fail the cast.
        if self.lump_bytes(id)?.is_empty() {
            return Ok(&[]);
        }

        // If the bytes live in the memory map at an offset that does not suit
        // `T`, materialise an aligned copy. Valve aligns lumps to 4 bytes so
        // this is not expected to fire, but a cast is not the place to guess.
        let misaligned = {
            let bytes = self.lump_bytes(id)?;
            !(bytes.as_ptr() as usize).is_multiple_of(align_of::<T>())
        };
        if misaligned {
            let copy = AlignedBuf::from_slice(self.lump_bytes(id)?);
            let _ = self.cache[id.index()].set(copy);
        }

        let bytes = self.lump_bytes(id)?;
        let stride = size_of::<T>();
        let rem = bytes.len() % stride;
        if rem != 0 {
            return Err(BspError::BadStride {
                lump: id.name(),
                ty: std::any::type_name::<T>(),
                len: bytes.len(),
                stride,
                rem,
            });
        }

        // `try_cast_slice` rather than `cast_slice`: a malformed map should
        // surface as an error, never as a panic inside a library.
        bytemuck::try_cast_slice(bytes).map_err(|e| BspError::BadCast {
            lump: id.name(),
            ty: std::any::type_name::<T>(),
            reason: format!("{e:?}"),
        })
    }

    // -- Typed lump accessors -------------------------------------------------

    pub fn planes(&self) -> Result<&[raw::DPlane]> {
        self.lump_slice(LumpId::Planes)
    }

    pub fn vertices(&self) -> Result<&[raw::DVertex]> {
        self.lump_slice(LumpId::Vertexes)
    }

    pub fn edges(&self) -> Result<&[raw::DEdge]> {
        self.lump_slice(LumpId::Edges)
    }

    /// Signed indices into [`Bsp::edges`]; negative means traverse the edge
    /// backwards. Index 0 is never used.
    pub fn surfedges(&self) -> Result<&[i32]> {
        self.lump_slice(LumpId::SurfEdges)
    }

    pub fn faces(&self) -> Result<&[raw::DFace]> {
        self.lump_slice(LumpId::Faces)
    }

    /// Faces as compiled for HDR lighting. Empty on maps without HDR.
    pub fn faces_hdr(&self) -> Result<&[raw::DFace]> {
        self.lump_slice(LumpId::FacesHdr)
    }

    pub fn nodes(&self) -> Result<&[raw::DNode]> {
        self.lump_slice(LumpId::Nodes)
    }

    /// Leaves, for lump version 1 (every TF2 map). Use [`Bsp::leaves_v0`] if
    /// `self.lump(LumpId::Leafs).version == 0`.
    pub fn leaves(&self) -> Result<&[raw::DLeaf]> {
        self.lump_slice(LumpId::Leafs)
    }

    pub fn leaves_v0(&self) -> Result<&[raw::DLeafV0]> {
        self.lump_slice(LumpId::Leafs)
    }

    pub fn texinfos(&self) -> Result<&[raw::TexInfo]> {
        self.lump_slice(LumpId::TexInfo)
    }

    pub fn texdatas(&self) -> Result<&[raw::DTexData]> {
        self.lump_slice(LumpId::TexData)
    }

    /// Model 0 is worldspawn; 1.. are brush entities.
    pub fn models(&self) -> Result<&[raw::DModel]> {
        self.lump_slice(LumpId::Models)
    }

    pub fn brushes(&self) -> Result<&[raw::DBrush]> {
        self.lump_slice(LumpId::Brushes)
    }

    pub fn brush_sides(&self) -> Result<&[raw::DBrushSide]> {
        self.lump_slice(LumpId::BrushSides)
    }

    pub fn dispinfos(&self) -> Result<&[raw::DDispInfo]> {
        self.lump_slice(LumpId::DispInfo)
    }

    pub fn disp_verts(&self) -> Result<&[raw::DispVert]> {
        self.lump_slice(LumpId::DispVerts)
    }

    pub fn disp_tris(&self) -> Result<&[raw::DispTri]> {
        self.lump_slice(LumpId::DispTris)
    }

    pub fn leaf_faces(&self) -> Result<&[u16]> {
        self.lump_slice(LumpId::LeafFaces)
    }

    pub fn cubemaps(&self) -> Result<&[raw::DCubemapSample]> {
        self.lump_slice(LumpId::Cubemaps)
    }

    /// LDR lightmap samples.
    pub fn lighting(&self) -> Result<&[raw::ColorRgbExp32]> {
        self.lump_slice(LumpId::Lighting)
    }

    /// HDR lightmap samples. Empty on maps compiled without HDR.
    pub fn lighting_hdr(&self) -> Result<&[raw::ColorRgbExp32]> {
        self.lump_slice(LumpId::LightingHdr)
    }

    /// Byte offsets into [`Bsp::texdata_string_data`].
    pub fn texdata_string_table(&self) -> Result<&[i32]> {
        self.lump_slice(LumpId::TexDataStringTable)
    }

    /// The NUL-separated blob of material names.
    pub fn texdata_string_data(&self) -> Result<&[u8]> {
        self.lump_bytes(LumpId::TexDataStringData)
    }

    /// The material path for a texdata index, e.g. `CONCRETE/CONCRETEWALL001`.
    ///
    /// Names are stored without the `materials/` prefix or `.vmt` suffix, and
    /// in whatever case the mapper used.
    pub fn texture_name(&self, texdata_index: usize) -> Result<&str> {
        let texdatas = self.texdatas()?;
        let td = texdatas
            .get(texdata_index)
            .ok_or(BspError::IndexOutOfRange {
                lump: LumpId::TexData.name(),
                index: texdata_index as i64,
                len: texdatas.len(),
            })?;

        let table = self.texdata_string_table()?;
        let id = td.name_string_table_id;
        let ofs = table
            .get(usize::try_from(id).unwrap_or(usize::MAX))
            .copied()
            .ok_or(BspError::IndexOutOfRange {
                lump: LumpId::TexDataStringTable.name(),
                index: id as i64,
                len: table.len(),
            })?;

        let data = self.texdata_string_data()?;
        let start = usize::try_from(ofs).unwrap_or(usize::MAX);
        let tail = data.get(start..).ok_or(BspError::IndexOutOfRange {
            lump: LumpId::TexDataStringData.name(),
            index: ofs as i64,
            len: data.len(),
        })?;
        let end = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or(BspError::UnterminatedString {
                lump: LumpId::TexDataStringData.name(),
                ofs: start,
            })?;

        // Material paths are ASCII in practice; keep the lossy path rather
        // than failing a whole map over one odd byte.
        Ok(std::str::from_utf8(&tail[..end]).unwrap_or(""))
    }

    /// Every material name in the map, indexed by texdata.
    pub fn texture_names(&self) -> Result<Vec<&str>> {
        (0..self.texdatas()?.len())
            .map(|i| self.texture_name(i))
            .collect()
    }

    /// The ENTITIES lump as text — a KeyValues document of `{ ... }` blocks.
    pub fn entities_str(&self) -> Result<&str> {
        let bytes = self.lump_bytes(LumpId::Entities)?;
        // The lump is NUL-terminated; trim it and anything after.
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Ok(std::str::from_utf8(&bytes[..end]).unwrap_or(""))
    }

    /// The GAME_LUMP directory. Static props live under id `sprp` (phase 2).
    pub fn game_lumps(&self) -> Result<Vec<raw::DGameLump>> {
        let bytes = self.lump_bytes(LumpId::GameLump)?;
        if bytes.len() < 4 {
            return Ok(Vec::new());
        }
        let count = i32::from_le_bytes(bytes[..4].try_into().unwrap()).max(0) as usize;
        let stride = size_of::<raw::DGameLump>();
        let available = (bytes.len() - 4) / stride;
        Ok((0..count.min(available))
            .map(|i| {
                let at = 4 + i * stride;
                bytemuck::pod_read_unaligned(&bytes[at..at + stride])
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The struct sizes are asserted at compile time in `raw.rs`; this checks
    /// the derived grid maths that the displacement builder depends on.
    #[test]
    fn displacement_grid_sizes() {
        for (power, verts, tris, side) in [(2, 25, 32, 5), (3, 81, 128, 9), (4, 289, 512, 17)] {
            let d = raw::DDispInfo {
                power,
                ..bytemuck::Zeroable::zeroed()
            };
            assert_eq!(d.side_len(), side, "power {power} side");
            assert_eq!(d.num_verts(), verts, "power {power} verts");
            assert_eq!(d.num_tris(), tris, "power {power} tris");
        }
    }

    #[test]
    fn ident_is_vbsp_in_file_order() {
        assert_eq!(VBSP_IDENT.to_le_bytes(), *b"VBSP");
    }

    #[test]
    fn lump_id_table_is_in_order() {
        for (i, id) in LumpId::ALL.iter().enumerate() {
            assert_eq!(id.index(), i, "{} is at the wrong index", id.name());
        }
    }

    /// Write a header-only BSP: valid, but with every lump zero-length.
    fn write_empty_bsp(name: &str) -> PathBuf {
        let mut header: raw::Header = bytemuck::Zeroable::zeroed();
        header.ident = VBSP_IDENT;
        header.version = BSP_VERSION;
        header.map_revision = 1;

        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, bytemuck::bytes_of(&header)).expect("write temp bsp");
        path
    }

    /// A map with no displacements has a zero-length DISPINFO lump, and an
    /// empty `&[u8]` carries a 1-aligned dangling pointer — which used to make
    /// the `Pod` cast fail rather than yield an empty slice. Every accessor
    /// must return empty data, not an error and not a panic.
    #[test]
    fn absent_lumps_read_as_empty_slices() {
        let path = write_empty_bsp("bsp_empty_lumps_test.bsp");
        let bsp = Bsp::open(&path).expect("header-only bsp should open");

        assert_eq!(bsp.version(), BSP_VERSION);
        assert!(bsp.faces().expect("faces").is_empty());
        assert!(bsp.vertices().expect("vertices").is_empty());
        assert!(bsp.edges().expect("edges").is_empty());
        assert!(bsp.surfedges().expect("surfedges").is_empty());
        assert!(bsp.planes().expect("planes").is_empty());
        assert!(bsp.dispinfos().expect("dispinfos").is_empty());
        assert!(bsp.disp_verts().expect("disp_verts").is_empty());
        assert!(bsp.disp_tris().expect("disp_tris").is_empty());
        assert!(bsp.texinfos().expect("texinfos").is_empty());
        assert!(bsp.texdatas().expect("texdatas").is_empty());
        assert!(bsp.models().expect("models").is_empty());
        assert!(bsp.leaves().expect("leaves").is_empty());
        assert!(bsp.lighting().expect("lighting").is_empty());
        assert!(bsp.cubemaps().expect("cubemaps").is_empty());
        assert!(bsp.texture_names().expect("texture_names").is_empty());
        assert_eq!(bsp.entities_str().expect("entities"), "");
        assert!(bsp.game_lumps().expect("game_lumps").is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_non_vbsp_and_bad_versions() {
        let path = std::env::temp_dir().join("bsp_bad_ident_test.bsp");
        std::fs::write(&path, [0u8; 2048]).expect("write");
        assert!(matches!(
            Bsp::open(&path),
            Err(BspError::BadIdent { found: 0 })
        ));

        let mut header: raw::Header = bytemuck::Zeroable::zeroed();
        header.ident = VBSP_IDENT;
        header.version = 17;
        std::fs::write(&path, bytemuck::bytes_of(&header)).expect("write");
        assert!(matches!(
            Bsp::open(&path),
            Err(BspError::UnsupportedVersion { found: 17 })
        ));

        // Too short to even hold a header.
        std::fs::write(&path, [0u8; 8]).expect("write");
        assert!(matches!(Bsp::open(&path), Err(BspError::TooSmall { .. })));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn leaf_stride_follows_lump_version() {
        assert_eq!(LumpId::Leafs.stride(0), Some(56));
        assert_eq!(LumpId::Leafs.stride(1), Some(32));
    }

    #[test]
    fn luxel_decode_matches_exponent_scaling() {
        let c = raw::ColorRgbExp32 {
            r: 255,
            g: 128,
            b: 0,
            exponent: 0,
        };
        let [r, g, b] = c.to_linear();
        assert!((r - 1.0).abs() < 1e-6);
        assert!((g - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(b, 0.0);

        // One stop up doubles the value.
        let bright = raw::ColorRgbExp32 { exponent: 1, ..c };
        assert!((bright.to_linear()[0] - 2.0).abs() < 1e-6);
    }
}
