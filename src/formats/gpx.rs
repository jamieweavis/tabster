//! Guitar Pro 6 `.gpx`: BCFZ (compressed) / BCFS file system containing `score.gpif`.
//! Owner: Phase 1 agent B.
//!
//! The container decoding is a port of alphaTab's `GpxFileSystem.ts` and `BitReader.ts`
//! (<https://github.com/CoderLine/alphaTab>, `packages/alphatab/src/importer/GpxFileSystem.ts`),
//! Copyright © CoderLine and contributors, licensed under the Mozilla Public License 2.0.
//!
//! Layout:
//! - `BCFZ` + `u32 LE` decompressed length + a bit stream. The bit stream is a sequence of chunks,
//!   each starting with a flag bit (bits are read MSB-first within each byte):
//!   - `0`: literal. A 2-bit length (read LSB-first, "reversed") followed by that many bytes
//!     (8 bits each, MSB-first, not byte-aligned).
//!   - `1`: back-reference. A 4-bit word size `w` (MSB-first), then `offset` and `size` as `w`-bit
//!     reversed integers. Copies `min(offset, size)` bytes starting `offset` bytes before the end
//!     of the output (so a reference never overlaps the bytes it produces).
//!
//!   The decompressed data starts with `BCFS`; decoding stops once the expected length is reached
//!   (or the input runs out).
//! - `BCFS` + a file system of 0x1000-byte sectors (offsets below are relative to the end of the
//!   4-byte header). Sector 0 is unused. A sector whose first `i32 LE` is `2` is a file entry:
//!   name at +0x04 (127 bytes, NUL-terminated), size at +0x8C, and from +0x94 a 0-terminated list
//!   of `i32` sector indices holding the file's data (sector `n` is at `n * 0x1000`).

use super::ParseError;
use crate::model::Score;

const SECTOR_SIZE: usize = 0x1000;
/// Refuse to decompress containers claiming to be larger than this (real files are < 1 MiB).
const MAX_DECOMPRESSED: usize = 64 * 1024 * 1024;
const SCORE_FILE: &str = "score.gpif";

pub fn parse(bytes: &[u8]) -> Result<Score, ParseError> {
    let xml = extract_gpif(bytes)?;
    super::gpif::parse_gpif(&xml)
}

/// Extract the raw `score.gpif` document from a BCFZ or BCFS container.
pub fn extract_gpif(bytes: &[u8]) -> Result<Vec<u8>, ParseError> {
    let files = read_file_system(bytes)?;
    files
        .into_iter()
        .find(|f| f.name == SCORE_FILE)
        .map(|f| f.data)
        .ok_or_else(|| ParseError::Container(format!("no {SCORE_FILE} in GPX container")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GpxFile {
    name: String,
    data: Vec<u8>,
}

/// Decode the container into its files.
fn read_file_system(bytes: &[u8]) -> Result<Vec<GpxFile>, ParseError> {
    let header = bytes
        .get(..4)
        .ok_or_else(|| ParseError::Container("GPX file too short".into()))?;
    match header {
        b"BCFZ" => {
            let decompressed = decompress(&bytes[4..])?;
            // The decompressed data includes its own 4-byte `BCFS` header; skip it.
            let fs = decompressed
                .get(4..)
                .ok_or_else(|| ParseError::Container("GPX container is truncated".into()))?;
            read_uncompressed_block(fs)
        }
        b"BCFS" => read_uncompressed_block(&bytes[4..]),
        _ => Err(ParseError::Container(
            "not a GPX container (expected BCFZ or BCFS)".into(),
        )),
    }
}

/// MSB-first bit reader (alphaTab `BitReader`).
struct BitReader<'a> {
    data: &'a [u8],
    /// Index of the next byte to load.
    pos: usize,
    current: u8,
    /// Bits of `current` already consumed (8 = need a new byte).
    bit: u8,
}

/// The bit stream ran out.
#[derive(Debug)]
struct EndOfInput;

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            current: 0,
            bit: 8,
        }
    }

    fn read_bit(&mut self) -> Result<u32, EndOfInput> {
        if self.bit >= 8 {
            self.current = *self.data.get(self.pos).ok_or(EndOfInput)?;
            self.pos += 1;
            self.bit = 0;
        }
        let value = (self.current >> (7 - self.bit)) & 1;
        self.bit += 1;
        Ok(u32::from(value))
    }

    /// `count` bits, most significant first.
    fn read_bits(&mut self, count: u32) -> Result<u32, EndOfInput> {
        let mut bits = 0;
        for i in (0..count).rev() {
            bits |= self.read_bit()? << i;
        }
        Ok(bits)
    }

    /// `count` bits, least significant first.
    fn read_bits_reversed(&mut self, count: u32) -> Result<u32, EndOfInput> {
        let mut bits = 0;
        for i in 0..count {
            bits |= self.read_bit()? << i;
        }
        Ok(bits)
    }

    fn read_byte(&mut self) -> Result<u8, EndOfInput> {
        // read_bits(8) never exceeds 0xFF.
        Ok(self.read_bits(8)? as u8)
    }
}

/// Decompress a BCFZ payload (everything after the `BCFZ` magic). The result includes the
/// decompressed `BCFS` header.
fn decompress(src: &[u8]) -> Result<Vec<u8>, ParseError> {
    let len_bytes: [u8; 4] = src
        .get(..4)
        .and_then(|b| b.try_into().ok())
        .ok_or(ParseError::Eof)?;
    let expected = i32::from_le_bytes(len_bytes);
    let expected = usize::try_from(expected)
        .map_err(|_| ParseError::Container("negative GPX length".into()))?;
    if expected > MAX_DECOMPRESSED {
        return Err(ParseError::Container(format!(
            "GPX container claims {expected} bytes, more than the {MAX_DECOMPRESSED} byte limit"
        )));
    }
    let mut reader = BitReader::new(&src[4..]);
    // Don't trust `expected` for the allocation size beyond what the input could plausibly hold.
    let mut out: Vec<u8> = Vec::with_capacity(expected.min(src.len().saturating_mul(8)));
    decompress_into(&mut reader, expected, &mut out)?;
    Ok(out)
}

fn decompress_into(
    src: &mut BitReader<'_>,
    expected: usize,
    out: &mut Vec<u8>,
) -> Result<(), ParseError> {
    // Like alphaTab, running out of input ends decompression without an error; a truncated
    // container then fails later when its file table doesn't add up.
    macro_rules! read {
        ($e:expr) => {
            match $e {
                Ok(v) => v,
                Err(EndOfInput) => return Ok(()),
            }
        };
    }
    while out.len() < expected {
        let flag = read!(src.read_bits(1));
        if flag == 1 {
            let word_size = read!(src.read_bits(4));
            let offset = read!(src.read_bits_reversed(word_size)) as usize;
            let size = read!(src.read_bits_reversed(word_size)) as usize;
            let start = out.len().checked_sub(offset).ok_or_else(|| {
                ParseError::Container("GPX back-reference points before the start".into())
            })?;
            let count = offset.min(size);
            out.extend_from_within(start..start + count);
        } else {
            let size = read!(src.read_bits_reversed(2));
            for _ in 0..size {
                out.push(read!(src.read_byte()));
            }
        }
    }
    Ok(())
}

fn get_i32(data: &[u8], offset: usize) -> Option<i32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(i32::from_le_bytes(bytes.try_into().ok()?))
}

/// NUL-terminated (or `max`-length) Latin-1 string.
fn get_string(data: &[u8], offset: usize, max: usize) -> String {
    let end = offset.saturating_add(max).min(data.len());
    data.get(offset..end)
        .unwrap_or_default()
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| char::from(b))
        .collect()
}

/// Read the BCFS sector file system (data after the `BCFS` header).
fn read_uncompressed_block(data: &[u8]) -> Result<Vec<GpxFile>, ParseError> {
    let corrupt = |msg: String| ParseError::Container(msg);
    let mut files = Vec::new();
    // Sector 0 is unused (0xFF filled); entries start at sector 1.
    let mut offset = SECTOR_SIZE;
    while offset + 3 < data.len() {
        let entry_type = get_i32(data, offset).ok_or(ParseError::Eof)?;
        if entry_type == 2 {
            let pointers = offset + 0x94;
            if pointers + 4 > data.len() {
                return Err(corrupt("GPX file entry is truncated".into()));
            }
            let name = get_string(data, offset + 0x04, 127);
            let size = get_i32(data, offset + 0x8c).ok_or(ParseError::Eof)?;
            let size = usize::try_from(size)
                .ok()
                .filter(|&s| s <= data.len())
                .ok_or_else(|| corrupt(format!("GPX file entry '{name}' has an invalid size")))?;
            let mut file_data = Vec::with_capacity(size);
            let mut index = 0usize;
            loop {
                let pointer = pointers + 4 * index;
                index += 1;
                let sector = get_i32(data, pointer).ok_or_else(|| {
                    corrupt(format!(
                        "GPX file entry '{name}' has an unterminated sector list"
                    ))
                })?;
                if sector == 0 {
                    break;
                }
                let sector_offset = usize::try_from(sector)
                    .ok()
                    .and_then(|s| s.checked_mul(SECTOR_SIZE))
                    .filter(|&o| o < data.len())
                    .ok_or_else(|| {
                        corrupt(format!(
                            "GPX file entry '{name}' points to sector {sector} outside of the container"
                        ))
                    })?;
                // The next entry follows the furthest data sector.
                offset = offset.max(sector_offset);
                if file_data.len() < size {
                    let end = (sector_offset + SECTOR_SIZE).min(data.len());
                    file_data.extend_from_slice(&data[sector_offset..end]);
                    // A short final sector is zero-padded, as in alphaTab's ByteBuffer.
                    file_data.resize(file_data.len() + SECTOR_SIZE - (end - sector_offset), 0);
                }
            }
            file_data.truncate(size);
            files.push(GpxFile {
                name,
                data: file_data,
            });
        }
        offset += SECTOR_SIZE;
    }
    Ok(files)
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Test-only BCFS builder and BCFZ encoder (the inverse of the decoder above).

    use super::SECTOR_SIZE;

    /// Build a complete `BCFS` container (including the header) holding `files`.
    pub fn build_bcfs(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut fs = vec![0xFF; SECTOR_SIZE];
        for (name, content) in files {
            let entry_sector = fs.len() / SECTOR_SIZE;
            let data_sectors = content.len().div_ceil(SECTOR_SIZE);
            let mut entry = vec![0u8; SECTOR_SIZE];
            entry[..4].copy_from_slice(&2i32.to_le_bytes());
            let name_bytes = name.as_bytes();
            entry[4..4 + name_bytes.len()].copy_from_slice(name_bytes);
            entry[0x8c..0x90].copy_from_slice(&(content.len() as i32).to_le_bytes());
            for i in 0..data_sectors {
                let sector = (entry_sector + 1 + i) as i32;
                let at = 0x94 + 4 * i;
                entry[at..at + 4].copy_from_slice(&sector.to_le_bytes());
            }
            fs.extend_from_slice(&entry);
            for chunk in content.chunks(SECTOR_SIZE) {
                let mut sector = chunk.to_vec();
                sector.resize(SECTOR_SIZE, 0);
                fs.extend_from_slice(&sector);
            }
        }
        let mut out = b"BCFS".to_vec();
        out.extend_from_slice(&fs);
        out
    }

    #[derive(Default)]
    struct BitWriter {
        out: Vec<u8>,
        used: u8,
    }

    impl BitWriter {
        fn bit(&mut self, b: u32) {
            if self.used == 0 {
                self.out.push(0);
            }
            if b & 1 == 1 {
                *self.out.last_mut().unwrap() |= 1 << (7 - self.used);
            }
            self.used = (self.used + 1) % 8;
        }
        fn bits(&mut self, value: u32, count: u32) {
            for i in (0..count).rev() {
                self.bit(value >> i);
            }
        }
        fn bits_reversed(&mut self, value: u32, count: u32) {
            for i in 0..count {
                self.bit(value >> i);
            }
        }
    }

    /// Compress `data` (which should start with `BCFS`) into a complete `BCFZ` file. With
    /// `back_refs` false only literal chunks are emitted.
    pub fn encode_bcfz(data: &[u8], back_refs: bool) -> Vec<u8> {
        const MAX_WORD: u32 = 15;
        let max_offset = (1usize << MAX_WORD) - 1;
        let mut w = BitWriter::default();
        let mut i = 0;
        while i < data.len() {
            let mut best: Option<(usize, usize)> = None; // (offset, len)
            if back_refs {
                let window = i.min(max_offset).min(4096);
                for offset in 1..=window {
                    let start = i - offset;
                    let max_len = offset.min(data.len() - i).min(max_offset);
                    let len = (0..max_len)
                        .take_while(|&k| data[start + k] == data[i + k])
                        .count();
                    if len >= 4 && best.is_none_or(|(_, l)| len > l) {
                        best = Some((offset, len));
                    }
                }
            }
            if let Some((offset, len)) = best {
                let word = usize::BITS - offset.max(len).leading_zeros();
                w.bit(1);
                w.bits(word, 4);
                w.bits_reversed(offset as u32, word);
                w.bits_reversed(len as u32, word);
                i += len;
            } else {
                let n = (data.len() - i).min(3);
                w.bit(0);
                w.bits_reversed(n as u32, 2);
                for &b in &data[i..i + n] {
                    w.bits(u32::from(b), 8);
                }
                i += n;
            }
        }
        let mut out = b"BCFZ".to_vec();
        out.extend_from_slice(&(data.len() as i32).to_le_bytes());
        out.extend_from_slice(&w.out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{build_bcfs, encode_bcfz};
    use super::*;

    const GPIF: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?>
<GPIF><GPVersion>6.0</GPVersion><Score><Title><![CDATA[Test Song]]></Title></Score>
<MasterTrack><Tracks>0</Tracks></MasterTrack><Tracks><Track id="0"><Name>Guitar</Name></Track></Tracks>
</GPIF>"#;

    fn big_gpif() -> Vec<u8> {
        // Spans several sectors and has plenty of repetition for back-references.
        let mut xml = b"<GPIF>".to_vec();
        for i in 0..400 {
            xml.extend_from_slice(
                format!("<Bar id=\"{i}\"><Voices>0 -1 -1 -1</Voices></Bar>\n").as_bytes(),
            );
        }
        xml.extend_from_slice(b"</GPIF>");
        xml
    }

    fn sample_files(gpif: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("misc.xml", b"<Misc/>".to_vec()),
            ("score.gpif", gpif.to_vec()),
            ("BinaryStylesheet", vec![0xAB; 5000]),
        ]
    }

    fn bcfs_for(gpif: &[u8]) -> Vec<u8> {
        let files = sample_files(gpif);
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (*n, d.as_slice())).collect();
        build_bcfs(&refs)
    }

    #[test]
    fn bit_reader_orders() {
        let mut r = BitReader::new(&[0b1011_0010, 0xFF]);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert_eq!(r.read_bits(3).unwrap(), 0b011);
        // Next bits are 0,0,1,0 → reversed (LSB first) = 0b0100.
        assert_eq!(r.read_bits_reversed(4).unwrap(), 0b0100);
        assert_eq!(r.read_byte().unwrap(), 0xFF);
        assert!(r.read_bit().is_err());
    }

    #[test]
    fn uncompressed_bcfs() {
        let bcfs = bcfs_for(GPIF);
        assert_eq!(extract_gpif(&bcfs).unwrap(), GPIF);
        let files = read_file_system(&bcfs).unwrap();
        let names: Vec<_> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["misc.xml", "score.gpif", "BinaryStylesheet"]);
        assert_eq!(files[2].data, vec![0xAB; 5000]);
    }

    #[test]
    fn bcfz_literal_only_round_trip() {
        let bcfs = bcfs_for(GPIF);
        let bcfz = encode_bcfz(&bcfs, false);
        assert!(bcfz.len() > bcfs.len(), "literal-only output is larger");
        assert_eq!(decompress(&bcfz[4..]).unwrap(), bcfs);
        assert_eq!(extract_gpif(&bcfz).unwrap(), GPIF);
    }

    #[test]
    fn bcfz_back_reference_round_trip() {
        let gpif = big_gpif();
        let bcfs = bcfs_for(&gpif);
        let bcfz = encode_bcfz(&bcfs, true);
        assert!(bcfz.len() < bcfs.len() / 4, "back-references compress");
        assert_eq!(decompress(&bcfz[4..]).unwrap(), bcfs);
        assert_eq!(extract_gpif(&bcfz).unwrap(), gpif);
    }

    #[test]
    fn checked_in_fixture() {
        let bytes = include_bytes!("../../testdata/gpx/minimal.gpx");
        assert!(bytes.starts_with(b"BCFZ"));
        let xml = extract_gpif(bytes).unwrap();
        assert!(xml.starts_with(b"<?xml"));
        assert!(
            std::str::from_utf8(&xml)
                .unwrap()
                .contains("Tabster GPX fixture")
        );
    }

    #[test]
    fn parse_delegates_to_gpif() {
        let bcfz = encode_bcfz(&bcfs_for(GPIF), true);
        // Whatever the GPIF parser returns, it must not be a container error.
        if let Err(ParseError::Container(e)) = parse(&bcfz) {
            panic!("container error: {e}");
        }
    }

    #[test]
    fn missing_score_file() {
        let bcfs = build_bcfs(&[("other.xml", b"<x/>")]);
        assert!(matches!(extract_gpif(&bcfs), Err(ParseError::Container(_))));
    }

    #[test]
    fn wrong_magic_and_short_input() {
        assert!(matches!(extract_gpif(b""), Err(ParseError::Container(_))));
        assert!(matches!(extract_gpif(b"BC"), Err(ParseError::Container(_))));
        assert!(matches!(
            extract_gpif(b"PK\x03\x04rest"),
            Err(ParseError::Container(_))
        ));
        assert!(extract_gpif(b"BCFZ").is_err());
        assert!(extract_gpif(b"BCFZ\x10\x00\x00\x00").is_err());
    }

    #[test]
    fn bogus_lengths() {
        // Huge declared decompressed length.
        assert!(extract_gpif(b"BCFZ\xff\xff\xff\x7f\x00").is_err());
        // Negative length.
        assert!(extract_gpif(b"BCFZ\xff\xff\xff\xff\x00").is_err());
        // Back-reference before the start of the output: flag 1, word 4, offset 15.
        let mut bad = b"BCFZ\x10\x00\x00\x00".to_vec();
        bad.extend_from_slice(&[0b1010_0111, 0b1111_1110, 0]);
        assert!(matches!(extract_gpif(&bad), Err(ParseError::Container(_))));

        // File entry with an oversized length / out-of-range sector.
        let mut bcfs = bcfs_for(GPIF);
        let entry = 4 + SECTOR_SIZE;
        let mut big = bcfs.clone();
        big[entry + 0x8c..entry + 0x90].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(extract_gpif(&big).is_err());
        bcfs[entry + 0x94..entry + 0x98].copy_from_slice(&100_000i32.to_le_bytes());
        assert!(extract_gpif(&bcfs).is_err());
        bcfs[entry + 0x94..entry + 0x98].copy_from_slice(&(-1i32).to_le_bytes());
        assert!(extract_gpif(&bcfs).is_err());
    }

    #[test]
    fn truncated_inputs_never_panic() {
        let bcfs = bcfs_for(&big_gpif());
        let bcfz = encode_bcfz(&bcfs, true);
        for container in [&bcfs, &bcfz] {
            for len in (0..container.len()).step_by(97) {
                let _ = extract_gpif(&container[..len]);
            }
        }
        // A truncated BCFZ stops decompressing early and then can't find (all of) the file.
        assert!(extract_gpif(&bcfz[..bcfz.len() / 2]).is_err());
    }

    /// Regenerate `testdata/gpx/minimal.gpx`: `cargo test write_gpx_fixture -- --ignored`.
    #[test]
    #[ignore]
    fn write_gpx_fixture() {
        let gpif = include_bytes!("../../testdata/gpx/minimal.gpif");
        let bcfs = build_bcfs(&[("score.gpif", gpif), ("misc.xml", b"<Misc/>")]);
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/gpx/minimal.gpx");
        std::fs::write(path, encode_bcfz(&bcfs, true)).unwrap();
    }

    proptest::proptest! {
        #[test]
        fn random_bytes_never_panic(body in proptest::collection::vec(proptest::num::u8::ANY, 0..2048)) {
            for magic in [b"BCFZ", b"BCFS"] {
                let mut bytes = magic.to_vec();
                bytes.extend_from_slice(&body);
                let _ = extract_gpif(&bytes);
            }
        }

        #[test]
        fn encoder_round_trips(data in proptest::collection::vec(0u8..4, 0..600), refs: bool) {
            let mut bcfs = b"BCFS".to_vec();
            bcfs.extend_from_slice(&data);
            let bcfz = encode_bcfz(&bcfs, refs);
            proptest::prop_assert_eq!(decompress(&bcfz[4..]).unwrap(), bcfs);
        }
    }
}
