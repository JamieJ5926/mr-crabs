//! Terminal snapshot binary representation and codec, ported from Ghostty
//! `src/terminal/snapshot/**`.
//!
//! A snapshot is one 10-byte envelope (`GHOSTSNP` + version 1) followed by a
//! sequence of records. Every record is framed as a fixed 10-byte header
//! (tag `u16`, payload length `u32`, CRC32C `u32`; all little-endian)
//! followed by its payload. The CRC covers the tag, length, and payload.
//!
//! Record order mirrors Ghostty: `TERMINAL`, `SCREEN`, `CONTINUATION`,
//! `READY`, `HISTORY`, `FINISH`. `READY` marks the renderable state; bytes
//! after `FINISH` belong to the transport and are not consumed.
//!
//! All decoding is bounded: record payloads are capped by
//! [`DecodeOptions::max_record_bytes`], continuation bytes by
//! [`DecodeOptions::max_continuation_bytes`], and the complete snapshot by
//! [`DecodeOptions::max_total_bytes`].

/// CRC32C (Castagnoli), table-driven software implementation.
pub mod crc32c {
    const TABLE: [u32; 256] = build_table();

    const fn build_table() -> [u32; 256] {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut crc = i as u32;
            let mut j = 0;
            while j < 8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0x82F63B78
                } else {
                    crc >> 1
                };
                j += 1;
            }
            table[i] = crc;
            i += 1;
        }
        table
    }

    pub fn update(crc: u32, bytes: &[u8]) -> u32 {
        let mut crc = crc;
        for &b in bytes {
            crc = TABLE[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
        }
        crc
    }

    pub fn checksum(bytes: &[u8]) -> u32 {
        update(0xFFFF_FFFF, bytes) ^ 0xFFFF_FFFF
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn known_vectors() {
            // RFC 3720 / iSCSI reflects iSCSI crc32c: init/final FFFFFFFF,
            // poly 0x82F63B78. Verified: crc32c("I123456789")==0x0BBD2F72.
            assert_eq!(super::checksum(b""), 0);
            assert_eq!(super::checksum(b"123456789"), 0xE306_9283);
            assert_eq!(super::checksum(b"I123456789"), 0x0BBD_2F72);
        }
    }
}

/// Identifies the layout and meaning of a record payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum Tag {
    Terminal = 1,
    Screen = 2,
    Page = 3,
    History = 4,
    Ready = 5,
    Finish = 6,
    Continuation = 7,
}

impl Tag {
    fn from_u16(v: u16) -> Option<Tag> {
        Some(match v {
            1 => Tag::Terminal,
            2 => Tag::Screen,
            3 => Tag::Page,
            4 => Tag::History,
            5 => Tag::Ready,
            6 => Tag::Finish,
            7 => Tag::Continuation,
            _ => return None,
        })
    }
}

/// Envelope constants (Ghostty `snapshot/envelope.zig`).
pub mod envelope {
    use super::DecodeError;

    pub const MAGIC: &[u8; 8] = b"GHOSTSNP";
    pub const VERSION: u16 = 1;
    pub const ENCODED_LEN: usize = 10;

    pub fn encode(out: &mut Vec<u8>) {
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<(), DecodeError> {
        if bytes.len() < ENCODED_LEN {
            return Err(DecodeError::EndOfStream);
        }
        if &bytes[..8] != MAGIC {
            return Err(DecodeError::InvalidMagic);
        }
        let version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if version != VERSION {
            return Err(DecodeError::UnsupportedVersion);
        }
        Ok(())
    }
}

/// Errors possible while decoding a snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    EndOfStream,
    InvalidMagic,
    UnsupportedVersion,
    InvalidTag,
    InvalidChecksum,
    PayloadTooLarge,
    TotalTooLarge,
    ContinuationLimitExceeded,
    UnexpectedRecordTag,
    TruncatedRecord,
    PayloadDecode(String),
    TrailingGarbage,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for DecodeError {}

/// Errors possible while encoding a snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EncodeError {
    PayloadTooLarge,
    PayloadEncode(String),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for EncodeError {}

/// A payload that can be stored in a snapshot record.
pub trait SnapshotPayload: Clone + std::fmt::Debug + Eq + PartialEq {
    fn encode_payload(&self, out: &mut Vec<u8>) -> Result<(), EncodeError>;
    fn decode_payload(bytes: &[u8]) -> Result<Self, DecodeError>;
}

/// The continuation needed to bring the VT parser back to its state at the
/// snapshot cut (Ghostty `continuation.Value`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Continuation {
    /// Neither the VT parser nor UTF-8 decoder has unfinished state.
    Ground,
    /// Canonical replay-safe PTY bytes; empty is equivalent to `Ground`.
    Bytes(Vec<u8>),
}

impl Continuation {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Ground => &[],
            Self::Bytes(b) => b,
        }
    }

    /// The continuation is valid if it is ground or contains fewer than
    /// 2^32 bytes (the record payload length limit).
    pub fn validate(&self) -> Result<(), EncodeError> {
        if matches!(self, Self::Bytes(b) if b.len() > u32::MAX as usize) {
            return Err(EncodeError::PayloadTooLarge);
        }
        Ok(())
    }
}

/// The complete content of one snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotContent<P> {
    /// TERMINAL record payload (terminal-wide state).
    pub terminal: P,
    /// SCREEN record payload (the visible screen).
    pub screen: P,
    /// CONTINUATION record.
    pub continuation: Continuation,
    /// HISTORY record payload (older scrollback state), if any.
    pub history: Option<P>,
}

/// Decode options and bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeOptions {
    /// Largest single record payload to accept.
    pub max_record_bytes: usize,
    /// Largest non-ground continuation the decoder may accept.
    pub max_continuation_bytes: usize,
    /// Largest complete snapshot (envelope + all records).
    pub max_total_bytes: usize,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_record_bytes: 64 * 1024 * 1024,
            max_continuation_bytes: 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
        }
    }
}

/// Encode one complete snapshot.
pub fn encode<P: SnapshotPayload>(
    content: &SnapshotContent<P>,
    out: &mut Vec<u8>,
) -> Result<(), EncodeError> {
    content.continuation.validate()?;

    envelope::encode(out);
    write_record(Tag::Terminal, &encode_payload(&content.terminal)?, out);
    write_record(Tag::Screen, &encode_payload(&content.screen)?, out);
    write_record(Tag::Continuation, content.continuation.bytes(), out);
    write_record(Tag::Ready, &[], out);
    if let Some(history) = &content.history {
        write_record(Tag::History, &encode_payload(history)?, out);
    }
    write_record(Tag::Finish, &[], out);
    Ok(())
}

fn encode_payload<P: SnapshotPayload>(payload: &P) -> Result<Vec<u8>, EncodeError> {
    let mut bytes = Vec::new();
    payload.encode_payload(&mut bytes)?;
    if bytes.len() > u32::MAX as usize {
        return Err(EncodeError::PayloadTooLarge);
    }
    Ok(bytes)
}

fn write_record(tag: Tag, payload: &[u8], out: &mut Vec<u8>) {
    let payload_len = payload.len() as u32;
    let mut crc_input = Vec::with_capacity(6 + payload.len());
    crc_input.extend_from_slice(&(tag as u16).to_le_bytes());
    crc_input.extend_from_slice(&payload_len.to_le_bytes());
    crc_input.extend_from_slice(payload);
    let crc = crc32c::checksum(&crc_input);
    out.extend_from_slice(&(tag as u16).to_le_bytes());
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(payload);
}

/// One decoded record.
struct Record {
    tag: Tag,
    payload: Vec<u8>,
}

fn read_record(
    bytes: &[u8],
    offset: usize,
    options: DecodeOptions,
) -> Result<(Record, usize), DecodeError> {
    if bytes.len() - offset < 10 {
        return Err(DecodeError::EndOfStream);
    }
    let tag_raw = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let tag = Tag::from_u16(tag_raw).ok_or(DecodeError::InvalidTag)?;
    let payload_len = u32::from_le_bytes([
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
    ]) as usize;
    let crc_expected = u32::from_le_bytes([
        bytes[offset + 6],
        bytes[offset + 7],
        bytes[offset + 8],
        bytes[offset + 9],
    ]);
    if payload_len > options.max_record_bytes {
        return Err(DecodeError::PayloadTooLarge);
    }
    if offset + 10 + payload_len > bytes.len() {
        return Err(DecodeError::TruncatedRecord);
    }
    let payload = &bytes[offset + 10..offset + 10 + payload_len];
    let mut crc_input = Vec::with_capacity(6 + payload_len);
    crc_input.extend_from_slice(&tag_raw.to_le_bytes());
    crc_input.extend_from_slice(&(payload_len as u32).to_le_bytes());
    crc_input.extend_from_slice(payload);
    if crc32c::checksum(&crc_input) != crc_expected {
        return Err(DecodeError::InvalidChecksum);
    }
    Ok((
        Record {
            tag,
            payload: payload.to_vec(),
        },
        offset + 10 + payload_len,
    ))
}

/// A decoded snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedSnapshot<P> {
    pub terminal: P,
    pub screen: P,
    pub history: Option<P>,
    pub continuation: Continuation,
    /// Bytes consumed through FINISH (bytes after FINISH belong to the
    /// containing transport).
    pub bytes_consumed: usize,
}

/// Decode exactly one snapshot. Bytes after FINISH are left unconsumed.
pub fn decode<P: SnapshotPayload>(
    bytes: &[u8],
    options: DecodeOptions,
) -> Result<DecodedSnapshot<P>, DecodeError> {
    if bytes.len() > options.max_total_bytes {
        return Err(DecodeError::TotalTooLarge);
    }
    envelope::decode(bytes)?;

    let mut offset = envelope::ENCODED_LEN;
    let mut terminal: Option<P> = None;
    let mut screen: Option<P> = None;
    let mut history: Option<P> = None;
    let mut continuation = Continuation::Ground;
    let mut saw_ready = false;

    loop {
        let (record, next) = read_record(bytes, offset, options)?;
        offset = next;
        match record.tag {
            Tag::Terminal => {
                if terminal.is_some() || saw_ready {
                    return Err(DecodeError::UnexpectedRecordTag);
                }
                terminal = Some(P::decode_payload(&record.payload)?);
            }
            Tag::Screen => {
                if screen.is_some() || saw_ready {
                    return Err(DecodeError::UnexpectedRecordTag);
                }
                screen = Some(P::decode_payload(&record.payload)?);
            }
            Tag::History => {
                if !saw_ready {
                    return Err(DecodeError::UnexpectedRecordTag);
                }
                history = Some(P::decode_payload(&record.payload)?);
            }
            Tag::Continuation => {
                if saw_ready || continuation != Continuation::Ground {
                    return Err(DecodeError::UnexpectedRecordTag);
                }
                if record.payload.len() > options.max_continuation_bytes {
                    return Err(DecodeError::ContinuationLimitExceeded);
                }
                continuation = if record.payload.is_empty() {
                    Continuation::Ground
                } else {
                    Continuation::Bytes(record.payload)
                };
            }
            Tag::Ready => {
                if saw_ready {
                    return Err(DecodeError::UnexpectedRecordTag);
                }
                saw_ready = true;
            }
            Tag::Finish => break,
            Tag::Page => return Err(DecodeError::UnexpectedRecordTag),
        }
    }

    let terminal = terminal.ok_or(DecodeError::UnexpectedRecordTag)?;
    let screen = screen.ok_or(DecodeError::UnexpectedRecordTag)?;
    if !saw_ready {
        return Err(DecodeError::UnexpectedRecordTag);
    }
    Ok(DecodedSnapshot {
        terminal,
        screen,
        history,
        continuation,
        bytes_consumed: offset,
    })
}

/// An incremental decoder: the terminal becomes usable at READY, before
/// history arrives (Ghostty `snapshot.Decoder`).
pub struct Decoder<'a, P: SnapshotPayload> {
    bytes: &'a [u8],
    offset: usize,
    options: DecodeOptions,
    state: DecoderState,
    _marker: std::marker::PhantomData<P>,
}

enum DecoderState {
    Ready,
    History,
    Finished,
}

impl<'a, P: SnapshotPayload> Decoder<'a, P> {
    /// Validate the envelope and prepare the decoder.
    pub fn init(bytes: &'a [u8], options: DecodeOptions) -> Result<Self, DecodeError> {
        if bytes.len() > options.max_total_bytes {
            return Err(DecodeError::TotalTooLarge);
        }
        envelope::decode(bytes)?;
        Ok(Self {
            bytes,
            offset: envelope::ENCODED_LEN,
            options,
            state: DecoderState::Ready,
            _marker: std::marker::PhantomData,
        })
    }

    /// Decode through READY, returning the renderable state.
    pub fn ready(&mut self) -> Result<DecoderReady<P>, DecodeError> {
        let mut terminal: Option<P> = None;
        let mut screen: Option<P> = None;
        let mut continuation = Continuation::Ground;
        loop {
            let (record, next) = read_record(self.bytes, self.offset, self.options)?;
            self.offset = next;
            match record.tag {
                Tag::Terminal => {
                    if terminal.is_some() {
                        return Err(DecodeError::UnexpectedRecordTag);
                    }
                    terminal = Some(P::decode_payload(&record.payload)?);
                }
                Tag::Screen => {
                    if screen.is_some() {
                        return Err(DecodeError::UnexpectedRecordTag);
                    }
                    screen = Some(P::decode_payload(&record.payload)?);
                }
                Tag::Continuation => {
                    if record.payload.len() > self.options.max_continuation_bytes {
                        return Err(DecodeError::ContinuationLimitExceeded);
                    }
                    continuation = if record.payload.is_empty() {
                        Continuation::Ground
                    } else {
                        Continuation::Bytes(record.payload)
                    };
                }
                Tag::Ready => {
                    self.state = DecoderState::History;
                    return Ok(DecoderReady {
                        terminal: terminal.ok_or(DecodeError::UnexpectedRecordTag)?,
                        screen: screen.ok_or(DecodeError::UnexpectedRecordTag)?,
                        continuation,
                    });
                }
                _ => return Err(DecodeError::UnexpectedRecordTag),
            }
        }
    }

    /// Apply the next history page; returns `None` once FINISH validates.
    pub fn next_history(&mut self) -> Result<Option<P>, DecodeError> {
        match self.state {
            DecoderState::Finished => return Ok(None),
            DecoderState::Ready => return Err(DecodeError::UnexpectedRecordTag),
            DecoderState::History => {}
        }
        let (record, next) = read_record(self.bytes, self.offset, self.options)?;
        match record.tag {
            Tag::History => {
                self.offset = next;
                Ok(Some(P::decode_payload(&record.payload)?))
            }
            Tag::Finish => {
                self.offset = next;
                self.state = DecoderState::Finished;
                Ok(None)
            }
            _ => Err(DecodeError::UnexpectedRecordTag),
        }
    }

    /// Total bytes consumed so far.
    pub fn bytes_consumed(&self) -> usize {
        self.offset
    }
}

/// The renderable state at READY.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecoderReady<P> {
    pub terminal: P,
    pub screen: P,
    pub continuation: Continuation,
}

/// Errors possible while replaying a recorded stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayError {
    /// The target rejected the bytes.
    TargetRejected,
}

/// A target that can consume replayed bytes (implemented by the terminal).
pub trait ReplayTarget {
    fn feed(&mut self, bytes: &[u8]) -> Result<(), ReplayError>;
}

/// Replay a recorded byte log through a target in bounded chunks. Returns
/// the number of bytes replayed.
pub fn replay_log(log: &[u8], target: &mut impl ReplayTarget) -> Result<usize, ReplayError> {
    const CHUNK: usize = 4096;
    let mut fed = 0;
    for chunk in log.chunks(CHUNK) {
        target.feed(chunk)?;
        fed += chunk.len();
    }
    Ok(fed)
}

/// A bounded recorder that captures a byte log with optional snapshot
/// checkpoints for later replay verification.
pub struct ReplayRecorder {
    log: Vec<u8>,
    /// Maximum bytes retained; overflow rejects further recording.
    pub max_bytes: usize,
}

impl ReplayRecorder {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            log: Vec::new(),
            max_bytes,
        }
    }

    /// Append bytes; returns `false` (and records nothing) when the log
    /// would exceed the bound.
    pub fn record(&mut self, bytes: &[u8]) -> bool {
        if self.log.len() + bytes.len() > self.max_bytes {
            return false;
        }
        self.log.extend_from_slice(bytes);
        true
    }

    pub fn log(&self) -> &[u8] {
        &self.log
    }

    pub fn len(&self) -> usize {
        self.log.len()
    }

    pub fn is_empty(&self) -> bool {
        self.log.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Payload(Vec<u8>);

    impl SnapshotPayload for Payload {
        fn encode_payload(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
            out.extend_from_slice(&(self.0.len() as u32).to_le_bytes());
            out.extend_from_slice(&self.0);
            Ok(())
        }
        fn decode_payload(bytes: &[u8]) -> Result<Self, DecodeError> {
            if bytes.len() < 4 {
                return Err(DecodeError::TruncatedRecord);
            }
            let len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
            if 4 + len > bytes.len() {
                return Err(DecodeError::TruncatedRecord);
            }
            Ok(Payload(bytes[4..4 + len].to_vec()))
        }
    }

    fn sample() -> SnapshotContent<Payload> {
        SnapshotContent {
            terminal: Payload(b"terminal-state".to_vec()),
            screen: Payload(b"screen-state".to_vec()),
            continuation: Continuation::Ground,
            history: Some(Payload(b"history-state".to_vec())),
        }
    }

    #[test]
    fn roundtrip() {
        let mut bytes = Vec::new();
        encode(&sample(), &mut bytes).unwrap();
        let decoded = decode::<Payload>(&bytes, DecodeOptions::default()).unwrap();
        assert_eq!(decoded.terminal, Payload(b"terminal-state".to_vec()));
        assert_eq!(decoded.screen, Payload(b"screen-state".to_vec()));
        assert_eq!(decoded.history, Some(Payload(b"history-state".to_vec())));
        assert_eq!(decoded.continuation, Continuation::Ground);
        assert_eq!(decoded.bytes_consumed, bytes.len());
    }

    #[test]
    fn continuation_roundtrip() {
        let mut content = sample();
        content.continuation = Continuation::Bytes(b"\x1b]133;C".to_vec());
        let mut bytes = Vec::new();
        encode(&content, &mut bytes).unwrap();
        let decoded = decode::<Payload>(&bytes, DecodeOptions::default()).unwrap();
        assert_eq!(
            decoded.continuation,
            Continuation::Bytes(b"\x1b]133;C".to_vec())
        );
    }

    #[test]
    fn envelope_rejects_bad_magic_and_version() {
        let mut bytes = Vec::new();
        encode(&sample(), &mut bytes).unwrap();
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert_eq!(
            decode::<Payload>(&bad, DecodeOptions::default()),
            Err(DecodeError::InvalidMagic)
        );
        let mut bad = bytes.clone();
        bad[9] = 2;
        assert_eq!(
            decode::<Payload>(&bad, DecodeOptions::default()),
            Err(DecodeError::UnsupportedVersion)
        );
    }

    #[test]
    fn every_truncation_rejected() {
        let mut bytes = Vec::new();
        encode(&sample(), &mut bytes).unwrap();
        for len in 0..bytes.len() {
            let result = decode::<Payload>(&bytes[..len], DecodeOptions::default());
            assert!(result.is_err(), "truncation at {len} must fail");
        }
    }

    #[test]
    fn checksum_corruption_rejected() {
        let mut bytes = Vec::new();
        encode(&sample(), &mut bytes).unwrap();
        // Flip a payload byte inside the TERMINAL record (after the header).
        let flip = envelope::ENCODED_LEN + 10 + 4;
        bytes[flip] ^= 0xFF;
        assert_eq!(
            decode::<Payload>(&bytes, DecodeOptions::default()),
            Err(DecodeError::InvalidChecksum)
        );
    }

    #[test]
    fn unknown_tag_rejected() {
        let mut bytes = Vec::new();
        encode(&sample(), &mut bytes).unwrap();
        // Overwrite the TERMINAL record tag with 0xFFFF.
        bytes[envelope::ENCODED_LEN] = 0xFF;
        bytes[envelope::ENCODED_LEN + 1] = 0xFF;
        assert_eq!(
            decode::<Payload>(&bytes, DecodeOptions::default()),
            Err(DecodeError::InvalidTag)
        );
    }

    #[test]
    fn payload_bound_enforced() {
        let mut bytes = Vec::new();
        encode(&sample(), &mut bytes).unwrap();
        let options = DecodeOptions {
            max_record_bytes: 4,
            ..Default::default()
        };
        assert_eq!(
            decode::<Payload>(&bytes, options),
            Err(DecodeError::PayloadTooLarge)
        );
    }

    #[test]
    fn continuation_bound_enforced() {
        let mut content = sample();
        content.continuation = Continuation::Bytes(vec![b'x'; 16]);
        let mut bytes = Vec::new();
        encode(&content, &mut bytes).unwrap();
        let options = DecodeOptions {
            max_continuation_bytes: 8,
            ..Default::default()
        };
        assert_eq!(
            decode::<Payload>(&bytes, options),
            Err(DecodeError::ContinuationLimitExceeded)
        );
    }

    #[test]
    fn incremental_decoder() {
        let mut bytes = Vec::new();
        encode(&sample(), &mut bytes).unwrap();
        let mut decoder = Decoder::<Payload>::init(&bytes, DecodeOptions::default()).unwrap();
        let ready = decoder.ready().unwrap();
        assert_eq!(ready.terminal, Payload(b"terminal-state".to_vec()));
        assert_eq!(ready.screen, Payload(b"screen-state".to_vec()));
        let history = decoder.next_history().unwrap().unwrap();
        assert_eq!(history, Payload(b"history-state".to_vec()));
        assert_eq!(decoder.next_history().unwrap(), None);
        assert_eq!(decoder.next_history().unwrap(), None);
        assert_eq!(decoder.bytes_consumed(), bytes.len());
    }

    #[test]
    fn replay_recorder_bounded() {
        struct RecordingTarget(Vec<u8>);
        impl ReplayTarget for RecordingTarget {
            fn feed(&mut self, bytes: &[u8]) -> Result<(), ReplayError> {
                self.0.extend_from_slice(bytes);
                Ok(())
            }
        }

        let mut r = ReplayRecorder::new(10);
        assert!(r.record(b"0123456789"));
        assert!(!r.record(b"X"));
        assert_eq!(r.len(), 10);
        let mut target = RecordingTarget(Vec::new());
        let fed = replay_log(r.log(), &mut target).unwrap();
        assert_eq!(fed, 10);
        assert_eq!(target.0, b"0123456789");
    }
}
