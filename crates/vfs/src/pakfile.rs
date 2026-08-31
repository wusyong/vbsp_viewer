//! The zip embedded in a BSP's `LUMP_PAKFILE`.
//!
//! A map's custom content — the cubemap-patched materials vbsp generates, and
//! anything a community mapper packed in — lives in a plain zip archive inside
//! the BSP, mounted above every other search path while that map is loaded.
//!
//! # Why this is hand-written rather than delegated to the `zip` crate
//!
//! **Every compressed entry in a TF2 map pakfile is LZMA (method 14), and the
//! `zip` crate's LZMA path cannot read them.** Measured across all 233 shipped
//! maps: 686 974 entries compressed with method 14, 121 555 stored, and *no
//! deflate at all* — so the `zip = { features = ["deflate"] }` the plan called
//! for would have failed on every single map, and adding `"lzma"` still fails,
//! because that decoder does not hand the known output size to the LZMA stream
//! and instead tries to infer it:
//!
//! ```text
//! LzmaError("LZ distance 1537 is beyond output size 127")
//! ```
//!
//! The fix is the same one `bsp` already applies to compressed lumps: a raw
//! LZMA1 stream with no end-of-stream marker cannot be decoded without being
//! *told* how long the output is, and a zip entry's header says exactly that.
//! Supplying it turns those same bytes into valid material text. Owning ~200
//! lines here is better than depending on a decoder that is wrong for the only
//! data this project reads, and it drops a dependency tree rather than adding
//! one.
//!
//! # Layout read
//!
//! The central directory is authoritative: find `PK\x05\x06` (end of central
//! directory) from the tail, walk the `PK\x01\x02` records it points at, then
//! read payloads via each entry's local header. Walking local headers directly
//! would also work here but silently accepts a truncated archive.

use crate::normalise_path;
use std::collections::HashMap;

/// Store — no compression.
const METHOD_STORE: u16 = 0;
/// LZMA, as used by every compressed entry vbsp writes.
const METHOD_LZMA: u16 = 14;

const SIG_LOCAL: [u8; 4] = *b"PK\x03\x04";
const SIG_CENTRAL: [u8; 4] = *b"PK\x01\x02";
const SIG_EOCD: [u8; 4] = *b"PK\x05\x06";

const EOCD_LEN: usize = 22;
const LOCAL_HEADER_LEN: usize = 30;
const CENTRAL_HEADER_LEN: usize = 46;

/// Bytes of an LZMA entry's own header: version major/minor then a `u16`
/// properties length.
const ZIP_LZMA_HEADER_LEN: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum PakError {
    #[error("pakfile has no end-of-central-directory record — not a zip")]
    NoEndOfCentralDirectory,

    #[error(
        "pakfile central directory claims bytes {ofs}..{end} but the lump is {len} bytes"
    )]
    DirectoryOutOfBounds { ofs: usize, end: usize, len: usize },

    #[error("pakfile entry {index}: central directory record is truncated")]
    TruncatedCentralRecord { index: usize },

    #[error("pakfile entry `{name}`: local header at {ofs} is truncated or has no signature")]
    BadLocalHeader { name: String, ofs: usize },

    #[error("pakfile entry `{name}`: data claims bytes {ofs}..{end} but the lump is {len} bytes")]
    DataOutOfBounds {
        name: String,
        ofs: usize,
        end: usize,
        len: usize,
    },

    #[error(
        "pakfile entry `{name}`: compression method {method} is not supported \
         (TF2 maps use only {METHOD_STORE} store and {METHOD_LZMA} LZMA)"
    )]
    UnsupportedMethod { name: String, method: u16 },

    #[error("pakfile entry `{name}`: LZMA header is truncated")]
    TruncatedLzmaHeader { name: String },

    #[error("pakfile entry `{name}`: LZMA decode failed: {source}")]
    Lzma {
        name: String,
        #[source]
        source: lzma_rs::error::Error,
    },

    #[error(
        "pakfile entry `{name}`: inflated to {got} bytes but the header promised {want}"
    )]
    SizeMismatch { name: String, got: usize, want: usize },

    #[error("`{name}` is not in this pakfile")]
    NotFound { name: String },
}

pub type Result<T> = std::result::Result<T, PakError>;

/// One file in the archive.
#[derive(Clone, Copy, Debug)]
struct Entry {
    method: u16,
    compressed_size: u32,
    uncompressed_size: u32,
    /// Offset of the entry's local header within the lump.
    local_header_offset: u32,
}

/// A parsed BSP pakfile, holding the lump bytes and a name index.
pub struct Pakfile {
    bytes: Vec<u8>,
    entries: HashMap<String, Entry>,
}

impl std::fmt::Debug for Pakfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pakfile")
            .field("bytes", &self.bytes.len())
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl Pakfile {
    /// Parse the central directory of a `LUMP_PAKFILE` blob.
    ///
    /// An empty lump is an empty archive, not an error: plenty of maps pack
    /// nothing.
    pub fn parse(bytes: Vec<u8>) -> Result<Pakfile> {
        if bytes.is_empty() {
            return Ok(Pakfile {
                bytes,
                entries: HashMap::new(),
            });
        }
        let eocd = find_eocd(&bytes).ok_or(PakError::NoEndOfCentralDirectory)?;
        let count = u16::from_le_bytes(bytes[eocd + 10..eocd + 12].try_into().unwrap()) as usize;
        let dir_size = u32::from_le_bytes(bytes[eocd + 12..eocd + 16].try_into().unwrap()) as usize;
        let dir_offset =
            u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as usize;

        let dir_end = dir_offset.saturating_add(dir_size);
        if dir_end > bytes.len() {
            return Err(PakError::DirectoryOutOfBounds {
                ofs: dir_offset,
                end: dir_end,
                len: bytes.len(),
            });
        }

        let mut entries = HashMap::with_capacity(count);
        let mut pos = dir_offset;
        for index in 0..count {
            let record = bytes
                .get(pos..pos + CENTRAL_HEADER_LEN)
                .filter(|r| r[0..4] == SIG_CENTRAL)
                .ok_or(PakError::TruncatedCentralRecord { index })?;

            let method = u16::from_le_bytes(record[10..12].try_into().unwrap());
            let compressed_size = u32::from_le_bytes(record[20..24].try_into().unwrap());
            let uncompressed_size = u32::from_le_bytes(record[24..28].try_into().unwrap());
            let name_len = u16::from_le_bytes(record[28..30].try_into().unwrap()) as usize;
            let extra_len = u16::from_le_bytes(record[30..32].try_into().unwrap()) as usize;
            let comment_len = u16::from_le_bytes(record[32..34].try_into().unwrap()) as usize;
            let local_header_offset = u32::from_le_bytes(record[42..46].try_into().unwrap());

            let name_at = pos + CENTRAL_HEADER_LEN;
            let name = bytes
                .get(name_at..name_at + name_len)
                .ok_or(PakError::TruncatedCentralRecord { index })?;
            // Zip names are bytes; pakfile names are ASCII paths in practice.
            let name = normalise_path(&String::from_utf8_lossy(name));

            // A directory entry has a trailing slash and no content.
            if !name.is_empty() {
                entries.insert(
                    name,
                    Entry {
                        method,
                        compressed_size,
                        uncompressed_size,
                        local_header_offset,
                    },
                );
            }
            pos = name_at + name_len + extra_len + comment_len;
        }

        Ok(Pakfile { bytes, entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Read and decompress one entry.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let entry = *self.entries.get(name).ok_or_else(|| PakError::NotFound {
            name: name.to_string(),
        })?;

        // The local header repeats the name and extra-field lengths, and they
        // may differ from the central directory's, so the data offset has to
        // come from the local header rather than being computed from the
        // central record.
        let ofs = entry.local_header_offset as usize;
        let header = self
            .bytes
            .get(ofs..ofs + LOCAL_HEADER_LEN)
            .filter(|h| h[0..4] == SIG_LOCAL)
            .ok_or_else(|| PakError::BadLocalHeader {
                name: name.to_string(),
                ofs,
            })?;
        let name_len = u16::from_le_bytes(header[26..28].try_into().unwrap()) as usize;
        let extra_len = u16::from_le_bytes(header[28..30].try_into().unwrap()) as usize;

        let start = ofs + LOCAL_HEADER_LEN + name_len + extra_len;
        let end = start + entry.compressed_size as usize;
        let data = self
            .bytes
            .get(start..end)
            .ok_or_else(|| PakError::DataOutOfBounds {
                name: name.to_string(),
                ofs: start,
                end,
                len: self.bytes.len(),
            })?;

        match entry.method {
            METHOD_STORE => Ok(data.to_vec()),
            METHOD_LZMA => decode_lzma(name, data, entry.uncompressed_size as usize),
            method => Err(PakError::UnsupportedMethod {
                name: name.to_string(),
                method,
            }),
        }
    }
}

/// Decode a zip method-14 entry.
///
/// The entry begins with zip's own four-byte LZMA header — version major,
/// version minor, then a `u16` properties length — followed by the properties
/// and a raw LZMA1 stream. **That stream carries no length and usually no
/// end-of-stream marker**, so the output size must come from the zip header;
/// this synthesises the 13-byte "LZMA alone" header (`props ++ size as u64 LE`)
/// that `lzma_rs` expects, exactly as `bsp` does for compressed lumps.
fn decode_lzma(name: &str, data: &[u8], uncompressed_size: usize) -> Result<Vec<u8>> {
    let props_size = data
        .get(2..4)
        .map(|b| u16::from_le_bytes(b.try_into().unwrap()) as usize)
        .ok_or_else(|| PakError::TruncatedLzmaHeader {
            name: name.to_string(),
        })?;
    let props_at = ZIP_LZMA_HEADER_LEN;
    let props = data
        .get(props_at..props_at + props_size)
        .ok_or_else(|| PakError::TruncatedLzmaHeader {
            name: name.to_string(),
        })?;
    let stream = &data[props_at + props_size..];

    let mut alone = Vec::with_capacity(props.len() + 8 + stream.len());
    alone.extend_from_slice(props);
    alone.extend_from_slice(&(uncompressed_size as u64).to_le_bytes());
    alone.extend_from_slice(stream);

    let mut out = Vec::with_capacity(uncompressed_size);
    lzma_rs::lzma_decompress(&mut std::io::Cursor::new(&alone), &mut out).map_err(|source| {
        PakError::Lzma {
            name: name.to_string(),
            source,
        }
    })?;
    if out.len() != uncompressed_size {
        return Err(PakError::SizeMismatch {
            name: name.to_string(),
            got: out.len(),
            want: uncompressed_size,
        });
    }
    Ok(out)
}

/// Find the end-of-central-directory record, scanning back from the tail.
///
/// The record is 22 bytes plus a comment of up to 64 KiB, so the search is
/// bounded rather than scanning the whole archive.
fn find_eocd(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < EOCD_LEN {
        return None;
    }
    let max_comment = 0xffff + EOCD_LEN;
    let start = bytes.len().saturating_sub(max_comment);
    (start..=bytes.len() - EOCD_LEN)
        .rev()
        .find(|&i| bytes[i..i + 4] == SIG_EOCD)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored-only archive with one file, built by hand.
    fn stored_zip(name: &str, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let local_offset = out.len() as u32;

        out.extend_from_slice(&SIG_LOCAL);
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&METHOD_STORE.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // time/date
        out.extend_from_slice(&0u32.to_le_bytes()); // crc
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(body);

        let dir_offset = out.len() as u32;
        out.extend_from_slice(&SIG_CENTRAL);
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&METHOD_STORE.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // time/date
        out.extend_from_slice(&0u32.to_le_bytes()); // crc
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out.extend_from_slice(&0u16.to_le_bytes()); // disk
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        out.extend_from_slice(&local_offset.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        let dir_size = out.len() as u32 - dir_offset;

        out.extend_from_slice(&SIG_EOCD);
        out.extend_from_slice(&0u16.to_le_bytes()); // disk
        out.extend_from_slice(&0u16.to_le_bytes()); // disk with dir
        out.extend_from_slice(&1u16.to_le_bytes()); // entries on this disk
        out.extend_from_slice(&1u16.to_le_bytes()); // entries total
        out.extend_from_slice(&dir_size.to_le_bytes());
        out.extend_from_slice(&dir_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out
    }

    #[test]
    fn an_empty_lump_is_an_empty_archive() {
        let pak = Pakfile::parse(Vec::new()).expect("empty is fine");
        assert!(pak.is_empty());
        assert!(!pak.contains("anything"));
    }

    #[test]
    fn reads_a_stored_entry_and_normalises_its_name() {
        let pak = Pakfile::parse(stored_zip(
            "materials/MAPS/Foo\\Bar.VMT",
            b"\"LightmappedGeneric\" {}",
        ))
        .expect("parse");
        assert_eq!(pak.len(), 1);
        // Names arrive with mixed case and backslashes; lookups are normalised.
        assert!(
            pak.contains("materials/maps/foo/bar.vmt"),
            "{:?}",
            pak.names().collect::<Vec<_>>()
        );
        assert_eq!(
            pak.read("materials/maps/foo/bar.vmt").expect("read"),
            b"\"LightmappedGeneric\" {}"
        );
    }

    #[test]
    fn a_missing_entry_is_an_error_not_a_panic() {
        let pak = Pakfile::parse(stored_zip("a.txt", b"x")).expect("parse");
        assert!(matches!(
            pak.read("b.txt"),
            Err(PakError::NotFound { .. })
        ));
    }

    #[test]
    fn garbage_is_rejected_rather_than_misread() {
        let err = Pakfile::parse(b"not a zip at all".to_vec()).expect_err("must reject");
        assert!(matches!(err, PakError::NoEndOfCentralDirectory), "{err}");
    }

    #[test]
    fn an_unsupported_method_names_the_method() {
        let mut bytes = stored_zip("a.txt", b"x");
        // Patch the method in both headers to deflate (8), which no shipped
        // TF2 map uses.
        let central = bytes
            .windows(4)
            .position(|w| w == SIG_CENTRAL)
            .expect("central dir");
        bytes[central + 10] = 8;
        let pak = Pakfile::parse(bytes).expect("parse");
        let err = pak.read("a.txt").expect_err("must reject");
        assert!(
            matches!(err, PakError::UnsupportedMethod { method: 8, .. }),
            "{err}"
        );
    }

    #[test]
    fn eocd_is_found_past_a_trailing_comment() {
        let mut bytes = stored_zip("a.txt", b"x");
        // A comment length of 5, plus five bytes of comment.
        let len = bytes.len();
        bytes[len - 2..].copy_from_slice(&5u16.to_le_bytes());
        bytes.extend_from_slice(b"hello");
        let pak = Pakfile::parse(bytes).expect("parse with comment");
        assert_eq!(pak.len(), 1);
    }

    #[test]
    fn lzma_needs_the_size_from_the_zip_header() {
        // The regression this module exists for. A raw LZMA1 stream carries no
        // length, so decoding without the header's `uncompressed_size` either
        // fails or truncates. Round-trip through the alone-header shim with a
        // deliberately wrong size and confirm it is caught rather than
        // returning short data.
        //
        // `lzma_rs` can compress in the alone format, which is props ++ size
        // ++ stream — the same shape `decode_lzma` builds — so a zip-framed
        // entry can be synthesised from it.
        let body = b"\"patch\" { \"include\" \"materials/glass/glasswindow002c.vmt\" }".repeat(4);
        let mut alone = Vec::new();
        lzma_rs::lzma_compress(&mut std::io::Cursor::new(&body[..]), &mut alone)
            .expect("compress");
        let (props, rest) = alone.split_at(5);
        let stream = &rest[8..]; // drop the alone header's size field

        let mut entry = Vec::new();
        entry.extend_from_slice(&[9, 38]); // version major/minor, as vbsp writes
        entry.extend_from_slice(&5u16.to_le_bytes());
        entry.extend_from_slice(props);
        entry.extend_from_slice(stream);

        let decoded = decode_lzma("x.vmt", &entry, body.len()).expect("decode");
        assert_eq!(decoded, body);

        // A size larger than the stream can fill is caught: the stream runs
        // out before the promise is met.
        let err = decode_lzma("x.vmt", &entry, body.len() + 64);
        assert!(err.is_err(), "over-long size accepted: {err:?}");
    }

    #[test]
    fn a_too_small_promised_size_truncates_silently() {
        // Worth pinning as a known limit rather than implying the size is
        // validated: the length comes from the zip header, and a raw LZMA1
        // stream has nothing to check it against. Asked for fewer bytes, the
        // decoder simply stops there and reports success, so a corrupt header
        // yields short data. Every entry vbsp writes has a correct size, and
        // the CRC in the record is the only thing that could catch this — not
        // checked here because it would mean hashing every entry read.
        let body = b"0123456789".repeat(40);
        let mut alone = Vec::new();
        lzma_rs::lzma_compress(&mut std::io::Cursor::new(&body[..]), &mut alone)
            .expect("compress");
        let (props, rest) = alone.split_at(5);
        let mut entry = Vec::new();
        entry.extend_from_slice(&[9, 38]);
        entry.extend_from_slice(&5u16.to_le_bytes());
        entry.extend_from_slice(props);
        entry.extend_from_slice(&rest[8..]);

        let short = decode_lzma("x.vmt", &entry, 10).expect("decodes as far as asked");
        assert_eq!(short, &body[..10]);
    }
}
