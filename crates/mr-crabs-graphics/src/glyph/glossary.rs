//! Per-session glossary for Glyph Protocol registrations.
//!
//! Faithful port of `src/terminal/apc/glyph/Glossary.zig`: at most
//! `MAX_GLOSSARY_ENTRIES` (1024) registrations keyed by codepoint, FIFO
//! eviction of the oldest entry, PUA-only registration, and re-registration
//! that moves an entry to the newest position.

use crate::glyph::glyf::{GlyfError, validate_simple_glyf};
use crate::glyph::request::{Format, Register, RegisterOption, RegisterValue, Size, Width};
use crate::glyph::{MAX_GLOSSARY_ENTRIES, MAX_PAYLOAD_SIZE};

/// Errors while registering a glossary entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryError {
    /// The register request is missing a required option or has an invalid
    /// explicitly-provided option value.
    InvalidOptions,
    /// The requested payload format is not supported.
    UnsupportedFormat,
    /// The decoded payload exceeds the protocol limit.
    PayloadTooLarge,
    /// The payload could not be decoded or parsed as the declared format.
    MalformedPayload,
    /// The glyf payload is composite, which the protocol forbids.
    CompositeUnsupported,
    /// The glyf payload contains hinting instructions, which the protocol
    /// forbids.
    HintingUnsupported,
    /// The target codepoint is not in a Private Use Area.
    OutOfNamespace,
    /// Allocation failed while building the entry.
    OutOfMemory,
}

/// Authored metrics for a glyph's design coordinate space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesignMetrics {
    pub units_per_em: u32,
    pub advance_width: u32,
    pub line_height: u32,
}

/// Renderer-neutral scale/alignment/padding constraint (the oracle's
/// `FontGlyph.RenderOptions.Constraint` normalization).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraint {
    pub size: Size,
    pub align_horizontal: AlignHorizontal,
    pub align_vertical: AlignVertical,
    pub pad_top: f64,
    pub pad_right: f64,
    pub pad_bottom: f64,
    pub pad_left: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignHorizontal {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignVertical {
    Start,
    Center,
    End,
}

/// A single glyph registration entry. The validated decoded payload bytes
/// are stored verbatim; rasterization is the renderer's job.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphEntry {
    /// Validated decoded glyf payload (≤ `MAX_PAYLOAD_SIZE`).
    pub payload: Vec<u8>,
    pub design: DesignMetrics,
    pub width: Width,
    pub constraint: Constraint,
}

impl GlyphEntry {
    /// Build an entry from a register request: validate format, metrics,
    /// width, constraint, and decode+validate the glyf payload.
    pub fn from_register(reg: &Register) -> Result<GlyphEntry, EntryError> {
        let fmt = match reg.get(RegisterOption::Fmt) {
            Some(RegisterValue::Fmt(fmt)) => fmt,
            _ => return Err(EntryError::InvalidOptions),
        };
        let upm = match reg.get(RegisterOption::Upm) {
            Some(RegisterValue::Upm(v)) => v,
            _ => return Err(EntryError::InvalidOptions),
        };
        let aw = match reg.get(RegisterOption::Aw) {
            Some(RegisterValue::Aw(v)) => v,
            _ => return Err(EntryError::InvalidOptions),
        };
        let lh = match reg.get(RegisterOption::Lh) {
            Some(RegisterValue::Lh(v)) => v,
            _ => return Err(EntryError::InvalidOptions),
        };
        if upm == 0 || aw == 0 || lh == 0 {
            return Err(EntryError::InvalidOptions);
        }
        let width = match reg.get(RegisterOption::Width) {
            Some(RegisterValue::Width(w)) => w,
            _ => return Err(EntryError::InvalidOptions),
        };
        let constraint = constraint_from_register(reg)?;

        let payload = match fmt {
            Format::Glyf => decode_and_validate_glyf(reg.payload())?,
            Format::Colrv0 | Format::Colrv1 => return Err(EntryError::UnsupportedFormat),
        };

        Ok(GlyphEntry {
            payload,
            design: DesignMetrics {
                units_per_em: upm,
                advance_width: aw,
                line_height: lh,
            },
            width,
            constraint,
        })
    }
}

fn constraint_from_register(reg: &Register) -> Result<Constraint, EntryError> {
    let size = match reg.get(RegisterOption::Size) {
        Some(RegisterValue::Size(s)) => s,
        _ => return Err(EntryError::InvalidOptions),
    };
    let align = match reg.get(RegisterOption::Align) {
        Some(RegisterValue::Align(a)) => a,
        _ => return Err(EntryError::InvalidOptions),
    };
    let pad = match reg.get(RegisterOption::Pad) {
        Some(RegisterValue::Pad(p)) => p,
        _ => return Err(EntryError::InvalidOptions),
    };

    Ok(Constraint {
        size,
        align_horizontal: match align.horizontal {
            crate::glyph::request::Horizontal::Start => AlignHorizontal::Start,
            crate::glyph::request::Horizontal::Center => AlignHorizontal::Center,
            crate::glyph::request::Horizontal::End => AlignHorizontal::End,
        },
        align_vertical: match align.vertical {
            crate::glyph::request::Vertical::Start => AlignVertical::Start,
            crate::glyph::request::Vertical::Center => AlignVertical::Center,
            crate::glyph::request::Vertical::End => AlignVertical::End,
            // No baseline alignment in the constraint model: start is the
            // closest stable default (the oracle's choice).
            crate::glyph::request::Vertical::Baseline => AlignVertical::Start,
        },
        pad_top: pad.top,
        pad_right: pad.right,
        pad_bottom: pad.bottom,
        pad_left: pad.left,
    })
}

fn decode_and_validate_glyf(payload_b64: &[u8]) -> Result<Vec<u8>, EntryError> {
    // Bound before decoding: base64 inflates by 4/3 plus padding.
    if payload_b64.len() > (MAX_PAYLOAD_SIZE * 4 / 3) + 4 {
        return Err(EntryError::PayloadTooLarge);
    }
    let buf = crate::image::decode_base64_lenient(payload_b64)
        .map_err(|_| EntryError::MalformedPayload)?;
    if buf.len() > MAX_PAYLOAD_SIZE {
        return Err(EntryError::PayloadTooLarge);
    }

    validate_simple_glyf(&buf).map_err(|err| match err {
        GlyfError::MalformedPayload => EntryError::MalformedPayload,
        GlyfError::CompositeUnsupported => EntryError::CompositeUnsupported,
        GlyfError::HintingUnsupported => EntryError::HintingUnsupported,
    })?;
    Ok(buf)
}

/// The per-terminal storage for Glyph Protocol codepoints.
#[derive(Clone, Debug, Default)]
pub struct Glossary {
    /// Ordered entries; index 0 is the oldest (FIFO eviction).
    entries: Vec<(u32, GlyphEntry)>,
}

impl Glossary {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Register a glyph entry. Re-registering a codepoint replaces the entry
    /// and moves it to the newest position; a full glossary evicts the
    /// oldest entry (FIFO).
    pub fn register(&mut self, cp: u32, entry: GlyphEntry) -> Result<(), EntryError> {
        if !is_private_use(cp) {
            return Err(EntryError::OutOfNamespace);
        }
        if let Some(idx) = self.entries.iter().position(|(c, _)| *c == cp) {
            self.entries.remove(idx);
            self.entries.push((cp, entry));
            return Ok(());
        }
        self.entries.push((cp, entry));
        if self.entries.len() > MAX_GLOSSARY_ENTRIES {
            self.entries.remove(0);
        }
        Ok(())
    }

    /// Delete a single entry; a missing entry is a no-op.
    pub fn delete(&mut self, cp: u32) -> Result<(), EntryError> {
        if !is_private_use(cp) {
            return Err(EntryError::OutOfNamespace);
        }
        if let Some(idx) = self.entries.iter().position(|(c, _)| *c == cp) {
            self.entries.remove(idx);
        }
        Ok(())
    }

    /// Clear all entries.
    pub fn clear_and_free(&mut self) {
        self.entries.clear();
    }

    /// True if the codepoint is covered by the glossary.
    pub fn contains(&self, cp: u32) -> bool {
        self.entries.iter().any(|(c, _)| *c == cp)
    }

    /// The entry registered at `cp`, if any.
    pub fn get(&self, cp: u32) -> Option<&GlyphEntry> {
        self.entries.iter().find(|(c, _)| *c == cp).map(|(_, e)| e)
    }
}

/// Return true if `cp` is in one of the Unicode Private Use Areas.
pub fn is_private_use(cp: u32) -> bool {
    (0xE000..=0xF8FF).contains(&cp)
        || (0xF0000..=0xFFFFD).contains(&cp)
        || (0x100000..=0x10FFFD).contains(&cp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::Request;
    use crate::glyph::request::RequestParser;

    /// Base64 of a valid simple glyph (the oracle's decode-triangle).
    const VALID_B64: &str = "AAEAZABkA4QDhAACAAABAQEB9P5wAyADhPzgAAA=";

    fn register_req(data: &str) -> Register {
        let mut p = RequestParser::new(1024 * 1024);
        p.feed_slice(data.as_bytes()).unwrap();
        match p.complete().unwrap() {
            Request::Register(r) => r,
            _ => panic!("expected register"),
        }
    }

    #[test]
    fn entry_from_register_with_defaults() {
        let reg = register_req(&format!("r;cp=e0a0;{VALID_B64}"));
        let entry = GlyphEntry::from_register(&reg).unwrap();
        assert_eq!(entry.design.units_per_em, 1000);
        assert_eq!(entry.design.advance_width, 1000);
        assert_eq!(entry.design.line_height, 1000);
        assert_eq!(entry.width, Width::Narrow);
        assert_eq!(entry.constraint.size, Size::Height);
        assert!(!entry.payload.is_empty());
    }

    #[test]
    fn entry_rejects_zero_metrics() {
        let reg = register_req(&format!("r;cp=e0a0;upm=0;{VALID_B64}"));
        assert_eq!(
            GlyphEntry::from_register(&reg),
            Err(EntryError::InvalidOptions)
        );
    }

    #[test]
    fn entry_rejects_unsupported_formats() {
        let reg = register_req(&format!("r;cp=e0a0;fmt=colrv0;{VALID_B64}"));
        assert_eq!(
            GlyphEntry::from_register(&reg),
            Err(EntryError::UnsupportedFormat)
        );
        let reg = register_req(&format!("r;cp=e0a0;fmt=colrv1;{VALID_B64}"));
        assert_eq!(
            GlyphEntry::from_register(&reg),
            Err(EntryError::UnsupportedFormat)
        );
    }

    #[test]
    fn entry_rejects_bad_base64() {
        let reg = register_req("r;cp=e0a0;%%%not-base64%%%");
        assert_eq!(
            GlyphEntry::from_register(&reg),
            Err(EntryError::MalformedPayload)
        );
    }

    #[test]
    fn entry_rejects_oversized_payload() {
        let big = "A".repeat((MAX_PAYLOAD_SIZE + 1) * 4 / 3 + 8);
        let reg = register_req(&format!("r;cp=e0a0;{big}"));
        assert_eq!(
            GlyphEntry::from_register(&reg),
            Err(EntryError::PayloadTooLarge)
        );
    }

    #[test]
    fn entry_rejects_composite_and_hinting() {
        // Composite glyph: numberOfContours = -1.
        let mut comp = Vec::new();
        comp.extend_from_slice(&(-1i16).to_be_bytes());
        comp.extend_from_slice(&[0u8; 8]);
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&comp);
        let reg = register_req(&format!("r;cp=e0a0;{b64}"));
        assert_eq!(
            GlyphEntry::from_register(&reg),
            Err(EntryError::CompositeUnsupported)
        );

        // Hinting: instructionLength > 0.
        let mut hint = Vec::new();
        hint.extend_from_slice(&1i16.to_be_bytes());
        hint.extend_from_slice(&[0u8; 8]);
        hint.extend_from_slice(&0u16.to_be_bytes());
        hint.extend_from_slice(&1u16.to_be_bytes());
        let b64 = base64::engine::general_purpose::STANDARD.encode(&hint);
        let reg = register_req(&format!("r;cp=e0a0;{b64}"));
        assert_eq!(
            GlyphEntry::from_register(&reg),
            Err(EntryError::HintingUnsupported)
        );
    }

    #[test]
    fn pua_ranges() {
        assert!(is_private_use(0xE000));
        assert!(is_private_use(0xF8FF));
        assert!(is_private_use(0xF0000));
        assert!(is_private_use(0xFFFFD));
        assert!(is_private_use(0x100000));
        assert!(is_private_use(0x10FFFD));
        assert!(!is_private_use(0x41));
        assert!(!is_private_use(0xDFFF));
        // Just past the end of the BMP private use area (0xE000..0xF8FF).
        assert!(!is_private_use(0xF900));
        assert!(!is_private_use(0x110000));
    }

    #[test]
    fn glossary_fifo_and_replace() {
        let mut g = Glossary::default();
        let reg = register_req(&format!("r;cp=e0a0;{VALID_B64}"));

        // Register e001, then e0a0, then re-register e0a0: the
        // re-registration moves e0a0 to the newest position and e001
        // becomes the oldest (FIFO order).
        g.register(0xE001, GlyphEntry::from_register(&reg).unwrap())
            .unwrap();
        g.register(0xE0A0, GlyphEntry::from_register(&reg).unwrap())
            .unwrap();
        g.register(0xE0A0, GlyphEntry::from_register(&reg).unwrap())
            .unwrap();
        assert!(g.contains(0xE0A0));

        // Fill to capacity with entries that do not collide with e001/
        // e0a0: the 1023rd new entry pushes the count past the bound and
        // evicts the oldest entry (e001).
        for i in 0..MAX_GLOSSARY_ENTRIES - 1 {
            g.register(0xE100 + i as u32, GlyphEntry::from_register(&reg).unwrap())
                .unwrap();
        }
        assert_eq!(g.len(), MAX_GLOSSARY_ENTRIES);
        assert!(g.contains(0xE0A0));
        assert!(!g.contains(0xE001));

        // Out-of-namespace registration fails without mutation.
        assert_eq!(
            g.register(0x41, GlyphEntry::from_register(&reg).unwrap()),
            Err(EntryError::OutOfNamespace)
        );
    }

    #[test]
    fn glossary_delete_and_clear() {
        let mut g = Glossary::default();
        let reg = register_req(&format!("r;cp=e0a0;{VALID_B64}"));
        g.register(0xE0A0, GlyphEntry::from_register(&reg).unwrap())
            .unwrap();
        assert_eq!(g.delete(0xE0A0), Ok(()));
        assert!(!g.contains(0xE0A0));
        // Deleting a missing entry is a no-op success.
        assert_eq!(g.delete(0xE0A0), Ok(()));
        assert_eq!(g.delete(0x41), Err(EntryError::OutOfNamespace));

        g.register(0xE0A1, GlyphEntry::from_register(&reg).unwrap())
            .unwrap();
        g.clear_and_free();
        assert_eq!(g.len(), 0);
    }
}
