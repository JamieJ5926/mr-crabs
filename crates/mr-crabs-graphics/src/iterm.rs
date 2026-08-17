//! iTerm2 image commands: OSC 1337 `File=` parsing, chunked upload
//! accumulation, and PNG loading through the shared image pipeline.
//!
//! The Ghostty oracle (`src/terminal/osc/parsers/iterm2.zig`) recognizes the
//! `File` key but does not yet implement it; this module implements the
//! documented iTerm2 protocol (<https://iterm2.com/documentation-escape-codes.html>)
//! with the crate's explicit bounds: bounded header, bounded per-chunk and
//! per-upload bytes, bounded in-flight upload count, and name length.
//!
//! Wire format: `OSC 1337 ; File = <args> ; <base64 payload> ST` where
//! `<args>` is a `;`-separated list of `key=value` pairs (keys are ASCII
//! case-insensitive): `name`, `size`, `inline`, `width`, `height`,
//! `preserveAspectRatio`. A chunked upload repeats the same `name` with
//! additional base64 data until `size` bytes have accumulated.

use crate::image::{DecodedImage, ImageError, decode_png_to_rgba};

/// Default maximum number of concurrent in-flight chunked uploads.
pub const DEFAULT_MAX_IN_FLIGHT: usize = 16;
/// Default maximum length of the upload `name`.
pub const DEFAULT_MAX_NAME_BYTES: usize = 1024;
/// Default maximum length of the parsed header (args) portion.
pub const DEFAULT_MAX_HEADER_BYTES: usize = 64 * 1024;
/// Per-upload payload bytes default to the protocol `max_size`.
pub const DEFAULT_MAX_UPLOAD_BYTES: usize = crate::image::MAX_SIZE;

/// Errors produced by iTerm2 image command parsing/accumulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItermError {
    /// Malformed structure, unknown key value, or invalid base64.
    Invalid,
    /// A decoded chunk exceeds the upload byte bound.
    PayloadTooLarge,
    /// The `name` exceeds the length bound.
    NameTooLong,
    /// More chunked uploads are in flight than the bound allows.
    TooManyUploads,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItermSize {
    Auto,
    Cells(u32),
    Pixels(u32),
    Percent(u32),
}

impl ItermSize {
    fn parse(value: &str) -> Result<Self, ItermError> {
        if value.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        if let Some(value) = value.strip_suffix("px") {
            return value
                .parse()
                .map(Self::Pixels)
                .map_err(|_| ItermError::Invalid);
        }
        if let Some(value) = value.strip_suffix('%') {
            return value
                .parse()
                .map(Self::Percent)
                .map_err(|_| ItermError::Invalid);
        }
        value
            .parse()
            .map(Self::Cells)
            .map_err(|_| ItermError::Invalid)
    }
}

/// One parsed `File=` value: decoded chunk plus metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItermFileArgs {
    pub name: String,
    pub size: Option<u64>,
    pub inline: bool,
    pub width: Option<ItermSize>,
    pub height: Option<ItermSize>,
    pub preserve_aspect_ratio: bool,
    /// Raw base64 payload. Chunked uploads concatenate this text and decode
    /// once, so fragments may split a base64 quantum.
    pub payload_b64: String,
}

/// Parse the value following `File=` in an OSC 1337 sequence.
///
/// The base64 payload starts after the last `;`; anything before it is the
/// args list. Returns `None` for a bare empty value.
pub fn parse_file_value(
    value: &str,
    max_header: usize,
    max_chunk: usize,
    max_name: usize,
) -> Result<Option<ItermFileArgs>, ItermError> {
    if value.is_empty() {
        return Ok(None);
    }

    // Split args from payload at the last ';' (base64 contains no ';').
    let (args, payload_b64) = match value.rfind(';') {
        Some(idx) => (&value[..idx], &value[idx + 1..]),
        None => ("", value),
    };
    if args.len() > max_header {
        return Err(ItermError::Invalid);
    }

    let mut name = String::new();
    let mut size: Option<u64> = None;
    let mut inline = false;
    let mut width: Option<ItermSize> = None;
    let mut height: Option<ItermSize> = None;
    let mut preserve_aspect_ratio = true;

    for arg in args.split(';') {
        if arg.is_empty() {
            continue;
        }
        let Some(eq) = arg.find('=') else {
            return Err(ItermError::Invalid);
        };
        let key = &arg[..eq];
        let val = &arg[eq + 1..];
        match key.to_ascii_lowercase().as_str() {
            "name" => {
                if val.len() > max_name {
                    return Err(ItermError::NameTooLong);
                }
                name = val.to_string();
            }
            "size" => {
                size = Some(val.parse::<u64>().map_err(|_| ItermError::Invalid)?);
            }
            "inline" => {
                inline = match val {
                    "1" => true,
                    "0" => false,
                    _ => return Err(ItermError::Invalid),
                };
            }
            "width" => {
                width = Some(ItermSize::parse(val)?);
            }
            "height" => {
                height = Some(ItermSize::parse(val)?);
            }
            "preserveaspectratio" => {
                preserve_aspect_ratio = val != "0";
            }
            // Unknown args are ignored for forward compatibility.
            _ => {}
        }
    }

    if payload_b64.len().saturating_mul(3) / 4 > max_chunk {
        return Err(ItermError::PayloadTooLarge);
    }
    Ok(Some(ItermFileArgs {
        name,
        size,
        inline,
        width,
        height,
        preserve_aspect_ratio,
        payload_b64: payload_b64.to_string(),
    }))
}

/// A fully accumulated upload, ready to load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletedUpload {
    pub name: String,
    pub inline: bool,
    pub width: Option<ItermSize>,
    pub height: Option<ItermSize>,
    pub preserve_aspect_ratio: bool,
    /// Complete file bytes (base64-decoded, unmodified).
    pub data: Vec<u8>,
}

/// In-flight state for one chunked upload.
#[derive(Clone, Debug)]
struct Upload {
    inline: bool,
    width: Option<ItermSize>,
    height: Option<ItermSize>,
    preserve_aspect_ratio: bool,
    encoded: String,
}

/// Accumulator for chunked iTerm2 uploads with explicit bounds.
#[derive(Clone, Debug)]
pub struct ItermUploads {
    inflight: Vec<(String, Upload)>,
    max_in_flight: usize,
    max_bytes: usize,
    total_bytes: usize,
}

impl Default for ItermUploads {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_IN_FLIGHT, DEFAULT_MAX_UPLOAD_BYTES)
    }
}

impl ItermUploads {
    pub fn new(max_in_flight: usize, max_bytes: usize) -> Self {
        Self {
            inflight: Vec::new(),
            max_in_flight,
            max_bytes,
            total_bytes: 0,
        }
    }

    pub fn in_flight_count(&self) -> usize {
        self.inflight.len()
    }

    pub fn in_flight_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Feed one parsed chunk. A declared `size` is an informational decoded
    /// byte target used to join repeated named fragments; an overshooting
    /// final fragment completes and is truncated to that target.
    pub fn feed(&mut self, args: ItermFileArgs) -> Result<Option<CompletedUpload>, ItermError> {
        let encoded_limit = self
            .max_bytes
            .saturating_mul(4)
            .div_ceil(3)
            .saturating_add(4);
        match args.size {
            Some(size) => {
                if size > self.max_bytes as u64 || args.payload_b64.len() > encoded_limit {
                    if let Some(idx) = self
                        .inflight
                        .iter()
                        .position(|(name, _)| *name == args.name)
                    {
                        let (_, removed) = self.inflight.remove(idx);
                        self.total_bytes = self.total_bytes.saturating_sub(removed.encoded.len());
                    }
                    return Err(ItermError::PayloadTooLarge);
                }
                let position = self
                    .inflight
                    .iter()
                    .position(|(name, _)| *name == args.name);
                let idx = match position {
                    Some(idx) => idx,
                    None => {
                        if self.inflight.len() >= self.max_in_flight {
                            return Err(ItermError::TooManyUploads);
                        }
                        self.inflight.push((
                            args.name.clone(),
                            Upload {
                                inline: args.inline,
                                width: args.width,
                                height: args.height,
                                preserve_aspect_ratio: args.preserve_aspect_ratio,
                                encoded: String::new(),
                            },
                        ));
                        self.inflight.len() - 1
                    }
                };
                let (_, upload) = &mut self.inflight[idx];
                if upload.encoded.len().saturating_add(args.payload_b64.len()) > encoded_limit {
                    let (_, removed) = self.inflight.remove(idx);
                    self.total_bytes = self.total_bytes.saturating_sub(removed.encoded.len());
                    return Err(ItermError::PayloadTooLarge);
                }
                upload.encoded.push_str(&args.payload_b64);
                self.total_bytes += args.payload_b64.len();
                let Ok(mut decoded) =
                    crate::image::decode_base64_lenient(upload.encoded.as_bytes())
                else {
                    return Ok(None);
                };
                if decoded.len() < size as usize {
                    return Ok(None);
                }
                decoded.truncate(size as usize);
                let (_, upload) = self.inflight.remove(idx);
                self.total_bytes = self.total_bytes.saturating_sub(upload.encoded.len());
                Ok(Some(upload.into_completed(args.name, decoded)))
            }
            None => {
                if args.payload_b64.is_empty() {
                    return Ok(None);
                }
                let data = crate::image::decode_base64_lenient(args.payload_b64.as_bytes())
                    .map_err(|_| ItermError::Invalid)?;
                if data.len() > self.max_bytes {
                    return Err(ItermError::PayloadTooLarge);
                }
                let upload = Upload {
                    inline: args.inline,
                    width: args.width,
                    height: args.height,
                    preserve_aspect_ratio: args.preserve_aspect_ratio,
                    encoded: String::new(),
                };
                Ok(Some(upload.into_completed(args.name, data)))
            }
        }
    }
}

impl Upload {
    fn into_completed(self, name: String, data: Vec<u8>) -> CompletedUpload {
        CompletedUpload {
            name,
            inline: self.inline,
            width: self.width,
            height: self.height,
            preserve_aspect_ratio: self.preserve_aspect_ratio,
            data,
        }
    }
}

/// Load a completed upload as an image: PNG is decoded to RGBA; any other
/// content is rejected (`UnsupportedFormat`) — Sixel and non-PNG raster
/// formats are out of scope for this slice.
pub fn load_upload(
    upload: &CompletedUpload,
    max_size: usize,
    max_dimension: u32,
) -> Result<DecodedImage, ImageError> {
    // iTerm2 inline images are PNG (or other raster formats we do not
    // decode). Try PNG first; the signature check keeps the failure cheap.
    if !upload.data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(ImageError::UnsupportedFormat);
    }
    decode_png_to_rgba(&upload.data, max_size, max_dimension)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    const PNG_4X4_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAPUlEQVR42g3KMQHAMBACQOREBCJeDiNSXgQiIicOaG8+ADBxKowDFeApORbVcA1oTKnSOrr/iMqsldvk+QNmXR65+p5O5AAAAABJRU5ErkJggg==";

    #[test]
    fn parse_single_shot_inline() {
        let value = format!("name=test.png;size=100;inline=1;{PNG_4X4_B64}");
        let args = parse_file_value(
            &value,
            DEFAULT_MAX_HEADER_BYTES,
            1024 * 1024,
            DEFAULT_MAX_NAME_BYTES,
        )
        .unwrap()
        .unwrap();
        assert_eq!(args.name, "test.png");
        assert_eq!(args.size, Some(100));
        assert!(args.inline);
        assert_eq!(args.payload_b64, PNG_4X4_B64);
    }

    #[test]
    fn parse_keys_are_case_insensitive_and_unknown_ignored() {
        let value =
            format!("Name=x;SIZE=10;INLINE=0;preserveAspectRatio=yes;future=1;{PNG_4X4_B64}");
        let args = parse_file_value(
            &value,
            DEFAULT_MAX_HEADER_BYTES,
            1024 * 1024,
            DEFAULT_MAX_NAME_BYTES,
        )
        .unwrap()
        .unwrap();
        assert_eq!(args.name, "x");
        assert_eq!(args.size, Some(10));
        assert!(!args.inline);
        assert!(args.preserve_aspect_ratio);
    }

    #[test]
    fn parse_rejects_malformed() {
        assert_eq!(
            parse_file_value("size=abc;AAAA", 1024, 1024, 1024),
            Err(ItermError::Invalid)
        );
        assert_eq!(
            parse_file_value("inline=2;AAAA", 1024, 1024, 1024),
            Err(ItermError::Invalid)
        );
        let malformed = parse_file_value(";%%%not-base64%%%", 1024, 1024, 1024)
            .expect("parse")
            .expect("args");
        assert_eq!(
            ItermUploads::default().feed(malformed),
            Err(ItermError::Invalid)
        );
        assert_eq!(parse_file_value("", 1024, 1024, 1024).unwrap(), None);
        assert_eq!(
            parse_file_value("noequals;AAAA", 1024, 1024, 1024),
            Err(ItermError::Invalid)
        );
    }

    #[test]
    fn parse_bounds_header_and_chunk() {
        let long_name = "n".repeat(2048);
        let value = format!("name={long_name};AAAA");
        assert_eq!(
            parse_file_value(&value, 4096, 1024, 1024),
            Err(ItermError::NameTooLong)
        );
        let big = "A".repeat(4096);
        let value = format!("name=x;{big}");
        assert_eq!(
            parse_file_value(&value, 1024, 1024, 1024),
            Err(ItermError::PayloadTooLarge)
        );
    }

    #[test]
    fn chunked_upload_accumulates_by_name() {
        let mut acc = ItermUploads::default();
        let chunk = base64_png_half();
        let value = format!("name=big.png;size=118;inline=1;{}", chunk.0);
        let args = parse_file_value(&value, 4096, 1024 * 1024, 1024)
            .unwrap()
            .unwrap();
        assert!(acc.feed(args).unwrap().is_none());
        assert_eq!(acc.in_flight_count(), 1);

        let value = format!("name=big.png;size=118;inline=1;{}", chunk.1);
        let args = parse_file_value(&value, 4096, 1024 * 1024, 1024)
            .unwrap()
            .unwrap();
        let done = acc.feed(args).unwrap().unwrap();
        assert_eq!(acc.in_flight_count(), 0);
        assert_eq!(done.data.len(), 118);
        assert_eq!(done.name, "big.png");
    }

    #[test]
    fn chunked_upload_different_names_do_not_merge() {
        let mut acc = ItermUploads::default();
        let chunk = base64_png_half();
        let value = format!("name=a.png;size=118;inline=1;{}", chunk.0);
        let args = parse_file_value(&value, 4096, 1024 * 1024, 1024)
            .unwrap()
            .unwrap();
        assert!(acc.feed(args).unwrap().is_none());
        let value = format!("name=b.png;size=118;inline=1;{}", chunk.0);
        let args = parse_file_value(&value, 4096, 1024 * 1024, 1024)
            .unwrap()
            .unwrap();
        assert!(acc.feed(args).unwrap().is_none());
        assert_eq!(acc.in_flight_count(), 2);
    }

    #[test]
    fn chunked_upload_bounds() {
        let mut acc = ItermUploads::new(2, 1024);
        let chunk = base64_png_half();
        let mut feed = |name: &str| {
            let value = format!("name={name};size=200;inline=1;{}", chunk.0);
            let args = parse_file_value(&value, 4096, 1024 * 1024, 1024)
                .unwrap()
                .unwrap();
            acc.feed(args)
        };
        assert!(feed("a.png").unwrap().is_none());
        assert!(feed("b.png").unwrap().is_none());
        // Third in-flight upload exceeds the count bound.
        assert_eq!(feed("c.png"), Err(ItermError::TooManyUploads));

        // Declared size is informational; overshoot completes and truncates.
        let mut acc2 = ItermUploads::default();
        let value = format!("name=z.png;size=10;inline=1;{PNG_4X4_B64}");
        let args = parse_file_value(&value, 4096, 1024 * 1024, 1024)
            .unwrap()
            .unwrap();
        assert_eq!(acc2.feed(args).unwrap().unwrap().data.len(), 10);
    }

    #[test]
    fn parse_width_height_units() {
        let args = parse_file_value("width=12px;height=50%;AAAA", 1024, 1024, 1024)
            .unwrap()
            .unwrap();
        assert_eq!(args.width, Some(ItermSize::Pixels(12)));
        assert_eq!(args.height, Some(ItermSize::Percent(50)));
        let args = parse_file_value("width=auto;height=3;AAAA", 1024, 1024, 1024)
            .unwrap()
            .unwrap();
        assert_eq!(args.width, Some(ItermSize::Auto));
        assert_eq!(args.height, Some(ItermSize::Cells(3)));
        for invalid in ["width=12em;AAAA", "width=px;AAAA", "width=auto%;AAAA"] {
            assert_eq!(
                parse_file_value(invalid, 1024, 1024, 1024),
                Err(ItermError::Invalid)
            );
        }
    }

    #[test]
    fn single_shot_and_first_chunk_respect_upload_bound() {
        let data = vec![7_u8; 2048];
        let encoded = base64::engine::general_purpose::STANDARD.encode(data);
        let args = parse_file_value(&encoded, 1024, 4096, 1024)
            .unwrap()
            .unwrap();
        assert_eq!(
            ItermUploads::new(2, 1024).feed(args),
            Err(ItermError::PayloadTooLarge)
        );
        let declared = format!("name=x;size=2048;{encoded}");
        let args = parse_file_value(&declared, 4096, 4096, 1024)
            .unwrap()
            .unwrap();
        assert_eq!(
            ItermUploads::new(2, 1024).feed(args),
            Err(ItermError::PayloadTooLarge)
        );
    }

    #[test]
    fn load_upload_decodes_png_and_rejects_others() {
        let upload = CompletedUpload {
            name: "x.png".into(),
            inline: true,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            data: base64::engine::general_purpose::STANDARD
                .decode(PNG_4X4_B64)
                .unwrap(),
        };
        let img =
            load_upload(&upload, crate::image::MAX_SIZE, crate::image::MAX_DIMENSION).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
        assert_eq!(img.rgba.len(), 64);

        let gif = CompletedUpload {
            name: "x.gif".into(),
            inline: true,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            data: b"GIF89a...".to_vec(),
        };
        assert_eq!(
            load_upload(&gif, crate::image::MAX_SIZE, crate::image::MAX_DIMENSION),
            Err(ImageError::UnsupportedFormat)
        );
    }

    /// Split the encoded payload across a non-base64-quantum boundary.
    fn base64_png_half() -> (String, String) {
        let split = PNG_4X4_B64.len() / 2 + 1;
        (
            PNG_4X4_B64[..split].to_string(),
            PNG_4X4_B64[split..].to_string(),
        )
    }
}
