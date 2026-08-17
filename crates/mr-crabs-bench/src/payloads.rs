//! Deterministic S12 payload generation (S12 corpus/replay fixtures).
//!
//! Every payload here is a pure function of a byte index, so generation is
//! deterministic across runs, processes, and toolchains. The canonical specs
//! and their FNV-1a 64 identity hashes are pinned in
//! `verification/corpus/replay/payloads.json`; the gate driver refuses to
//! compare a workload against the oracle baseline unless the bench-reported
//! payload spec AND identity hash both equal the pinned corpus values.

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Size of the 10 MiB throughput payloads, in bytes.
pub const TEN_MIB: usize = 10 * 1024 * 1024;
/// `ascii_10mb` payload byte count.
pub const ASCII_10MB_BYTES: usize = TEN_MIB;
/// `unicode_10mb` payload byte count.
pub const UNICODE_10MB_BYTES: usize = TEN_MIB;
/// `scrollback_1m` logical line count.
pub const SCROLLBACK_LINES: usize = 1_000_000;
/// `scrollback_1m` payload byte count: one million `line\n` records (5 bytes
/// each), byte-identical to the S0 oracle workload.
pub const SCROLLBACK_BYTES: usize = SCROLLBACK_LINES * 5;

/// Pinned FNV-1a 64 identity of the canonical `ascii_10mb` payload
/// (see `verification/corpus/replay/payloads.json`).
pub const ASCII_10MB_FNV1A64: &str = "69822425ae3d3bd5";
/// Pinned FNV-1a 64 identity of the canonical `unicode_10mb` payload.
pub const UNICODE_10MB_FNV1A64: &str = "1c42a1a0dd376cec";
/// Pinned FNV-1a 64 identity of the canonical `scrollback_1m` payload.
pub const SCROLLBACK_FNV1A64: &str = "562435e19e354a25";

/// Byte at index `j` of the canonical ASCII payload.
///
/// Composition (`ascii_sgr_cycle32`): a 32-byte cycle of `ESC[31m` (bytes
/// 0-4), `ESC[0m` (bytes 5-8), then 23 printable ASCII letters `A`..`Z`
/// cycling every 32 bytes. Exactly one byte is produced per index.
pub fn ascii_byte(j: u64) -> u8 {
    match j % 32 {
        0 => 0x1b,
        1 => b'[',
        2 => b'3',
        3 => b'1',
        4 => b'm',
        5 => 0x1b,
        6 => b'[',
        7 => b'0',
        8 => b'm',
        _ => b'A' + ((j / 32) % 26) as u8,
    }
}

/// Byte at index `j` of the canonical mixed-Unicode payload.
///
/// Composition (`unicode_48_cycle`): a 48-byte cycle of `ESC[31m` (bytes
/// 0-4), `ESC[0m` (bytes 5-8), three 3-byte UTF-8 CJK codepoints from
/// `U+4E00..U+7DFF` (bytes 9-17), then 30 ASCII letters (bytes 18-47).
pub fn unicode_byte(j: u64) -> u8 {
    // Ten MiB ends one byte into the cycle's third CJK codepoint. Replace
    // that otherwise-incomplete UTF-8 lead byte with ASCII so the fixed-size
    // benchmark stream remains valid UTF-8 without another allocation.
    if j + 1 == UNICODE_10MB_BYTES as u64 {
        return b'A';
    }
    let k = j / 48;
    match j % 48 {
        0 => 0x1b,
        1 => b'[',
        2 => b'3',
        3 => b'1',
        4 => b'm',
        5 => 0x1b,
        6 => b'[',
        7 => b'0',
        8 => b'm',
        r @ 9..=17 => {
            let c = (r - 9) / 3;
            let cp: u32 = 0x4E00 + ((k * 3 + c) % 0x3000) as u32;
            let bytes = [
                0xE0 | (cp >> 12) as u8,
                0x80 | ((cp >> 6) & 0x3F) as u8,
                0x80 | (cp & 0x3F) as u8,
            ];
            bytes[((r - 9) % 3) as usize]
        }
        r => b'A' + ((k * 7 + r) % 26) as u8,
    }
}

/// The canonical 10 MiB ASCII payload.
pub fn ascii_payload() -> Vec<u8> {
    let mut out = Vec::with_capacity(ASCII_10MB_BYTES);
    for j in 0..ASCII_10MB_BYTES as u64 {
        out.push(ascii_byte(j));
    }
    out
}

/// The canonical 10 MiB mixed-Unicode payload.
pub fn unicode_payload() -> Vec<u8> {
    let mut out = Vec::with_capacity(UNICODE_10MB_BYTES);
    for j in 0..UNICODE_10MB_BYTES as u64 {
        out.push(unicode_byte(j));
    }
    out
}

/// The canonical 1M-line scrollback payload: `line\n` repeated one million
/// times (exactly 5,000,000 bytes), matching the S0 oracle workload.
pub fn scrollback_payload() -> Vec<u8> {
    let mut out = Vec::with_capacity(SCROLLBACK_BYTES);
    for _ in 0..SCROLLBACK_LINES {
        out.extend_from_slice(b"line\n");
    }
    out
}

/// FNV-1a 64 identity hash of `bytes`, hex-encoded (lowercase).
///
/// Deterministic across languages and platforms; used to pin payload
/// identity in the corpus manifest without a cryptographic dependency.
pub fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

/// Deterministic `n`-byte ASCII sample with a newline every 80 bytes
/// (used by the engine-count workloads so each engine accrues scrollback).
pub fn sample_ascii_lines(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n as u64 {
        out.push(if i % 80 == 79 {
            b'\n'
        } else {
            b'A' + ((i / 80) % 26) as u8
        });
    }
    out
}

/// Deterministic xorshift64 chunk for replay scripts.
///
/// `chunk(seed, step, len)` is a pure function of its arguments: identical
/// inputs produce identical bytes on every run and every platform.
pub fn seeded_chunk(seed: u64, step: u64, len: usize) -> Vec<u8> {
    let mut state = seed ^ step.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push((state & 0xFF) as u8);
    }
    out
}

/// S8 search workload history size (logical lines).
pub const SEARCH_LINES: usize = 200_000;
/// Every `SEARCH_NEEDLE_EVERY`-th line contains the needle.
pub const SEARCH_NEEDLE_EVERY: usize = 997;
/// The search needle (never appears in `plain line` rows).
pub const SEARCH_NEEDLE: &[u8] = b"needle";
/// Exact match count for `SEARCH_LINES` at `SEARCH_NEEDLE_EVERY` spacing.
pub const SEARCH_EXPECTED_MATCHES: usize = SEARCH_LINES / SEARCH_NEEDLE_EVERY + 1;

/// Deterministic search-history payload: `lines` CRLF-terminated rows of
/// `plain line`, except every `needle_every`-th row contains the needle.
pub fn search_history_payload(lines: usize, needle_every: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(lines * 11);
    for i in 0..lines {
        if i % needle_every == 0 {
            out.extend_from_slice(format!("needle line {i}\r\n").as_bytes());
        } else {
            out.extend_from_slice(b"plain line\r\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_payload_is_deterministic_and_pinned() {
        let a = ascii_payload();
        let b = ascii_payload();
        assert_eq!(a.len(), ASCII_10MB_BYTES);
        assert_eq!(a, b, "payload generation must be deterministic");
        assert_eq!(fnv1a64_hex(&a), ASCII_10MB_FNV1A64);
    }

    #[test]
    fn unicode_payload_is_deterministic_and_pinned() {
        let a = unicode_payload();
        let b = unicode_payload();
        assert_eq!(a.len(), UNICODE_10MB_BYTES);
        assert_eq!(a, b, "payload generation must be deterministic");
        assert_eq!(fnv1a64_hex(&a), UNICODE_10MB_FNV1A64);
        // All non-escape bytes must be valid UTF-8 (the terminal feed path
        // parses UTF-8 sequences; a malformed stream would change semantics).
        let text = String::from_utf8(a).expect("unicode payload must be valid UTF-8");
        assert!(
            text.contains('\u{4E00}'),
            "payload must contain CJK content"
        );
    }

    #[test]
    fn scrollback_payload_is_deterministic_and_pinned() {
        let a = scrollback_payload();
        let b = scrollback_payload();
        assert_eq!(a.len(), SCROLLBACK_BYTES);
        assert_eq!(a, b, "payload generation must be deterministic");
        assert_eq!(fnv1a64_hex(&a), SCROLLBACK_FNV1A64);
        assert_eq!(a.iter().filter(|&&b| b == b'\n').count(), SCROLLBACK_LINES);
    }

    #[test]
    fn ascii_prefix_matches_pinned_corpus_vector() {
        // 96-byte prefix, mirrored from verification/corpus/replay payload
        // generation (see payloads.json composition notes).
        let prefix: Vec<u8> = (0..96).map(ascii_byte).collect();
        let expected: &[u8] = &[
            0x1b, b'[', b'3', b'1', b'm', 0x1b, b'[', b'0', b'm', b'A', b'A', b'A', b'A', b'A',
            b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A',
            b'A', b'A', b'A', b'A', 0x1b, b'[', b'3', b'1', b'm', 0x1b, b'[', b'0', b'm', b'B',
            b'B', b'B', b'B', b'B', b'B', b'B', b'B', b'B', b'B', b'B', b'B', b'B', b'B', b'B',
            b'B', b'B', b'B', b'B', b'B', b'B', b'B', b'B', 0x1b, b'[', b'3', b'1', b'm', 0x1b,
            b'[', b'0', b'm', b'C', b'C', b'C', b'C', b'C', b'C', b'C', b'C', b'C', b'C', b'C',
            b'C', b'C', b'C', b'C', b'C', b'C', b'C', b'C', b'C', b'C', b'C', b'C',
        ];
        assert_eq!(prefix, expected);
    }

    #[test]
    fn unicode_prefix_matches_pinned_corpus_vector() {
        let prefix: Vec<u8> = (0..96).map(unicode_byte).collect();
        // Cycle 0: ESC[31m ESC[0m U+4E00 U+4E01 U+4E02 STUVWXYZABCDEFGHIJKLMNOPQRSTUV
        // Cycle 1: ESC[31m ESC[0m U+4E03 U+4E04 U+4E05 ZABCDEFGHIJKLMNOPQRSTUVWXYZABC
        let expected: Vec<u8> = {
            let mut v = vec![0x1b, b'[', b'3', b'1', b'm', 0x1b, b'[', b'0', b'm'];
            for cp in [0x4E00u32, 0x4E01, 0x4E02] {
                v.extend_from_slice(&[
                    0xE0 | (cp >> 12) as u8,
                    0x80 | ((cp >> 6) & 0x3F) as u8,
                    0x80 | (cp & 0x3F) as u8,
                ]);
            }
            v.extend_from_slice(b"STUVWXYZABCDEFGHIJKLMNOPQRSTUV");
            v.extend_from_slice(&[0x1b, b'[', b'3', b'1', b'm', 0x1b, b'[', b'0', b'm']);
            for cp in [0x4E03u32, 0x4E04, 0x4E05] {
                v.extend_from_slice(&[
                    0xE0 | (cp >> 12) as u8,
                    0x80 | ((cp >> 6) & 0x3F) as u8,
                    0x80 | (cp & 0x3F) as u8,
                ]);
            }
            v.extend_from_slice(b"ZABCDEFGHIJKLMNOPQRSTUVWXYZABC");
            v
        };
        assert_eq!(prefix, expected);
    }

    #[test]
    fn seeded_chunk_is_deterministic_and_distinguishes_steps() {
        let a = seeded_chunk(0x5eed, 3, 4096);
        let b = seeded_chunk(0x5eed, 3, 4096);
        let c = seeded_chunk(0x5eed, 4, 4096);
        assert_eq!(a.len(), 4096);
        assert_eq!(a, b);
        assert_ne!(a, c, "different steps must produce different chunks");
    }

    #[test]
    fn fnv1a64_known_vector() {
        // FNV-1a 64 of the empty string is the offset basis itself.
        assert_eq!(fnv1a64_hex(b""), format!("{FNV_OFFSET:016x}"));
        // Well-known vector: fnv1a64("foobar") from the reference test suite.
        assert_eq!(fnv1a64_hex(b"foobar"), "85944171f73967e8");
    }

    #[test]
    fn search_history_payload_is_deterministic_with_pinned_match_count() {
        let a = search_history_payload(SEARCH_LINES, SEARCH_NEEDLE_EVERY);
        let b = search_history_payload(SEARCH_LINES, SEARCH_NEEDLE_EVERY);
        assert_eq!(a, b, "search payload generation must be deterministic");
        // Count lines containing the needle by scanning row starts.
        let mut matches = 0usize;
        let mut row_start = 0usize;
        for (i, &byte) in a.iter().enumerate() {
            if byte == b'\n' {
                if a[row_start..i]
                    .windows(SEARCH_NEEDLE.len())
                    .any(|w| w == SEARCH_NEEDLE)
                {
                    matches += 1;
                }
                row_start = i + 1;
            }
        }
        assert_eq!(matches, SEARCH_EXPECTED_MATCHES);
        assert_eq!(
            a.iter().filter(|&&b| b == b'\n').count(),
            SEARCH_LINES,
            "payload must contain exactly SEARCH_LINES lines"
        );
    }
}
