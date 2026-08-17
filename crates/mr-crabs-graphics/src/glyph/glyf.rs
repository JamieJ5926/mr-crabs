//! Simple-glyf payload validation for the Glyph Protocol.
//!
//! Port of the validation performed by `src/font/opentype/glyf.zig`
//! (`Entry.size` + the checks in `decodeGlyfPayload`): simple glyphs only
//! (no composites), strictly increasing contour endpoints, no hinting
//! instructions, bounded point counts, and checked coordinate deltas.
//! The decoded payload is stored verbatim after validation; rasterization
//! is the renderer's job.

/// Maximum logical points accepted (a u16 endpoint index plus one).
const MAX_POINTS: usize = 65536;

/// Errors from validating a glyf payload; mapped to protocol reasons by the
/// glossary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyfError {
    /// The payload does not parse as a simple glyph.
    MalformedPayload,
    /// The glyph is composite, which the protocol forbids.
    CompositeUnsupported,
    /// The glyph contains hinting instructions, which the protocol forbids.
    HintingUnsupported,
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_u8(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn read_u16(&mut self) -> Option<u16> {
        let hi = self.read_u8()? as u16;
        let lo = self.read_u8()? as u16;
        Some((hi << 8) | lo)
    }

    fn read_i16(&mut self) -> Option<i16> {
        Some(self.read_u16()? as i16)
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
}

/// Read one coordinate delta (short/same/full encoding).
pub fn validate_simple_glyf(data: &[u8]) -> Result<(), GlyfError> {
    let mut cur = Cursor::new(data);

    // Header: numberOfContours i16 + bbox (4 x i16).
    let num_contours = cur.read_i16().ok_or(GlyfError::MalformedPayload)?;
    for _ in 0..4 {
        cur.read_i16().ok_or(GlyfError::MalformedPayload)?;
    }

    if num_contours < 0 {
        return Err(GlyfError::CompositeUnsupported);
    }
    let num_contours = num_contours as usize;

    // A zero-contour glyph may have no further data; the two-byte minimum
    // for extra data is the instructionLength field.
    if num_contours == 0 && cur.remaining() < 2 {
        return Ok(());
    }

    // endPtsOfContours[numberOfContours]: strictly increasing u16 indices.
    let mut max_point_index: i64 = -1;
    for _ in 0..num_contours {
        let index = cur.read_u16().ok_or(GlyfError::MalformedPayload)? as i64;
        if index <= max_point_index {
            return Err(GlyfError::MalformedPayload);
        }
        max_point_index = index;
    }

    // instructionLength: hinting instructions are forbidden by the protocol.
    let instructions_length = cur.read_u16().ok_or(GlyfError::MalformedPayload)?;
    if instructions_length > 0 {
        return Err(GlyfError::HintingUnsupported);
    }

    // A zero-contour glyph has no points: nothing further to validate
    // (oracle `decode` returns the empty outline here).
    if num_contours == 0 {
        return Ok(());
    }

    let max_point_index = max_point_index as usize;
    if max_point_index >= MAX_POINTS {
        return Err(GlyfError::MalformedPayload);
    }
    let points = max_point_index + 1;

    // flags[variable] with REPEAT_FLAG expansion: materialize the logical
    // flags array (bounded by MAX_POINTS); coordinate bytes are validated
    // directly while reading.
    let mut flags: Vec<u8> = Vec::with_capacity(points.min(1024));
    while flags.len() < points {
        let flag = cur.read_u8().ok_or(GlyfError::MalformedPayload)?;
        let mut span = 1usize;
        if flag & 0x08 != 0 {
            span += cur.read_u8().ok_or(GlyfError::MalformedPayload)? as usize;
        }
        if flags.len().saturating_add(span) > points {
            return Err(GlyfError::MalformedPayload);
        }
        for _ in 0..span {
            flags.push(flag);
        }
    }

    // Coordinates: xCoordinates then yCoordinates, with checked i16 delta
    // accumulation (the oracle's CoordinateOverflow).
    let mut pos = cur.pos;
    let mut prev: i16 = 0;
    for &flag in &flags {
        let delta = read_coord(data, &mut pos, flag, true)?;
        prev = prev.checked_add(delta).ok_or(GlyfError::MalformedPayload)?;
    }
    let mut prev: i16 = 0;
    for &flag in &flags {
        let delta = read_coord(data, &mut pos, flag, false)?;
        prev = prev.checked_add(delta).ok_or(GlyfError::MalformedPayload)?;
    }
    Ok(())
}

/// Read one coordinate delta (short/same/full encoding).
fn read_coord(data: &[u8], pos: &mut usize, flag: u8, is_x: bool) -> Result<i16, GlyfError> {
    let short = if is_x {
        flag & 0x02 != 0
    } else {
        flag & 0x04 != 0
    };
    let same = if is_x {
        flag & 0x10 != 0
    } else {
        flag & 0x20 != 0
    };
    let delta: i16 = if short {
        let b = *data.get(*pos).ok_or(GlyfError::MalformedPayload)?;
        *pos += 1;
        if same { b as i16 } else { -(b as i16) }
    } else if same {
        0
    } else {
        let hi = *data.get(*pos).ok_or(GlyfError::MalformedPayload)? as u16;
        let lo = *data.get(*pos + 1).ok_or(GlyfError::MalformedPayload)? as u16;
        *pos += 2;
        ((hi << 8) | lo) as i16
    };
    Ok(delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a valid minimal glyph: 1 contour, N points, no instructions.
    fn simple_glyph(end_pts: &[u16], flags: &[u8], xs: &[u8], ys: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1i16.to_be_bytes()); // numberOfContours
        out.extend_from_slice(&0i16.to_be_bytes()); // xMin
        out.extend_from_slice(&0i16.to_be_bytes()); // yMin
        out.extend_from_slice(&0i16.to_be_bytes()); // xMax
        out.extend_from_slice(&0i16.to_be_bytes()); // yMax
        for e in end_pts {
            out.extend_from_slice(&e.to_be_bytes());
        }
        out.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
        for f in flags {
            out.push(*f);
        }
        out.extend_from_slice(xs);
        out.extend_from_slice(ys);
        out
    }

    /// The oracle's "decode triangle" glyf payload (valid, one contour).
    const TRIANGLE_B64: &str = "AAEAZABkA4QDhAACAAABAQEB9P5wAyADhPzgAAA=";

    #[test]
    fn triangle_fixture_validates() {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD
            .decode(TRIANGLE_B64)
            .unwrap();
        assert_eq!(validate_simple_glyf(&data), Ok(()));
    }

    #[test]
    fn empty_outline_validates() {
        // numberOfContours=0 with no further data (the oracle accepts a
        // header-only zero-contour glyph).
        let mut out = Vec::new();
        out.extend_from_slice(&0i16.to_be_bytes());
        out.extend_from_slice(&[0u8; 8]);
        assert_eq!(validate_simple_glyf(&out), Ok(()));
    }

    #[test]
    fn zero_contour_with_no_instructions_validates() {
        let mut out = Vec::new();
        out.extend_from_slice(&0i16.to_be_bytes());
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&0u16.to_be_bytes()); // instructionLength only
        assert_eq!(validate_simple_glyf(&out), Ok(()));
    }

    #[test]
    fn composite_glyph_rejected() {
        let mut out = Vec::new();
        out.extend_from_slice(&(-1i16).to_be_bytes());
        out.extend_from_slice(&[0u8; 8]);
        assert_eq!(
            validate_simple_glyf(&out),
            Err(GlyfError::CompositeUnsupported)
        );
    }

    #[test]
    fn hinting_instructions_rejected() {
        let mut out = Vec::new();
        out.extend_from_slice(&1i16.to_be_bytes());
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&0u16.to_be_bytes()); // endPts
        out.extend_from_slice(&1u16.to_be_bytes()); // instructionLength > 0
        assert_eq!(
            validate_simple_glyf(&out),
            Err(GlyfError::HintingUnsupported)
        );
    }

    #[test]
    fn non_monotonic_endpoints_rejected() {
        let mut out = Vec::new();
        out.extend_from_slice(&2i16.to_be_bytes());
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&1u16.to_be_bytes()); // endPts[0] = 1
        out.extend_from_slice(&1u16.to_be_bytes()); // endPts[1] = 1 (not >)
        assert_eq!(validate_simple_glyf(&out), Err(GlyfError::MalformedPayload));
    }

    #[test]
    fn truncated_payload_rejected() {
        assert_eq!(validate_simple_glyf(&[]), Err(GlyfError::MalformedPayload));
        assert_eq!(
            validate_simple_glyf(&[0u8; 9]),
            Err(GlyfError::MalformedPayload)
        );
    }

    #[test]
    fn coordinate_overflow_rejected() {
        // Two points with deltas that overflow i16 accumulation:
        // first x delta = 32767, second = 1 -> 32768 overflows.
        let glyph = simple_glyph(
            &[1],
            &[0b01, 0b01], // on-curve, full i16 coords
            &[0x7F, 0xFF, 0x00, 0x01],
            &[0x00, 0x00, 0x00, 0x00],
        );
        assert_eq!(
            validate_simple_glyf(&glyph),
            Err(GlyfError::MalformedPayload)
        );
    }

    #[test]
    fn repeat_flag_expansion() {
        // One endpoint at index 2; first flag repeats twice (covers points
        // 0..=2 with two flag bytes) using short positive deltas.
        let glyph = simple_glyph(
            &[2],
            &[0b01 | 0x02 | 0x04 | 0x08, 2, 0b01 | 0x02 | 0x04],
            &[1, 1, 1], // x shorts
            &[1, 1, 1], // y shorts
        );
        assert_eq!(validate_simple_glyf(&glyph), Ok(()));
    }

    #[test]
    fn repeat_overflow_rejected() {
        // Repeat count pushes past the endpoint index.
        let mut out = Vec::new();
        out.extend_from_slice(&1i16.to_be_bytes());
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&0u16.to_be_bytes()); // endPts[0] = 0
        out.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
        out.push(0x08); // repeat flag
        out.push(5); // 5 extra points -> past index 0
        assert_eq!(validate_simple_glyf(&out), Err(GlyfError::MalformedPayload));
    }

    #[test]
    fn generated_glyph_roundtrips_through_base64() {
        let glyph = simple_glyph(&[0], &[0b01], &[0x00, 0x00], &[0x00, 0x00]);
        assert_eq!(validate_simple_glyf(&glyph), Ok(()));
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&glyph);
        let back = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(validate_simple_glyf(&back), Ok(()));
    }
}
