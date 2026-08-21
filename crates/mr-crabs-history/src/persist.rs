//! Bounded, versioned history persistence (S8: `history-persistence`).
//!
//! [`HistoryFile`] captures the paged scrollback into a self-contained,
//! versioned binary format with an explicit byte cap and an explicit line
//! cap. Loading rejects corrupt, version-mismatched, oversized, and
//! truncated payloads before any allocation proportional to the payload.
//!
//! Format (all integers little-endian):
//!
//! ```text
//! magic    "MCH8"                      4 bytes
//! version  u32                         2
//! flags    u8                          bit 0: payload chunks are LZ4 blocks
//! chunks   u32                         number of line chunks
//! crc32    u32                         IEEE CRC-32 of the payload region
//! payload:
//!   styles   u32                       number of engine-global styles
//!   stylelen u32                       JSON byte length of the style table
//!   stylejson stylelen bytes           complete engine-global style table
//!   per chunk:
//!     lines  u32                       lines in this chunk (<= 1024)
//!     cols   u16 * lines               width of each line
//!     cells  u32                       total cells in this chunk
//!     len    u32                       payload byte length
//!     data   len bytes                 cells * 8 bytes, raw or LZ4 block
//! ```
//!
//! Every decode path is size-checked before reading; chunk counts and cell
//! counts are validated against the configured caps before decompression.

use mr_crabs_terminal::{Cell, Style, Terminal};

/// File-format magic.
pub const PERSIST_MAGIC: &[u8; 4] = b"MCH8";
/// File-format version. Bump on any incompatible layout change.
pub const PERSIST_VERSION: u32 = 2;
/// Maximum lines per chunk (bounds per-chunk headers).
pub const PERSIST_CHUNK_LINES: u32 = 1024;
/// Default maximum encoded file size.
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;
/// Default maximum persisted lines.
pub const DEFAULT_MAX_LINES: usize = 1_000_000;

#[derive(Clone, Copy, Debug)]
pub struct PersistConfig {
    /// Hard cap on the encoded file size; encode/decode fail with
    /// [`PersistError::TooLarge`] beyond it.
    pub max_bytes: usize,
    /// Hard cap on persisted lines; capture/decode fail with
    /// [`PersistError::TooManyLines`] beyond it.
    pub max_lines: usize,
    /// Compress chunk payloads with LZ4 blocks.
    pub compress: bool,
}

impl Default for PersistConfig {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
            max_lines: DEFAULT_MAX_LINES,
            compress: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersistError {
    /// Encoded size exceeds the configured cap.
    TooLarge,
    /// Line count exceeds the configured cap.
    TooManyLines,
    /// The payload carries a different format version.
    VersionMismatch(u32),
    /// Magic mismatch or structurally invalid payload.
    BadMagic,
    /// CRC mismatch or decompression failure.
    Corrupt,
    /// The payload ends before its declared length.
    Truncated,
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "history file exceeds the configured size cap"),
            Self::TooManyLines => write!(f, "history file exceeds the configured line cap"),
            Self::VersionMismatch(version) => {
                write!(f, "history file version {version} is not supported")
            }
            Self::BadMagic => write!(f, "history file magic mismatch"),
            Self::Corrupt => write!(f, "history file payload is corrupt"),
            Self::Truncated => write!(f, "history file payload is truncated"),
        }
    }
}

impl std::error::Error for PersistError {}

/// A captured history: per-line widths and cells plus format metadata.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HistoryFile {
    pub version: u32,
    /// Complete engine-global style table for every persisted cell ID.
    pub styles: Vec<Style>,
    pub cols: Vec<u16>,
    pub lines: Vec<Vec<Cell>>,
}

impl HistoryFile {
    /// Capture the full scrollback through the read contract.
    pub fn capture(
        term: &mut Terminal,
        config: &PersistConfig,
    ) -> Result<Self, PersistError> {
        let history_len = term.history_len();
        if history_len > config.max_lines {
            return Err(PersistError::TooManyLines);
        }
        let mut cols = Vec::with_capacity(history_len);
        let mut lines = Vec::with_capacity(history_len);
        for index in 0..history_len {
            let width = term
                .history_line_cols(index)
                .ok_or(PersistError::Corrupt)?;
            let width = u16::try_from(width).map_err(|_| PersistError::Corrupt)?;
            let mut cells = Vec::new();
            if !term.read_history_line(index, &mut cells)
                || cells.len() != usize::from(width)
            {
                return Err(PersistError::Corrupt);
            }
            cols.push(width);
            lines.push(cells);
        }
        let styles = term.global_styles().to_vec();
        validate_styles(&styles)?;
        if lines
            .iter()
            .flatten()
            .any(|cell| usize::from(cell.style) >= styles.len())
        {
            return Err(PersistError::Corrupt);
        }
        Ok(Self {
            version: PERSIST_VERSION,
            styles,
            cols,
            lines,
        })
    }

    /// Encode to the versioned binary format; fails with
    /// [`PersistError::TooLarge`] once the encoded size exceeds the cap.
    pub fn encode(&self, config: &PersistConfig) -> Result<Vec<u8>, PersistError> {
        if self.version != PERSIST_VERSION {
            return Err(PersistError::VersionMismatch(self.version));
        }
        validate_styles(&self.styles)?;
        if self.lines.len() > config.max_lines || self.lines.len() != self.cols.len() {
            return Err(if self.lines.len() > config.max_lines {
                PersistError::TooManyLines
            } else {
                PersistError::Corrupt
            });
        }
        for (&width, cells) in self.cols.iter().zip(&self.lines) {
            if cells.len() != usize::from(width)
                || cells
                    .iter()
                    .any(|cell| usize::from(cell.style) >= self.styles.len())
            {
                return Err(PersistError::Corrupt);
            }
        }

        let chunk_count = u32::try_from(self.lines.len().div_ceil(PERSIST_CHUNK_LINES as usize))
            .map_err(|_| PersistError::TooLarge)?;
        let style_count = u32::try_from(self.styles.len()).map_err(|_| PersistError::TooLarge)?;
        let style_json = serde_json::to_vec(&self.styles).map_err(|_| PersistError::Corrupt)?;
        let style_json_len =
            u32::try_from(style_json.len()).map_err(|_| PersistError::TooLarge)?;

        // Header: magic(4) + version(4) + flags(1) + chunks(4) + crc(4).
        const HEADER_LEN: usize = 17;
        let initial_len = HEADER_LEN
            .checked_add(8)
            .and_then(|len| len.checked_add(style_json.len()))
            .ok_or(PersistError::TooLarge)?;
        if initial_len > config.max_bytes {
            return Err(PersistError::TooLarge);
        }
        let mut out = Vec::with_capacity(initial_len);
        out.extend_from_slice(PERSIST_MAGIC);
        out.extend_from_slice(&PERSIST_VERSION.to_le_bytes());
        out.push(if config.compress { 1 } else { 0 });
        out.extend_from_slice(&chunk_count.to_le_bytes());
        // CRC placeholder, filled after the payload is appended.
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&style_count.to_le_bytes());
        out.extend_from_slice(&style_json_len.to_le_bytes());
        out.extend_from_slice(&style_json);

        for (chunk_index, chunk) in self.lines.chunks(PERSIST_CHUNK_LINES as usize).enumerate() {
            let line_base = chunk_index
                .checked_mul(PERSIST_CHUNK_LINES as usize)
                .ok_or(PersistError::TooLarge)?;
            let line_count = u32::try_from(chunk.len()).map_err(|_| PersistError::TooLarge)?;
            let cell_count = chunk.iter().try_fold(0usize, |count, cells| {
                count.checked_add(cells.len()).ok_or(PersistError::TooLarge)
            })?;
            let cell_count_u32 = u32::try_from(cell_count).map_err(|_| PersistError::TooLarge)?;
            let payload_bytes = cell_count
                .checked_mul(std::mem::size_of::<Cell>())
                .ok_or(PersistError::TooLarge)?;
            let mut payload = Vec::with_capacity(payload_bytes);
            for (line, cells) in chunk.iter().enumerate() {
                if cells.len() != usize::from(self.cols[line_base + line]) {
                    return Err(PersistError::Corrupt);
                }
                payload.extend_from_slice(cell_bytes(cells));
            }
            let payload = if config.compress {
                lz4_flex::block::compress(&payload)
            } else {
                payload
            };
            let len = u32::try_from(payload.len()).map_err(|_| PersistError::TooLarge)?;
            // Chunk header budget: lines + cols + cells + len.
            let header_budget = chunk
                .len()
                .checked_mul(2)
                .and_then(|cols_len| 4usize.checked_add(cols_len))
                .and_then(|len| len.checked_add(4))
                .and_then(|len| len.checked_add(4))
                .ok_or(PersistError::TooLarge)?;
            let encoded_len = out
                .len()
                .checked_add(header_budget)
                .and_then(|total| total.checked_add(payload.len()))
                .ok_or(PersistError::TooLarge)?;
            if encoded_len > config.max_bytes {
                return Err(PersistError::TooLarge);
            }
            out.extend_from_slice(&line_count.to_le_bytes());
            for line in 0..chunk.len() {
                out.extend_from_slice(&self.cols[line_base + line].to_le_bytes());
            }
            out.extend_from_slice(&cell_count_u32.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&payload);
        }

        // CRC covers the complete style table and every history chunk.
        let crc = crc32(&out[HEADER_LEN..]);
        out[13..17].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    /// Decode and validate a persisted file. Rejects oversized payloads
    /// before parsing, then validates magic, version, and every chunk bound
    /// (including the declared line widths, which bound the decompression
    /// buffer) before decompressing; the CRC over the payload region is
    /// validated once the structure has been walked, so truncated payloads
    /// classify as [`PersistError::Truncated`] rather than `Corrupt`.
    pub fn decode(bytes: &[u8], config: &PersistConfig) -> Result<Self, PersistError> {
        if bytes.len() > config.max_bytes {
            return Err(PersistError::TooLarge);
        }
        if bytes.len() < 17 {
            return Err(PersistError::Truncated);
        }
        if &bytes[0..4] != PERSIST_MAGIC {
            return Err(PersistError::BadMagic);
        }
        let version = u32::from_le_bytes(bytes[4..8].try_into().expect("slice"));
        if version != PERSIST_VERSION {
            return Err(PersistError::VersionMismatch(version));
        }
        let compressed = bytes[8] & 1 != 0;
        let chunk_count = u32::from_le_bytes(bytes[9..13].try_into().expect("slice")) as usize;
        let expected_crc = u32::from_le_bytes(bytes[13..17].try_into().expect("slice"));

        // Structural parse first (bounds-checked, so a payload that ends
        // early classifies as Truncated); the CRC is validated after the
        // whole payload region has been walked.
        let mut cursor = 17usize;
        let style_count = read_u32(bytes, &mut cursor)? as usize;
        if style_count == 0 || style_count > usize::from(u16::MAX) + 1 {
            return Err(PersistError::Corrupt);
        }
        let style_json_len = read_u32(bytes, &mut cursor)? as usize;
        let style_json_end = cursor
            .checked_add(style_json_len)
            .ok_or(PersistError::Corrupt)?;
        let style_json = bytes
            .get(cursor..style_json_end)
            .ok_or(PersistError::Truncated)?;
        cursor = style_json_end;
        let styles: Vec<Style> =
            serde_json::from_slice(style_json).map_err(|_| PersistError::Corrupt)?;
        if styles.len() != style_count {
            return Err(PersistError::Corrupt);
        }
        validate_styles(&styles)?;

        let mut cols = Vec::new();
        let mut lines = Vec::new();
        for _ in 0..chunk_count {
            let line_count = read_u32(bytes, &mut cursor)? as usize;
            if line_count == 0 || line_count > PERSIST_CHUNK_LINES as usize {
                return Err(PersistError::Corrupt);
            }
            let total_lines = cols
                .len()
                .checked_add(line_count)
                .ok_or(PersistError::TooManyLines)?;
            if total_lines > config.max_lines {
                return Err(PersistError::TooManyLines);
            }
            let mut chunk_cols = Vec::with_capacity(line_count);
            let mut width_sum = 0usize;
            for _ in 0..line_count {
                let width = read_u16(bytes, &mut cursor)?;
                width_sum = width_sum
                    .checked_add(usize::from(width))
                    .ok_or(PersistError::Corrupt)?;
                chunk_cols.push(width);
            }
            let cell_count = read_u32(bytes, &mut cursor)? as usize;
            let payload_len = read_u32(bytes, &mut cursor)? as usize;
            if cell_count == 0 || payload_len == 0 || cell_count != width_sum {
                return Err(PersistError::Corrupt);
            }
            let cell_bytes_len = cell_count
                .checked_mul(std::mem::size_of::<Cell>())
                .ok_or(PersistError::Corrupt)?;
            if compressed {
                let maximum_compressed_len =
                    cell_bytes_len.checked_mul(2).ok_or(PersistError::Corrupt)?;
                if payload_len > maximum_compressed_len {
                    return Err(PersistError::Corrupt);
                }
                let maximum_expansion = payload_len
                    .checked_mul(255)
                    .and_then(|len| len.checked_add(32))
                    .ok_or(PersistError::Corrupt)?;
                if cell_bytes_len > maximum_expansion {
                    return Err(PersistError::Corrupt);
                }
            } else if payload_len != cell_bytes_len {
                return Err(PersistError::Corrupt);
            }
            let payload_end = cursor
                .checked_add(payload_len)
                .ok_or(PersistError::Corrupt)?;
            let payload = bytes
                .get(cursor..payload_end)
                .ok_or(PersistError::Truncated)?;
            cursor = payload_end;

            let mut cells = vec![Cell::default(); cell_count];
            let out_bytes = unsafe {
                // SAFETY: `Cell` is #[repr(C)] 8 bytes with no padding;
                // `cells` owns exactly `cell_count` initialized cells and
                // exposing that allocation as bytes weakens alignment.
                std::slice::from_raw_parts_mut(cells.as_mut_ptr().cast::<u8>(), cell_bytes_len)
            };
            if compressed {
                lz4_flex::block::decompress_into(payload, out_bytes)
                    .map_err(|_| PersistError::Corrupt)?;
            } else {
                out_bytes.copy_from_slice(payload);
            }
            if cells
                .iter()
                .any(|cell| usize::from(cell.style) >= styles.len())
            {
                return Err(PersistError::Corrupt);
            }

            let mut offset = 0usize;
            for &width in &chunk_cols {
                let end = offset
                    .checked_add(usize::from(width))
                    .ok_or(PersistError::Corrupt)?;
                if end > cells.len() {
                    return Err(PersistError::Corrupt);
                }
                cols.push(width);
                lines.push(cells[offset..end].to_vec());
                offset = end;
            }
            if offset != cells.len() {
                return Err(PersistError::Corrupt);
            }
        }
        if cursor != bytes.len() {
            return Err(PersistError::Corrupt);
        }
        let actual_crc = crc32(&bytes[17..]);
        if actual_crc != expected_crc {
            return Err(PersistError::Corrupt);
        }
        Ok(Self {
            version: PERSIST_VERSION,
            styles,
            cols,
            lines,
        })
    }
}

fn validate_styles(styles: &[Style]) -> Result<(), PersistError> {
    if styles.is_empty()
        || styles.len() > usize::from(u16::MAX) + 1
        || styles[0] != Style::default()
    {
        return Err(PersistError::Corrupt);
    }
    Ok(())
}

fn cell_bytes(cells: &[Cell]) -> &[u8] {
    unsafe {
        // SAFETY: `Cell` is #[repr(C)] 8 bytes with no padding; the slice
        // owns exactly `cells.len()` cells; u8 alignment is weaker.
        std::slice::from_raw_parts(cells.as_ptr().cast::<u8>(), std::mem::size_of_val(cells))
    }
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, PersistError> {
    let end = cursor.checked_add(4).ok_or(PersistError::Corrupt)?;
    let slice = bytes.get(*cursor..end).ok_or(PersistError::Truncated)?;
    *cursor = end;
    Ok(u32::from_le_bytes(slice.try_into().expect("slice")))
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, PersistError> {
    let end = cursor.checked_add(2).ok_or(PersistError::Corrupt)?;
    let slice = bytes.get(*cursor..end).ok_or(PersistError::Truncated)?;
    *cursor = end;
    Ok(u16::from_le_bytes(slice.try_into().expect("slice")))
}

/// IEEE CRC-32 (bitwise; deterministic across platforms).
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use mr_crabs_terminal::{GridSize, ScrollbackConfig, Terminal};

    fn pattern_term() -> Terminal {
        let mut term = Terminal::new_with_config(
            GridSize::new(8, 3),
            ScrollbackConfig {
                max_lines: 1000,
                hot_page_lines: 2,
                max_queued_jobs: 4,
                max_pending_completions: 4,
            },
        )
        .unwrap();
        // CRLF line endings: a bare LF does not return the cursor to column
        // 0, so the 15-character payloads would drift and wrap into two
        // rows per feed. With CRLF each feed scrolls exactly two rows
        // ("persist " + "line NN") into history.
        for i in 0..40 {
            let color = if i % 2 == 0 { 31 } else { 34 };
            term.feed(format!("\x1b[{color}mpersist line {i:02}\x1b[0m\r\n").as_bytes())
                .unwrap_or_else(|error| panic!("persist pattern fixture feed should succeed for line {i:02}: {error}"));
        }
        term.force_compress_all();
        term
    }

    #[test]
    fn roundtrip_preserves_history_byte_identically() {
        let mut term = pattern_term();
        let config = PersistConfig::default();
        let file = HistoryFile::capture(&mut term, &config).expect("capture");
        assert_eq!(file.lines.len(), term.history_len());
        assert!(file.styles.len() >= 3);
        assert_eq!(file.styles, term.global_styles());
        assert!(file.lines.iter().flatten().any(|cell| cell.style != 0));
        let encoded = file.encode(&config).expect("encode");
        assert!(encoded.len() < config.max_bytes);

        let decoded = HistoryFile::decode(&encoded, &config).expect("decode");
        assert_eq!(decoded, file);
        // Each 15-character payload occupies two 8-column rows, so 40 feeds
        // leave `term.history_len()` lines; the engine's history length is
        // the authority.
        let expected_lines = term.history_len();
        assert_eq!(decoded.lines.len(), expected_lines);
        assert_eq!(decoded.cols.len(), expected_lines);
        // Spot-check content through the terminal read path.
        let mut out = Vec::new();
        assert!(term.read_history_line(0, &mut out));
        assert_eq!(decoded.lines[0], out);
    }

    #[test]
    fn decode_rejects_corrupt_version_mismatch_and_oversize() {
        let mut term = pattern_term();
        let config = PersistConfig::default();
        let file = HistoryFile::capture(&mut term, &config).expect("capture");
        let encoded = file.encode(&config).expect("encode");

        // Corrupt: flip one payload byte.
        let mut corrupt = encoded.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0xFF;
        assert_eq!(
            HistoryFile::decode(&corrupt, &config),
            Err(PersistError::Corrupt)
        );

        // Corrupt: wrong magic.
        let mut bad_magic = encoded.clone();
        bad_magic[0] = b'X';
        assert_eq!(
            HistoryFile::decode(&bad_magic, &config),
            Err(PersistError::BadMagic)
        );

        // Version mismatch.
        let mut old_version = encoded.clone();
        old_version[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            HistoryFile::decode(&old_version, &config),
            Err(PersistError::VersionMismatch(1))
        );

        // Truncated.
        assert_eq!(
            HistoryFile::decode(&encoded[..encoded.len() - 5], &config),
            Err(PersistError::Truncated)
        );

        // Oversized: the payload is small, so shrink the cap below it.
        let tiny = PersistConfig {
            max_bytes: 16,
            max_lines: 1_000_000,
            compress: true,
        };
        assert_eq!(
            HistoryFile::decode(&encoded, &tiny),
            Err(PersistError::TooLarge)
        );

        // Line cap.
        let few_lines = PersistConfig {
            max_bytes: 1_000_000,
            max_lines: 10,
            compress: true,
        };
        assert_eq!(
            HistoryFile::decode(&encoded, &few_lines),
            Err(PersistError::TooManyLines)
        );
    }

    #[test]
    fn encode_rejects_oversize_and_uncompressed_roundtrip() {
        let mut term = pattern_term();
        let config = PersistConfig {
            max_bytes: 16,
            max_lines: 1_000_000,
            compress: true,
        };
        let file = HistoryFile::capture(&mut term, &config).expect("capture");
        assert_eq!(file.encode(&config), Err(PersistError::TooLarge));

        // Uncompressed variant roundtrips too.
        let plain = PersistConfig {
            max_bytes: DEFAULT_MAX_BYTES,
            max_lines: 1_000_000,
            compress: false,
        };
        let file = HistoryFile::capture(&mut term, &plain).expect("capture");
        let encoded = file.encode(&plain).expect("encode uncompressed");
        let decoded = HistoryFile::decode(&encoded, &plain).expect("decode uncompressed");
        assert_eq!(decoded, file);
    }
    #[test]
    fn empty_history_still_carries_the_global_style_table() {
        let mut term = Terminal::new(GridSize::new(8, 3)).unwrap();
        term.feed(b"\x1b[38;2;12;34;56mstyled\x1b[0m")
            .expect("persist empty-history fixture feed should succeed for truecolor styled text");
        assert_eq!(term.history_len(), 0);

        let config = PersistConfig::default();
        let file = HistoryFile::capture(&mut term, &config).expect("capture empty history");
        assert!(file.styles.len() > 1);
        assert!(file.lines.is_empty());
        let encoded = file.encode(&config).expect("encode empty history");
        assert_eq!(u32::from_le_bytes(encoded[9..13].try_into().unwrap()), 0);
        assert!(encoded.len() > 25);
        assert_eq!(
            HistoryFile::decode(&encoded, &config).expect("decode empty history"),
            file
        );
    }

    #[test]
    fn invalid_style_tables_and_cell_ids_are_rejected() {
        let mut term = pattern_term();
        let plain = PersistConfig {
            max_bytes: DEFAULT_MAX_BYTES,
            max_lines: DEFAULT_MAX_LINES,
            compress: false,
        };
        let file = HistoryFile::capture(&mut term, &plain).expect("capture");

        let mut empty_styles = file.clone();
        empty_styles.styles.clear();
        assert_eq!(empty_styles.encode(&plain), Err(PersistError::Corrupt));

        let mut nondefault_zero = file.clone();
        nondefault_zero.styles[0] = nondefault_zero.styles[1].clone();
        assert_eq!(nondefault_zero.encode(&plain), Err(PersistError::Corrupt));

        let mut invalid_cell = file.clone();
        invalid_cell.lines[0][0].style = u16::MAX;
        assert_eq!(invalid_cell.encode(&plain), Err(PersistError::Corrupt));

        let mut encoded = file.encode(&plain).expect("encode valid plain history");
        let style_json_len = u32::from_le_bytes(encoded[21..25].try_into().unwrap()) as usize;
        let mut cursor = 25 + style_json_len;
        let line_count = u32::from_le_bytes(encoded[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4 + line_count * 2 + 4 + 4;
        encoded[cursor + 4..cursor + 6].copy_from_slice(&u16::MAX.to_le_bytes());
        let crc = crc32(&encoded[17..]);
        encoded[13..17].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(HistoryFile::decode(&encoded, &plain), Err(PersistError::Corrupt));
    }

    #[test]
    fn roundtrip_indexes_widths_across_chunk_boundaries() {
        let mut cols = vec![1; PERSIST_CHUNK_LINES as usize];
        cols.push(2);
        let mut lines = vec![vec![Cell::default()]; PERSIST_CHUNK_LINES as usize];
        lines.push(vec![Cell::default(); 2]);
        let file = HistoryFile {
            version: PERSIST_VERSION,
            styles: vec![Style::default()],
            cols,
            lines,
        };
        let config = PersistConfig::default();
        let encoded = file.encode(&config).expect("encode second chunk");
        assert_eq!(
            HistoryFile::decode(&encoded, &config).expect("decode second chunk"),
            file
        );
    }

    #[test]
    fn crc32_is_deterministic_and_sensitive() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"abc"), crc32(b"abc"));
        assert_ne!(crc32(b"abc"), crc32(b"abd"));
        // Known vector (IEEE CRC-32 of "123456789").
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
