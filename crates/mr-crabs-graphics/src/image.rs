//! Decoded image representation, PNG decoding, and zlib decompression.
//!
//! Provenance: `src/terminal/kitty/graphics_image.zig` (Ghostty source commit
//! `d2c70a8c7b9b6893c13640c02d7b6f9a1624f3f0`). A stored image always holds
//! fully decoded RGBA/RGB/gray bytes: zlib payloads are inflated and PNG
//! payloads are decoded to RGBA before an image completes, so
//! `Image.compression` is always `None` and `Image.format` is never `Png`
//! for stored images.

use std::fmt;

/// Maximum width or height of an image. Taken directly from Kitty and the
/// oracle (`graphics_image.zig:17` `max_dimension = 10000`).
pub const MAX_DIMENSION: u32 = 10_000;

/// Maximum size in bytes of image payload data (raw, decompressed, or
/// decoded). Taken from Kitty and the oracle (`graphics_image.zig:20`
/// `max_size = 400 * 1024 * 1024`).
pub const MAX_SIZE: usize = 400 * 1024 * 1024;

/// Image pixel formats understood by the Kitty graphics protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// 24-bit RGB.
    Rgb,
    /// 32-bit RGBA.
    Rgba,
    /// 8-bit grayscale.
    Gray,
    /// 16-bit gray + alpha.
    GrayAlpha,
    /// PNG container; only valid while a transmission is still loading.
    Png,
}

impl ImageFormat {
    /// Map a wire-format `f` value to a format (24/32/100 per the kitty
    /// protocol). Unknown values are rejected like the oracle.
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            24 => Some(Self::Rgb),
            32 => Some(Self::Rgba),
            100 => Some(Self::Png),
            _ => None,
        }
    }

    /// Bytes per pixel for a non-PNG format. `Png` is invalid here; callers
    /// must validate before use (the oracle `formatBpp` marks it unreachable).
    pub fn bpp(self) -> u8 {
        match self {
            Self::Gray => 1,
            Self::GrayAlpha => 2,
            Self::Rgb => 3,
            Self::Rgba => 4,
            Self::Png => 0,
        }
    }
}

/// Compression of a transmission payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    ZlibDeflate,
}

/// Transmission medium (kitty `t=` key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Medium {
    Direct,
    File,
    TemporaryFile,
    SharedMemory,
}

impl Medium {
    /// Map a wire character to a medium (`d`/`f`/`t`/`s`).
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            b'd' => Some(Self::Direct),
            b'f' => Some(Self::File),
            b't' => Some(Self::TemporaryFile),
            b's' => Some(Self::SharedMemory),
            _ => None,
        }
    }
}

/// Errors produced by image loading, decoding, and storage. The variant
/// names and their response encodings match `graphics_exec.zig`
/// (`encodeError`, lines ~360-383).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageError {
    InvalidData,
    DecompressionFailed,
    DimensionsRequired,
    DimensionsTooLarge,
    FilePathTooLong,
    TemporaryFileNotInTempDir,
    TemporaryFileNotNamedCorrectly,
    UnsupportedFormat,
    UnsupportedMedium,
    UnsupportedDepth,
    /// The image alone exceeds the storage byte budget, or eviction could
    /// not free enough bytes (maps to `error.OutOfMemory` upstream).
    OutOfMemory,
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ImageError {}

impl ImageError {
    /// The exact error message emitted in a kitty graphics response,
    /// matching `graphics_exec.zig` `encodeError`.
    pub fn message(self) -> &'static str {
        match self {
            Self::OutOfMemory => "ENOMEM: out of memory",
            Self::InvalidData => "EINVAL: invalid data",
            Self::DecompressionFailed => "EINVAL: decompression failed",
            Self::FilePathTooLong => "EINVAL: file path too long",
            Self::TemporaryFileNotInTempDir => "EINVAL: temporary file not in temp dir",
            Self::TemporaryFileNotNamedCorrectly => "EINVAL: temporary file not named correctly",
            Self::UnsupportedFormat => "EINVAL: unsupported format",
            Self::UnsupportedMedium => "EINVAL: unsupported medium",
            Self::UnsupportedDepth => "EINVAL: unsupported pixel depth",
            Self::DimensionsRequired => "EINVAL: dimensions required",
            Self::DimensionsTooLarge => "EINVAL: dimensions too large",
        }
    }
}

/// Owned decoded image bytes.
///
/// `Pending` reserves the exact decoded byte length of a payload that has not
/// arrived yet (used by snapshot/restore integration); `Complete` holds the
/// bytes. Both count against the storage byte budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageData {
    Complete(Vec<u8>),
    Pending(usize),
}

impl ImageData {
    pub fn len(&self) -> usize {
        match self {
            Self::Complete(data) => data.len(),
            Self::Pending(len) => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns decoded bytes when the payload is complete.
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Complete(data) => Some(data),
            Self::Pending(_) => None,
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending(_))
    }
}

/// A fully (or pending-) decoded image in storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub id: u32,
    pub number: u32,
    pub width: u32,
    pub height: u32,
    /// Never `Png` for a stored image (PNG is decoded on load).
    pub format: ImageFormat,
    /// Always `None` for a stored image (payloads are inflated on load).
    pub compression: Compression,
    pub data: ImageData,
    /// Kitty usage hint `N=1`: transient images are evicted first.
    pub transient: bool,
    /// Loaded without an explicit ID or number: never respond to it.
    pub implicit_id: bool,
    /// Number of placements referencing this image.
    pub placement_count: u32,
    /// Unique monotonically increasing stamp assigned each time this image
    /// is added to (or replaced in) a store. Zero means "never stored".
    pub generation: u64,
}

impl Image {
    /// The decoded byte length, counting pending reservations.
    pub fn data_len(&self) -> usize {
        self.data.len()
    }
}

/// A decoded PNG: RGBA pixels plus dimensions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decode a PNG into 8-bit RGBA with explicit bounds.
///
/// Mirrors the oracle's `decodePng` (`graphics_image.zig:547-593`):
/// - `max_size` bounds the decoded output (the oracle's limited allocator
///   turns an exceeded budget into `InvalidData`);
/// - `max_dimension` bounds each axis (`DimensionsTooLarge`);
/// - any decode failure is `InvalidData`;
/// - the output is always RGBA (EXPAND|STRIP_16 plus RGB expansion).
pub fn decode_png_to_rgba(
    data: &[u8],
    max_size: usize,
    max_dimension: u32,
) -> Result<DecodedImage, ImageError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    // Indexed/grayscale -> RGB(A), 16-bit -> 8-bit; output is 8-bit
    // RGB or RGBA which we expand to RGBA below.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| ImageError::InvalidData)?;
    let (width, height) = (reader.info().width, reader.info().height);

    // Bound the dimensions before any allocation or decode work.
    if width > max_dimension || height > max_dimension {
        return Err(ImageError::DimensionsTooLarge);
    }
    let out_size = reader.output_buffer_size();
    if out_size > max_size {
        return Err(ImageError::InvalidData);
    }

    let mut buf = vec![0u8; out_size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|_| ImageError::InvalidData)?;
    debug_assert_eq!(info.width, width);
    debug_assert_eq!(info.height, height);

    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(buf.len() / 3 * 4);
            for px in buf.chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
            }
            out
        }
        // normalize_to_color8 guarantees 8-bit RGB/RGBA output; anything
        // else is not a format we can consume.
        _ => return Err(ImageError::UnsupportedFormat),
    };

    Ok(DecodedImage {
        width,
        height,
        rgba,
    })
}

/// Decode base64 (STANDARD alphabet) tolerating a missing final padding
/// quantum, exactly like the oracle's decoder (`simd/base64.zig` trims
/// padding and decodes the final partial quad). Any invalid character or
/// an invalid final quantum is an error.
pub(crate) fn decode_base64_lenient(enc: &[u8]) -> Result<Vec<u8>, ()> {
    use base64::Engine;

    if enc.is_empty() {
        return Ok(Vec::new());
    }
    let pad = (4 - enc.len() % 4) % 4;
    let est = base64::decoded_len_estimate(enc.len() + pad);
    let mut buf = vec![0u8; est];
    let n = if pad == 0 {
        base64::engine::general_purpose::STANDARD
            .decode_slice(enc, &mut buf)
            .map_err(|_| ())?
    } else {
        let mut padded = Vec::with_capacity(enc.len() + pad);
        padded.extend_from_slice(enc);
        padded.resize(enc.len() + pad, b'=');
        base64::engine::general_purpose::STANDARD
            .decode_slice(&padded, &mut buf)
            .map_err(|_| ())?
    };
    buf.truncate(n);
    Ok(buf)
}

/// Inflate a zlib stream with an explicit output bound.
///
/// Mirrors the oracle's `decompressZlib` (`graphics_image.zig:507-545`):
/// output beyond `max_size` or any stream error is `DecompressionFailed`.
pub fn zlib_decompress(data: &[u8], max_size: usize) -> Result<Vec<u8>, ImageError> {
    use std::io::Read;

    let mut out = Vec::new();
    let mut decoder = flate2::read::ZlibDecoder::new(std::io::Cursor::new(data));
    let mut limited = decoder.by_ref().take((max_size as u64) + 1);
    limited
        .read_to_end(&mut out)
        .map_err(|_| ImageError::DecompressionFailed)?;
    if out.len() > max_size {
        return Err(ImageError::DecompressionFailed);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Corpus fixture: the oracle's raw PNG testdata (50x76).
    const PNG_50X76: &[u8] = include_bytes!(
        "../../../verification/graphics-corpus/fixtures/image-png-none-50x76-2147483647-raw.data"
    );
    /// Oracle fixture: 20x15 raw RGB (900 bytes).
    const RGB_20X15: &[u8] = include_bytes!(
        "../../../verification/graphics-corpus/fixtures/image-rgb-none-20x15-2147483647-raw.data"
    );

    #[test]
    fn decode_png_50x76_matches_oracle_fixture() {
        let decoded = decode_png_to_rgba(PNG_50X76, MAX_SIZE, MAX_DIMENSION).unwrap();
        assert_eq!(decoded.width, 50);
        assert_eq!(decoded.height, 76);
        // 50 * 76 * 4 RGBA bytes.
        assert_eq!(decoded.rgba.len(), 50 * 76 * 4);
        // Deterministic corpus hash of the decoded RGBA pixels (recorded in
        // rust/verification/graphics-corpus/s7-graphics.json).
        let hash = crate::testutil::sha256_hex(&decoded.rgba);
        assert_eq!(
            hash,
            "fa7c6853bc6e6526df1efdb5af90ad170d23836c355847f969eee6f5da7cdf62"
        );
    }

    #[test]
    fn decode_png_rgb_expands_to_rgba() {
        // dog.png is an RGB (no alpha) 500x306 PNG from the oracle testdata.
        let png = include_bytes!("../../../verification/graphics-corpus/fixtures/dog.png");
        let decoded = decode_png_to_rgba(png, MAX_SIZE, MAX_DIMENSION).unwrap();
        assert_eq!((decoded.width, decoded.height), (500, 306));
        assert_eq!(decoded.rgba.len(), 500 * 306 * 4);
        let hash = crate::testutil::sha256_hex(&decoded.rgba);
        assert_eq!(
            hash,
            "822f005518939a3c773e74f36870dba27922b9f07f17b2f490e99cda330b1116"
        );
    }

    #[test]
    fn decode_png_rejects_oversized_output() {
        // A tiny valid PNG (8x8) decodes fine; an absurd declared budget
        // must not permit allocation beyond the bound. The bound is checked
        // against the actual 8x8 output, so a small bound rejects.
        let png: &[u8] = PNG_50X76;
        assert!(decode_png_to_rgba(png, 16, MAX_DIMENSION).is_err());
    }

    #[test]
    fn decode_png_rejects_oversized_dimensions() {
        // 50x76 is well under 10000; use an artificial bound to prove the
        // dimension check fires before decode work.
        assert_eq!(
            decode_png_to_rgba(PNG_50X76, MAX_SIZE, 10),
            Err(ImageError::DimensionsTooLarge)
        );
    }

    #[test]
    fn decode_png_rejects_garbage() {
        assert_eq!(
            decode_png_to_rgba(b"not a png at all........", MAX_SIZE, MAX_DIMENSION),
            Err(ImageError::InvalidData)
        );
    }

    #[test]
    fn zlib_roundtrip_matches_oracle_fixture() {
        // The oracle's zlib fixture decompresses to the same bytes as the
        // uncompressed RGB fixture (both 20x15 gradient data).
        let compressed: &[u8] = include_bytes!(
            "../../../verification/graphics-corpus/fixtures/image-rgb-zlib_deflate-128x96-2147483647-raw.data"
        );
        let inflated = zlib_decompress(compressed, MAX_SIZE).unwrap();
        assert_eq!(inflated.len(), 128 * 96 * 3);
        let hash = crate::testutil::sha256_hex(&inflated);
        assert_eq!(
            hash,
            "5846279b35a1f8d74f03984432998a15fc52e4e9d34cb5a8e237674c277b95ad"
        );
    }

    #[test]
    fn zlib_decompress_bounds_output() {
        let compressed: &[u8] = include_bytes!(
            "../../../verification/graphics-corpus/fixtures/image-rgb-zlib_deflate-128x96-2147483647-raw.data"
        );
        assert_eq!(
            zlib_decompress(compressed, 100),
            Err(ImageError::DecompressionFailed)
        );
    }

    #[test]
    fn zlib_decompress_rejects_garbage() {
        assert_eq!(
            zlib_decompress(b"not zlib data", MAX_SIZE),
            Err(ImageError::DecompressionFailed)
        );
    }

    #[test]
    fn rgb_fixture_hash_is_recorded_corpus() {
        // The uncompressed 20x15 RGB fixture is exactly the decoded pixels;
        // its hash is the corpus ground truth for raw transmissions.
        let hash = crate::testutil::sha256_hex(RGB_20X15);
        assert_eq!(
            hash,
            "3422f5c19f5dcb11337383841b4354a18ba95ea5b0b9e7ca905ac4d397a33c0c"
        );
    }
}
