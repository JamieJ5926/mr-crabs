//! Exact Termbench generator (cmuratori/termbench 82afbc6).
//!
//! Contract (pinned fixture `/tmp/mr-crabs-warp-benchmark/fixtures/termbench/termbench.cpp`):
//! - NumberTable supplies ordinary base-10 variable-width `0..255` decimal strings
//!   (AppendDecimal / AppendColor: `ESC[38;2;R;G;Bm` foreground, `ESC[48;2;R;G;Bm` background,
//!   each component decimal minimal, no leading zeros; FG and BG are separate SGR
//!   sequences — never combined).
//! - For FrameIndex `F` (`0 .. count`), row `Y` (`0..=24`), col `X` (`0..=80`),
//!   exactly 2025 cells/frame (`81 * 25`):
//!   - CUP once per row via `AppendGoto(1,1+Y)` => `ESC[(Y+1);1H`
//!   - FGPerChar: foreground `((F)&255,(F+Y)&255,(F+Y+X)&255)`, then `'a' + ((F+X+Y)%25)`.
//!   - FGBGPerChar: background `((F+Y+X)&255,(F+Y)&255,F&255)` as a separate SGR
//!     **first**, then the same foreground SGR, then char.
//! - `FlushBuffer` after each frame; Small `512` frames, Normal `8192` frames.
//! - The post-benchmark trailer (`SetColor 0/white, ESC[0m, 1024 newlines, CPU lines`)
//!   is **not** part of the measured payload and is excluded here.
//!
//! The generator is streaming: frames are emitted into bounded chunks
//! (default 256 KiB) so the Normal payload (311 MiB fg / 605 MiB fgbg) is never
//! duplicated in memory.

use std::fmt;

pub const TERMBENCH_WIDTH_INCLUSIVE: u16 = 80;
pub const TERMBENCH_HEIGHT_INCLUSIVE: u16 = 24;
pub const TERMBENCH_COLS: usize = 81; // 0..=80 inclusive
pub const TERMBENCH_ROWS: usize = 25; // 0..=24 inclusive
pub const TERMBENCH_CELLS_PER_FRAME: usize = TERMBENCH_COLS * TERMBENCH_ROWS; // 2025
pub const TERMBENCH_SMALL_FRAMES: usize = 512;
pub const TERMBENCH_NORMAL_FRAMES: usize = 8192;

/// Variant identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TermbenchVariant {
    FGPerChar,
    FGBGPerChar,
}

impl TermbenchVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FGPerChar => "fg_per_char",
            Self::FGBGPerChar => "fgbg_per_char",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fg_per_char" | "FGPerChar" | "fg" => Some(Self::FGPerChar),
            "fgbg_per_char" | "FGBGPerChar" | "fgbg" => Some(Self::FGBGPerChar),
            _ => None,
        }
    }
}

impl fmt::Display for TermbenchVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Pinned FNV-1a 64 identities (computed from the exact generator below
/// and pinned here; the workload pre-pass fails closed on mismatch).
/// Values were derived by streaming the exact termbench.cpp payload through
/// FNV-1a 64 and are reproduced by `cargo test --manifest-path crates/mr-crabs-bench/Cargo.toml --lib termbench`.
pub const TERMBENCH_FG_SMALL_FNV1A64: &str = "c5984b8c8e5ba815";
pub const TERMBENCH_FG_NORMAL_FNV1A64: &str = "76e481efc981efab";
pub const TERMBENCH_FGBG_SMALL_FNV1A64: &str = "dbc8514b256e6775";
pub const TERMBENCH_FGBG_NORMAL_FNV1A64: &str = "f720557f1aac8d3b";

/// Pinned byte lengths (variable-width SGR decimals make these non-trivial).
pub const TERMBENCH_FG_SMALL_BYTES: u64 = 19_484_492;
pub const TERMBENCH_FG_NORMAL_BYTES: u64 = 311_751_872;
pub const TERMBENCH_FGBG_SMALL_BYTES: u64 = 37_847_192;
pub const TERMBENCH_FGBG_NORMAL_BYTES: u64 = 605_555_072;

/// Default streaming chunk size: matches the `throughput_workload` 256 KiB
/// choice (PTY-style) and keeps the measured feed window bounded.
pub const TERMBENCH_CHUNK_BYTES: usize = 256 * 1024;

/// FNV-1a 64 offset basis / prime (mirrors `payloads.rs` / `workloads.rs`).
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Append a minimal base-10 decimal string for `value` (0..=u32) into `out`.
/// Termbench `AppendDecimal` emits the same minimal representation (no leading
/// zeros, single `0` for zero).
#[inline]
fn append_decimal(out: &mut Vec<u8>, value: u32) {
    // Cheap path for 0..255 hot range (RGB / CUP). Use stack buffer.
    // For general u32 we reuse the minimal itoa; `value.to_string` is
    // functionally identical but allocation-free via lexical write.
    // Manual unrolled fast path avoids allocation.
    let mut buf = [0u8; 10];
    let mut len = 0usize;
    let mut v = value;
    if v == 0 {
        out.push(b'0');
        return;
    }
    // Write reversed digits.
    while v > 0 {
        buf[len] = b'0' + (v % 10) as u8;
        v /= 10;
        len += 1;
    }
    // Reverse into out.
    for i in (0..len).rev() {
        out.push(buf[i]);
    }
}

#[inline]
fn append_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(s);
}

/// Append `ESC[38;2;R;G;Bm` (foreground) or `ESC[48;2;R;G;Bm` (background)
/// with **variable-width** decimal components, exactly as termbench.cpp
/// `AppendColor` (separate SGR, not combined).
#[inline]
fn append_color(out: &mut Vec<u8>, is_foreground: bool, r: u8, g: u8, b: u8) {
    if is_foreground {
        append_str(out, b"\x1b[38;2;");
    } else {
        append_str(out, b"\x1b[48;2;");
    }
    append_decimal(out, r as u32);
    out.push(b';');
    append_decimal(out, g as u32);
    out.push(b';');
    append_decimal(out, b as u32);
    out.push(b'm');
}

/// Append `ESC[(Y);(X)H` where `x` and `y` are 1-based decimal.
#[inline]
fn append_cup(out: &mut Vec<u8>, x: u32, y: u32) {
    append_str(out, b"\x1b[");
    append_decimal(out, y);
    out.push(b';');
    append_decimal(out, x);
    out.push(b'H');
}

/// Expected foreground RGB for `(frame, y, x)` per FGPerChar contract.
#[inline]
pub fn expected_fg_rgb(frame: u32, y: u32, x: u32) -> [u8; 3] {
    [
        (frame & 255) as u8,
        ((frame + y) & 255) as u8,
        ((frame + y + x) & 255) as u8,
    ]
}

/// Expected background RGB for `(frame, y, x)` per FGBGPerChar contract.
#[inline]
pub fn expected_bg_rgb(frame: u32, y: u32, x: u32) -> [u8; 3] {
    [
        ((frame + y + x) & 255) as u8,
        ((frame + y) & 255) as u8,
        (frame & 255) as u8,
    ]
}

/// Expected cell char for `(frame, y, x)`: `'a' + ((F+X+Y)%25)` in `a..=y`.
#[inline]
pub fn expected_char(frame: u32, y: u32, x: u32) -> u8 {
    b'a' + (((frame + x + y) % 25) as u8)
}

/// Append a single frame's bytes for `variant`/`frame` into `out`.
pub fn append_frame(out: &mut Vec<u8>, variant: TermbenchVariant, frame: u32) {
    for y in 0..=TERMBENCH_HEIGHT_INCLUSIVE as u32 {
        append_cup(out, 1, 1 + y);
        for x in 0..=TERMBENCH_WIDTH_INCLUSIVE as u32 {
            match variant {
                TermbenchVariant::FGPerChar => {
                    let [fr, fg, fb] = expected_fg_rgb(frame, y, x);
                    append_color(out, true, fr, fg, fb);
                    out.push(expected_char(frame, y, x));
                }
                TermbenchVariant::FGBGPerChar => {
                    let [br, bg, bb] = expected_bg_rgb(frame, y, x);
                    let [fr, fg, fb] = expected_fg_rgb(frame, y, x);
                    append_color(out, false, br, bg, bb);
                    append_color(out, true, fr, fg, fb);
                    out.push(expected_char(frame, y, x));
                }
            }
        }
    }
}

/// Total byte length for `(variant, frames)` — computed by streaming the
/// exact generator (one reusable frame buffer) so the pinned constants are
/// validated mutation-sensitively. Never return the pinned constants directly.
pub fn total_bytes(variant: TermbenchVariant, frames: usize) -> u64 {
    let mut n: u64 = 0;
    let mut buf = Vec::with_capacity(80 * 1024);
    for f in 0..frames as u32 {
        buf.clear();
        append_frame(&mut buf, variant, f);
        n += buf.len() as u64;
    }
    n
}

/// FNV-1a 64 hex of `bytes`.
pub fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    format!("{h:016x}")
}

/// Streaming FNV-1a 64 identity of the termbench payload without ever
/// holding the full Normal payload in memory.
///
/// Iterates frames `0..frames`, emitting each frame into a reusable buffer
/// and hashing in `TERMBENCH_CHUNK_BYTES` chunks (the same chunking the
/// measured workload uses for feeding).
pub fn streaming_fnv1a64(variant: TermbenchVariant, frames: usize) -> String {
    streaming_fnv1a64_with_chunk(variant, frames, TERMBENCH_CHUNK_BYTES)
}

pub fn streaming_fnv1a64_with_chunk(
    variant: TermbenchVariant,
    frames: usize,
    chunk_bytes: usize,
) -> String {
    assert!(chunk_bytes != 0, "chunk_bytes must be non-zero");
    let mut h = FNV_OFFSET;
    let mut chunk = Vec::with_capacity(chunk_bytes);
    let mut frame_buf = Vec::with_capacity(80 * 1024);
    for f in 0..frames as u32 {
        frame_buf.clear();
        append_frame(&mut frame_buf, variant, f);
        let mut pos = 0usize;
        while pos < frame_buf.len() {
            let space = chunk_bytes - chunk.len();
            let take = (frame_buf.len() - pos).min(space);
            chunk.extend_from_slice(&frame_buf[pos..pos + take]);
            pos += take;
            if chunk.len() == chunk_bytes {
                for &b in &chunk {
                    h ^= u64::from(b);
                    h = h.wrapping_mul(FNV_PRIME);
                }
                chunk.clear();
            }
        }
    }
    for &b in &chunk {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    format!("{h:016x}")
}

/// Streaming identity plus total byte count (one pass), bounded memory.
pub fn streaming_identity(variant: TermbenchVariant, frames: usize) -> (String, u64) {
    let mut h = FNV_OFFSET;
    let mut total: u64 = 0;
    let mut chunk = Vec::with_capacity(TERMBENCH_CHUNK_BYTES);
    let mut frame_buf = Vec::with_capacity(80 * 1024);
    for f in 0..frames as u32 {
        frame_buf.clear();
        append_frame(&mut frame_buf, variant, f);
        let mut pos = 0usize;
        while pos < frame_buf.len() {
            let space = TERMBENCH_CHUNK_BYTES - chunk.len();
            let take = (frame_buf.len() - pos).min(space);
            chunk.extend_from_slice(&frame_buf[pos..pos + take]);
            pos += take;
            if chunk.len() == TERMBENCH_CHUNK_BYTES {
                for &b in &chunk {
                    h ^= u64::from(b);
                    h = h.wrapping_mul(FNV_PRIME);
                }
                total += chunk.len() as u64;
                chunk.clear();
            }
        }
    }
    for &b in &chunk {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    total += chunk.len() as u64;
    (format!("{h:016x}"), total)
}

/// Iterator that yields bounded chunks (`Vec<u8>` capped at `chunk_bytes`)
/// without ever materializing the whole payload. Each chunk carries the
/// canonical bytes at its global offset; callers should `feed` each chunk
/// sequentially.
pub struct TermbenchChunks {
    variant: TermbenchVariant,
    frames: usize,
    chunk_bytes: usize,
    next_frame: u32,
    /// Bytes of the current frame not yet emitted into chunked output.
    frame_buf: Vec<u8>,
    frame_pos: usize,
    /// Accumulated chunk not yet yielded.
    pending: Vec<u8>,
    done: bool,
}

impl TermbenchChunks {
    pub fn new(variant: TermbenchVariant, frames: usize, chunk_bytes: usize) -> Self {
        assert!(chunk_bytes != 0, "chunk_bytes must be non-zero");
        Self {
            variant,
            frames,
            chunk_bytes,
            next_frame: 0,
            frame_buf: Vec::with_capacity(80 * 1024),
            frame_pos: 0,
            pending: Vec::with_capacity(chunk_bytes),
            done: false,
        }
    }
}

impl Iterator for TermbenchChunks {
    type Item = Vec<u8>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        while self.pending.len() < self.chunk_bytes {
            if self.frame_pos >= self.frame_buf.len() {
                if self.next_frame as usize >= self.frames {
                    break;
                }
                self.frame_buf.clear();
                append_frame(&mut self.frame_buf, self.variant, self.next_frame);
                self.next_frame += 1;
                self.frame_pos = 0;
            }
            let space = self.chunk_bytes - self.pending.len();
            let take = (self.frame_buf.len() - self.frame_pos).min(space);
            self.pending
                .extend_from_slice(&self.frame_buf[self.frame_pos..self.frame_pos + take]);
            self.frame_pos += take;
            // If pending reached capacity, yield it now to preserve bounded size.
            if self.pending.len() == self.chunk_bytes {
                break;
            }
        }
        if self.pending.is_empty() {
            if self.next_frame as usize >= self.frames && self.frame_pos >= self.frame_buf.len() {
                self.done = true;
            }
            return None;
        }
        // Yield current pending and reset.
        let mut out = Vec::with_capacity(self.chunk_bytes);
        std::mem::swap(&mut out, &mut self.pending);
        // If this was the final partial chunk, mark done on next call.
        if self.next_frame as usize >= self.frames
            && self.frame_pos >= self.frame_buf.len()
            && self.pending.is_empty()
        {
            // Next call will return None after draining.
        }
        Some(out)
    }
}

/// Canonical workload spec JSON for `(variant, frames)`.
pub fn spec_json(variant: TermbenchVariant, frames: usize) -> serde_json::Value {
    let (bytes, fnv) = match (variant, frames) {
        (TermbenchVariant::FGPerChar, TERMBENCH_SMALL_FRAMES) => {
            (TERMBENCH_FG_SMALL_BYTES, TERMBENCH_FG_SMALL_FNV1A64)
        }
        (TermbenchVariant::FGPerChar, TERMBENCH_NORMAL_FRAMES) => {
            (TERMBENCH_FG_NORMAL_BYTES, TERMBENCH_FG_NORMAL_FNV1A64)
        }
        (TermbenchVariant::FGBGPerChar, TERMBENCH_SMALL_FRAMES) => {
            (TERMBENCH_FGBG_SMALL_BYTES, TERMBENCH_FGBG_SMALL_FNV1A64)
        }
        (TermbenchVariant::FGBGPerChar, TERMBENCH_NORMAL_FRAMES) => {
            (TERMBENCH_FGBG_NORMAL_BYTES, TERMBENCH_FGBG_NORMAL_FNV1A64)
        }
        _ => {
            let (hash, b) = streaming_identity(variant, frames);
            // Stream-computed values for non-pinned frame counts (tests).
            return serde_json::json!({
                "variant": variant.as_str(),
                "frames": frames,
                "cells_per_frame": TERMBENCH_CELLS_PER_FRAME,
                "grid": [TERMBENCH_COLS, TERMBENCH_ROWS],
                "width_inclusive": TERMBENCH_WIDTH_INCLUSIVE,
                "height_inclusive": TERMBENCH_HEIGHT_INCLUSIVE,
                "chunk_bytes": TERMBENCH_CHUNK_BYTES,
                "total_bytes": b,
                "fnv1a64": hash,
            });
        }
    };
    serde_json::json!({
        "variant": variant.as_str(),
        "frames": frames,
        "cells_per_frame": TERMBENCH_CELLS_PER_FRAME,
        "grid": [TERMBENCH_COLS, TERMBENCH_ROWS],
        "width_inclusive": TERMBENCH_WIDTH_INCLUSIVE,
        "height_inclusive": TERMBENCH_HEIGHT_INCLUSIVE,
        "chunk_bytes": TERMBENCH_CHUNK_BYTES,
        "total_bytes": bytes,
        "fnv1a64": fnv,
    })
}

/// Build a single-frame payload (for oracle tests) — the concatenation of
/// one frame's CUP + per-cell SGR + char bytes.
pub fn frame_payload(variant: TermbenchVariant, frame: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(70 * 1024);
    append_frame(&mut out, variant, frame);
    out
}

/// Convenience: generate the first `frames` frames concatenated (caller must
/// ensure `frames` is small — e.g. Small thresholds — to avoid OOM; Normal
/// callers should stream via `TermbenchChunks`). Fails closed on Normal-sized
/// requests to prevent accidental 311/605 MiB concatenation.
pub fn payload_concat(variant: TermbenchVariant, frames: usize) -> Vec<u8> {
    // Fail closed: never concatenate Normal (8192) or larger payloads.
    assert!(
        frames < TERMBENCH_NORMAL_FRAMES,
        "payload_concat: frames={frames} would materialize ~{} MiB; use TermbenchChunks streaming",
        (frames as u64 * TERMBENCH_FGBG_NORMAL_BYTES / TERMBENCH_NORMAL_FRAMES as u64)
            / (1024 * 1024)
    );
    let mut out = Vec::new();
    // Caller responsibility: do not call for Normal in tests unless bounded.
    // Reserve approximate size for Small to avoid repeated realloc.
    let estimate = match (variant, frames) {
        (TermbenchVariant::FGPerChar, TERMBENCH_SMALL_FRAMES) => TERMBENCH_FG_SMALL_BYTES as usize,
        (TermbenchVariant::FGBGPerChar, TERMBENCH_SMALL_FRAMES) => {
            TERMBENCH_FGBG_SMALL_BYTES as usize
        }
        _ => frames * 40000,
    };
    out.reserve(estimate.min(64 * 1024 * 1024));
    for f in 0..frames as u32 {
        append_frame(&mut out, variant, f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_per_frame_is_2025() {
        assert_eq!(TERMBENCH_CELLS_PER_FRAME, 2025);
        assert_eq!(TERMBENCH_COLS * TERMBENCH_ROWS, 2025);
        // Loops are inclusive.
        let cols = (0..=TERMBENCH_WIDTH_INCLUSIVE).count();
        let rows = (0..=TERMBENCH_HEIGHT_INCLUSIVE).count();
        assert_eq!(cols * rows, 2025);
    }

    #[test]
    fn frame_counts_are_exact() {
        assert_eq!(TERMBENCH_SMALL_FRAMES, 512);
        assert_eq!(TERMBENCH_NORMAL_FRAMES, 8192);
    }

    #[test]
    fn decimal_is_variable_width_no_leading_zeros() {
        for n in 0u32..=255 {
            let mut out = Vec::new();
            append_decimal(&mut out, n);
            assert_eq!(out, n.to_string().as_bytes());
        }
        let mut out = Vec::new();
        append_decimal(&mut out, 0);
        assert_eq!(out, b"0");
        out.clear();
        append_decimal(&mut out, 10);
        assert_eq!(out, b"10");
        out.clear();
        append_decimal(&mut out, 100);
        assert_eq!(out, b"100");
    }

    #[test]
    fn cup_encoding_matches_termbench() {
        let mut out = Vec::new();
        append_cup(&mut out, 1, 1);
        assert_eq!(out, b"\x1b[1;1H");
        out.clear();
        append_cup(&mut out, 1, 25);
        assert_eq!(out, b"\x1b[25;1H");
    }

    #[test]
    fn color_separate_sgr_not_combined() {
        let mut out = Vec::new();
        append_color(&mut out, true, 1, 2, 3);
        assert_eq!(out, b"\x1b[38;2;1;2;3m");
        out.clear();
        append_color(&mut out, false, 4, 5, 6);
        assert_eq!(out, b"\x1b[48;2;4;5;6m");
        // FGBGPerChar emits two separate SGRs, not one combined CSI.
        let mut fg = Vec::new();
        append_color(&mut fg, false, 10, 20, 30);
        append_color(&mut fg, true, 40, 50, 60);
        assert!(fg.windows(2).any(|w| w == b"\x1b[")); // two ESC[
        // Must contain exactly two 'm' terminators.
        assert_eq!(fg.iter().filter(|&&b| b == b'm').count(), 2);
    }

    #[test]
    fn frame_payload_has_2025_cells_and_25_cups() {
        for variant in [TermbenchVariant::FGPerChar, TermbenchVariant::FGBGPerChar] {
            let bytes = frame_payload(variant, 0);
            // Count CUPs: ESC[*. *;1H patterns for 25 rows.
            let cups = bytes.windows(2).filter(|w| *w == b"\x1b[").count();
            // CUPs + per-cell colors each contain ESC[, so count CUPs via ";1H".
            let cup_markers = bytes.windows(3).filter(|w| *w == b";1H").count();
            assert_eq!(
                cup_markers, TERMBENCH_ROWS,
                "variant {variant}: expected 25 CUPs, got {cup_markers}"
            );
            let _ = cups; // keep.
            // Count payload chars: each cell ends with a in a..y
            // Instead assert cell count via oracle: frame length must decode to 2025 content chars.
            // Extract char after each SGR terminator: harder; count 'a'..='y' following 'm'.
            let mut cells = 0usize;
            for i in 0..bytes.len() {
                if bytes[i] == b'm' {
                    if i + 1 < bytes.len() && (b'a'..=b'y').contains(&bytes[i + 1]) {
                        // For FGBG there are two 'm' per cell; only the second precedes char.
                        // Count when the next byte after 'm' is a cell char and either we are FGPerChar
                        // or we have seen that the prior char was the bg's 'm' (so two in a row not valid).
                        // Simpler: count occurrences of pattern "m<a..y>" and divide logic.
                        cells += 1;
                    }
                }
            }
            // For FGPerChar each cell has exactly one m<char>, so cells ==2025.
            // For FGBGPerChar each cell has two m's but only the second precedes char => also 2025.
            assert_eq!(
                cells, TERMBENCH_CELLS_PER_FRAME,
                "variant {variant} frame 0 cell count {cells}"
            );
        }
    }

    #[test]
    fn oracle_sampled_rgb_matches_termbench_cpp_anchors() {
        // Anchors from termbench.cpp contract (FGPerChar / FGBGPerChar):
        // F=0 Y=0 X=0 => fg 0,0,0 bg 0,0,0 char a
        assert_eq!(expected_fg_rgb(0, 0, 0), [0, 0, 0]);
        assert_eq!(expected_bg_rgb(0, 0, 0), [0, 0, 0]);
        assert_eq!(expected_char(0, 0, 0), b'a');
        // F=0 Y=0 X=80 => fg 0,0,80 bg 80,0,0 char f ( (0+80)%25=5 -> f )
        assert_eq!(expected_fg_rgb(0, 0, 80), [0, 0, 80]);
        assert_eq!(expected_bg_rgb(0, 0, 80), [80, 0, 0]);
        assert_eq!(expected_char(0, 0, 80), b'f');
        // F=0 Y=24 X=80
        assert_eq!(expected_fg_rgb(0, 24, 80), [0, 24, 104]);
        assert_eq!(expected_bg_rgb(0, 24, 80), [104, 24, 0]);
        assert_eq!(expected_char(0, 24, 80), b'e');
        // F=1 Y=0 X=0
        assert_eq!(expected_fg_rgb(1, 0, 0), [1, 1, 1]);
        assert_eq!(expected_bg_rgb(1, 0, 0), [1, 1, 1]);
        assert_eq!(expected_char(1, 0, 0), b'b');
        // F=33 Y=0 X=0
        assert_eq!(expected_fg_rgb(33, 0, 0), [33, 33, 33]);
        assert_eq!(expected_bg_rgb(33, 0, 0), [33, 33, 33]);
        assert_eq!(expected_char(33, 0, 0), b'i');
    }

    #[test]
    fn streaming_identity_matches_pinned_hashes_small() {
        // Small uses bounded streaming same as workload pre-pass.
        let fg = streaming_fnv1a64(TermbenchVariant::FGPerChar, TERMBENCH_SMALL_FRAMES);
        assert_eq!(fg, TERMBENCH_FG_SMALL_FNV1A64);
        let fgbg = streaming_fnv1a64(TermbenchVariant::FGBGPerChar, TERMBENCH_SMALL_FRAMES);
        assert_eq!(fgbg, TERMBENCH_FGBG_SMALL_FNV1A64);
    }

    #[test]
    fn total_bytes_matches_pinned_constants_small() {
        assert_eq!(
            total_bytes(TermbenchVariant::FGPerChar, TERMBENCH_SMALL_FRAMES),
            TERMBENCH_FG_SMALL_BYTES
        );
        assert_eq!(
            total_bytes(TermbenchVariant::FGBGPerChar, TERMBENCH_SMALL_FRAMES),
            TERMBENCH_FGBG_SMALL_BYTES
        );
    }

    #[test]
    fn spec_json_includes_required_metadata() {
        let spec = spec_json(TermbenchVariant::FGPerChar, TERMBENCH_SMALL_FRAMES);
        assert_eq!(spec["variant"], "fg_per_char");
        assert_eq!(spec["frames"], 512);
        assert_eq!(spec["cells_per_frame"], 2025);
        assert_eq!(spec["grid"], serde_json::json!([81, 25]));
        assert_eq!(
            spec["total_bytes"],
            serde_json::json!(TERMBENCH_FG_SMALL_BYTES)
        );
        assert_eq!(
            spec["fnv1a64"],
            serde_json::json!(TERMBENCH_FG_SMALL_FNV1A64)
        );
        assert_eq!(
            spec["chunk_bytes"],
            serde_json::json!(TERMBENCH_CHUNK_BYTES)
        );
    }
    #[test]
    fn chunks_generator_preserves_identity_and_bounded_size() {
        let variant = TermbenchVariant::FGPerChar;
        let frames = 8usize;
        let chunk_bytes = 4096usize;
        // Reference streaming hash.
        let reference = streaming_fnv1a64_with_chunk(variant, frames, chunk_bytes);
        // Chunks iterator hash.
        let mut h2 = FNV_OFFSET;
        let mut total = 0usize;
        for chunk in TermbenchChunks::new(variant, frames, chunk_bytes) {
            assert!(chunk.len() <= chunk_bytes);
            assert!(!chunk.is_empty());
            for &b in &chunk {
                h2 ^= u64::from(b);
                h2 = h2.wrapping_mul(FNV_PRIME);
            }
            total += chunk.len();
        }
        assert_eq!(format!("{h2:016x}"), reference);
        assert!(total > 0);
    }

    #[test]
    #[should_panic(expected = "chunk_bytes must be non-zero")]
    fn termbench_chunks_rejects_zero_chunk_bytes() {
        let _ = TermbenchChunks::new(TermbenchVariant::FGPerChar, 8, 0);
    }

    #[test]
    #[should_panic(expected = "chunk_bytes must be non-zero")]
    fn streaming_fnv1a64_rejects_zero_chunk_bytes() {
        let _ = streaming_fnv1a64_with_chunk(TermbenchVariant::FGPerChar, 8, 0);
    }
}
