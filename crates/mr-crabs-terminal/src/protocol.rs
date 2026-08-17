//! S6 protocol engine: OSC/DCS/APC parsing wired into [`crate::compact::CompactEngine`],
//! with Ghostty-compatible dispatch semantics.
//!
//! Byte flow ([`TerminalProtocol::feed`]):
//!
//! 1. A byte-level [`Scanner`] recognizes OSC (`ESC ]` / C1 `0x9D`),
//!    SOS/PM/APC (`ESC X` / `ESC ^` / `ESC _` / C1 `0x98`/`0x9E`/`0x9F`),
//!    and DCS (`ESC P` / C1 `0x90`) strings with the same `Pending` /
//!    `StreamPair` semantics as `mr_crabs_protocols::apc::Scanner`
//!    ([`ScanStep`]). String payload bytes never reach vte; the scanner
//!    feeds vte a synthetic no-op sequence that returns it to the ground
//!    state.
//! 2. OSC payload bytes feed [`mr_crabs_protocols::osc::Parser`]; at the
//!    terminator the typed [`Command`] is dispatched with Ghostty semantics:
//!    title/pwd storage, hyperlink application, semantic prompt state,
//!    palette effects, and sink notifications.
//! 3. SOS/PM/APC payload bytes feed [`mr_crabs_protocols::apc::Handler`];
//!    completed commands go to the sink (kitty graphics payloads are handed
//!    off for the graphics slice).
//! 4. DCS sequences are intercepted at the scanner level too: the intro
//!    (params/intermediates/final byte) drives
//!    [`mr_crabs_protocols::dcs::Handler::hook`], payload bytes feed `put`,
//!    and `ST`/abort drive `unhook`. vte 0.15's ansi `Processor` wraps the
//!    caller in a `Performer` whose `hook`/`put`/`unhook` are no-ops, so
//!    scanner-level interception is the only path that reaches the DCS
//!    handler. XTGETTCAP and DECRQSS replies are written back through the
//!    sink's `write_pty`; tmux control-mode notifications go to the sink.
//! 5. CSI reports (DA1/DA2/DA3, DSR, DECRQM, text-area size) and BEL are
//!    intercepted from the vte `Handler` interface and answered with the
//!    Ghostty encoders.
//!
//! The engine owns the mutable terminal state and the protocol state; there
//! is no locking and no allocation on the parse path beyond the bounded
//! captures.

use std::io::Write as _;

use mr_crabs_protocols::apc::{self, ScanStep};
use mr_crabs_protocols::color::{ColorTarget, DynamicColor, Rgb};
use mr_crabs_protocols::dcs::{self, CursorShapeKind, DcsIntro, DecrqssContext};
use mr_crabs_protocols::osc::{self, Command};
use mr_crabs_protocols::reports::{
    self, ColorScheme, DeviceAttributeReq, ModeReport, ModeState, SizeReportStyle, Visibility,
};
use mr_crabs_protocols::semantic_prompt::{Action, SemanticPrompt};
use mr_crabs_protocols::sgr::{SgrAttr, SgrState, UnderlineStyle};
use mr_crabs_protocols::shell::{SemanticAction, SemanticPromptState};
use mr_crabs_protocols::sink::{ClipboardEvent, NoopSink, ProtocolSink};
use mr_crabs_protocols::snapshot::{DecodeError, EncodeError, SnapshotPayload};
use vte::ansi::{
    Attr, CharsetIndex, ClearMode, Color, CursorShape, CursorStyle, Handler, Hyperlink,
    KeyboardModes, KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, NamedColor,
    PrivateMode, Processor, Rgb as VteRgb, StandardCharset, TabulationClearMode,
};

use crate::compact::CompactEngine;
use crate::{Cell, GridSize, NormalizedSnapshot, TerminalError, TerminalMode};

/// Scanner states for OSC/SOS/PM/APC/DCS string detection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScannerState {
    Ground,
    Escape,
    /// OSC string payload.
    Osc,
    /// SOS/PM/APC string payload.
    String,
    /// DCS string payload (after the intro's final byte).
    Dcs,
    /// ESC received inside an OSC string; the next byte decides ST vs abort.
    OscEscape,
    /// ESC received inside an SOS/PM/APC string.
    StringEscape,
    /// ESC received inside a DCS string.
    DcsEscape,
}

/// The byte-level scanner that separates OSC/SOS/PM/APC/DCS strings from the
/// VT stream. Returns [`ScanStep`] with the same `Pending`/`StreamPair`
/// semantics as `mr_crabs_protocols::apc::Scanner`; DCS intro bytes are
/// delivered as `Payload` (the adapter parses the intro into a [`DcsIntro`]).
pub struct Scanner {
    state: ScannerState,
    /// Terminator byte that ended the most recent OSC string (`0x07` for
    /// BEL, `0x1c` for ST), so replies echo the request's terminator.
    osc_terminator: Option<u8>,
    /// Remaining UTF-8 continuation bytes in the current scalar. C1-valued
    /// continuation bytes are payload/text, not 8-bit control introducers.
    utf8_remaining: u8,
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            state: ScannerState::Ground,
            osc_terminator: None,
            utf8_remaining: 0,
        }
    }

    pub fn state(&self) -> ScannerState {
        self.state
    }

    /// True when a printable-ASCII byte can bypass the scanner entirely:
    /// the scanner is in the ground state and no UTF-8 continuation is
    /// pending, so `next(byte)` for any byte in `0x20..0x7f` returns
    /// `ScanStep::Stream` without changing scanner state.
    pub fn can_fast_stream(&self) -> bool {
        self.state == ScannerState::Ground && self.utf8_remaining == 0
    }

    fn interrupt_utf8(&mut self) {
        self.utf8_remaining = 0;
    }

    /// The terminator byte of the most recent OSC string (`0x07` BEL /
    /// `0x1c` ST), consumed by [`TerminalProtocol::osc_end`].
    pub fn osc_terminator(&self) -> Option<u8> {
        self.osc_terminator
    }

    /// Scan one byte. In a string state, payload bytes are returned as
    /// `Payload` (the caller feeds them to the matching protocol handler);
    /// only the stream bytes, start, and end markers are surfaced.
    pub fn next(&mut self, byte: u8) -> ScanStep {
        // UTF-8 decoding exists only in the ground state. String protocols
        // are byte streams whose BEL/ST/CAN/C1 controls retain their raw
        // meaning even after an invalid lead byte.
        if self.state == ScannerState::Ground && self.utf8_remaining > 0 {
            if (0x80..=0xbf).contains(&byte) {
                self.utf8_remaining -= 1;
                return ScanStep::Stream(byte);
            }
            self.utf8_remaining = 0;
        }
        if self.state == ScannerState::Ground {
            self.utf8_remaining = match byte {
                0xc2..=0xdf => 1,
                0xe0..=0xef => 2,
                0xf0..=0xf4 => 3,
                _ => 0,
            };
            if self.utf8_remaining > 0 {
                return ScanStep::Stream(byte);
            }
        }
        match self.state {
            ScannerState::Ground => match byte {
                0x1b => {
                    self.state = ScannerState::Escape;
                    ScanStep::Pending
                }
                0x9d => {
                    // C1 OSC (8-bit)
                    self.state = ScannerState::Osc;
                    ScanStep::Started
                }
                0x90 => {
                    // C1 DCS (8-bit)
                    self.state = ScannerState::Dcs;
                    ScanStep::Started
                }
                0x98 | 0x9e | 0x9f => {
                    // C1 SOS / PM / APC
                    self.state = ScannerState::String;
                    ScanStep::Started
                }
                _ => ScanStep::Stream(byte),
            },
            ScannerState::Escape => match byte {
                b']' => {
                    self.state = ScannerState::Osc;
                    ScanStep::Started
                }
                b'P' => {
                    self.state = ScannerState::Dcs;
                    ScanStep::Started
                }
                b'X' | b'^' | b'_' => {
                    self.state = ScannerState::String;
                    ScanStep::Started
                }
                0x1b => {
                    // Release the previous ESC while retaining this ESC as
                    // the possible prefix of a subsequent string.
                    self.state = ScannerState::Escape;
                    ScanStep::Stream(0x1b)
                }
                _ => {
                    self.state = ScannerState::Ground;
                    ScanStep::StreamPair(0x1b, byte)
                }
            },
            ScannerState::Osc => match byte {
                0x07 => {
                    self.state = ScannerState::Ground;
                    self.osc_terminator = Some(0x07);
                    ScanStep::Ended
                }
                0x9c => {
                    // C1 ST
                    self.state = ScannerState::Ground;
                    self.osc_terminator = Some(0x1c);
                    ScanStep::Ended
                }
                0x1b => {
                    // Defer the decision until the following byte: ESC \
                    // terminates the string, any other ESC sequence aborts.
                    self.state = ScannerState::OscEscape;
                    ScanStep::Pending
                }
                0x18 | 0x1a => {
                    // CAN / SUB abort the string.
                    self.state = ScannerState::Ground;
                    ScanStep::Aborted
                }
                0x80..=0x9b | 0x9d..=0x9f => {
                    // Other C1 bytes abort the string.
                    self.state = ScannerState::Ground;
                    ScanStep::Aborted
                }
                _ => ScanStep::Payload,
            },
            ScannerState::String => match byte {
                0x1b => {
                    self.state = ScannerState::StringEscape;
                    ScanStep::Pending
                }
                0x9c => {
                    self.state = ScannerState::Ground;
                    ScanStep::Ended
                }
                0x18 | 0x1a => {
                    self.state = ScannerState::Ground;
                    ScanStep::Aborted
                }
                0x80..=0x9f => {
                    self.state = ScannerState::Ground;
                    ScanStep::Aborted
                }
                _ => ScanStep::Payload,
            },
            ScannerState::Dcs => match byte {
                0x1b => {
                    self.state = ScannerState::DcsEscape;
                    ScanStep::Pending
                }
                0x9c => {
                    self.state = ScannerState::Ground;
                    ScanStep::Ended
                }
                0x18 | 0x1a => {
                    self.state = ScannerState::Ground;
                    ScanStep::Aborted
                }
                0x80..=0x9f => {
                    self.state = ScannerState::Ground;
                    ScanStep::Aborted
                }
                _ => ScanStep::Payload,
            },
            ScannerState::OscEscape => {
                self.state = ScannerState::Ground;
                if byte == b'\\' {
                    self.osc_terminator = Some(0x1c);
                    ScanStep::Ended
                } else {
                    ScanStep::Aborted
                }
            }
            ScannerState::StringEscape => {
                self.state = ScannerState::Ground;
                if byte == b'\\' {
                    ScanStep::Ended
                } else {
                    ScanStep::Aborted
                }
            }
            ScannerState::DcsEscape => {
                self.state = ScannerState::Ground;
                if byte == b'\\' {
                    ScanStep::Ended
                } else {
                    ScanStep::Aborted
                }
            }
        }
    }
}

/// The owned protocol + compact-grid engine.
pub struct TerminalProtocol {
    pub(crate) engine: CompactEngine,
    osc: osc::Parser,
    dcs: dcs::Handler,
    apc: apc::Handler,
    scanner: Scanner,
    /// Persistent vte processor: escape-sequence state must survive across
    /// `advance` calls. Wrapped in `Option` so the processor can be moved
    /// out for the `Handler` borrow with a zero-allocation `take`/restore
    /// swap (`Processor::default` allocates the 2 MiB synchronized-update
    /// scratch buffer; a `mem::take` per byte would allocate and drop it
    /// on every single byte).
    processor: Option<Processor>,
    /// Batching buffer for ground-state stream bytes. Bytes are handed to
    /// vte in runs (one `advance` per run) instead of one `advance` per
    /// byte; ESC and C1 string introducers always flush the run first, so
    /// no escape sequence or mode change is ever delayed across the flush
    /// boundary and per-byte semantics are preserved.
    pending_run: [u8; 128],
    pending_run_len: usize,
    /// Chunk-boundary state for the vte escape parser: 0 = ground,
    /// 1 = ESC/intermediate, 2 = CSI. This only gates the whole-chunk
    /// plain-ASCII bypass; the persistent vte `Processor` remains authoritative.
    vte_prefix_state: u8,
    in_string: bool,
    string_kind: StringKind,
    /// DCS intro collection (the scanner delivers intro bytes as payload).
    dcs_intro: DcsIntroState,
    sgr: SgrState,
    semantic: SemanticPromptState,
    sink: Box<dyn ProtocolSink>,
    /// The current window title (truncated to 1024 bytes).
    title: Option<String>,
    /// The current working directory URL (OSC 7), truncated to 4096 bytes.
    pwd: Option<String>,
    /// Number of BELs received.
    bell_count: u64,
    /// Bounded reply scratch buffer (reused; capacity never exceeds the
    /// largest reply).
    reply: Vec<u8>,
    /// Scrolling region tracking for DECRQSS (CompactEngine stores the
    /// region privately; the adapter mirrors `set_scrolling_region`).
    scroll_region_start: usize,
    scroll_region_end: usize,
    /// DECBKM (private mode 67): Backspace sends BS instead of DEL.
    decbkm: bool,
    /// DEC 1035: ignore NumLock when selecting application keypad sequences.
    ignore_keypad_with_numlock: bool,
    /// DEC 1036: Meta/Alt prefixes the key with ESC.
    dec1036: bool,
    /// xterm modifyOtherKeys level 2 (`CSI > 4 ; 2 m`).
    modify_other_keys_2: bool,
    /// Prefix probe for XTWINOPS CSI 16 t, which vte 0.15 does not expose
    /// through its Handler trait. Kept across PTY chunks.
    csi16_probe_len: u8,
    /// OSC/CSI palette overlay used for color queries.
    palette: [Option<VteRgb>; 260],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StringKind {
    Osc,
    Apc,
    Dcs,
}

/// DCS intro parsing: `ESC P [params] [intermediates] final`.
///
/// Mirrors vte's DCS entry state (up to 16 u16 params, up to 2
/// intermediates) so the intro can be replayed into
/// [`dcs::Handler::hook`] as a [`DcsIntro`].
struct DcsIntroState {
    phase: DcsIntroPhase,
    params: [u16; 16],
    params_len: usize,
    /// Current parameter being accumulated (digits).
    value: u16,
    intermediates: [u8; 2],
    intermediates_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DcsIntroPhase {
    /// Collecting digits / separators before the final byte.
    Params,
    /// Collecting intermediates (0x20-0x2F).
    Intermediates,
    /// Final byte consumed; payload bytes flow to `dcs::Handler::put`.
    Payload,
}

impl DcsIntroState {
    fn new() -> Self {
        Self {
            phase: DcsIntroPhase::Params,
            params: [0; 16],
            params_len: 0,
            value: 0,
            intermediates: [0; 2],
            intermediates_len: 0,
        }
    }

    /// Feed one DCS byte; returns the final-byte hook once the intro
    /// completes.
    fn feed(&mut self, byte: u8) -> DcsIntroResult {
        match self.phase {
            DcsIntroPhase::Payload => DcsIntroResult::Payload,
            DcsIntroPhase::Params => match byte {
                b'0'..=b'9' => {
                    self.value = self
                        .value
                        .saturating_mul(10)
                        .saturating_add(u16::from(byte - b'0'));
                    DcsIntroResult::None
                }
                b';' | b':' => {
                    self.finish_param();
                    DcsIntroResult::None
                }
                0x20..=0x2f => {
                    self.finish_param();
                    self.phase = DcsIntroPhase::Intermediates;
                    self.push_intermediate(byte);
                    DcsIntroResult::None
                }
                0x40..=0x7e => {
                    self.finish_param();
                    self.phase = DcsIntroPhase::Payload;
                    DcsIntroResult::Hook(byte)
                }
                _ => {
                    // Control byte inside the intro: ignore (vte DCS entry
                    // `anywhere`).
                    DcsIntroResult::None
                }
            },
            DcsIntroPhase::Intermediates => match byte {
                0x20..=0x2f => {
                    self.push_intermediate(byte);
                    DcsIntroResult::None
                }
                0x40..=0x7e => {
                    self.phase = DcsIntroPhase::Payload;
                    DcsIntroResult::Hook(byte)
                }
                _ => {
                    // Back to parameter collection.
                    self.phase = DcsIntroPhase::Params;
                    self.feed(byte)
                }
            },
        }
    }

    fn finish_param(&mut self) {
        if self.params_len < self.params.len() {
            self.params[self.params_len] = self.value;
            self.params_len += 1;
        }
        self.value = 0;
    }

    fn push_intermediate(&mut self, byte: u8) {
        if self.intermediates_len < self.intermediates.len() {
            self.intermediates[self.intermediates_len] = byte;
            self.intermediates_len += 1;
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DcsIntroResult {
    None,
    /// The intro completed at this final byte.
    Hook(u8),
    /// The intro completed earlier; this byte is DCS payload.
    Payload,
}

impl TerminalProtocol {
    pub fn new(size: &GridSize) -> Result<Self, TerminalError> {
        Ok(Self {
            engine: CompactEngine::new(*size)?,
            osc: osc::Parser::new(),
            dcs: dcs::Handler::new(),
            apc: apc::Handler::new(),
            scanner: Scanner::new(),
            processor: Some(Processor::new()),
            pending_run: [0u8; 128],
            pending_run_len: 0,
            vte_prefix_state: 0,
            in_string: false,
            string_kind: StringKind::Osc,
            dcs_intro: DcsIntroState::new(),
            sgr: SgrState::new(),
            semantic: SemanticPromptState::new(),
            sink: Box::new(NoopSink),
            title: None,
            pwd: None,
            bell_count: 0,
            reply: Vec::new(),
            scroll_region_start: 0,
            scroll_region_end: usize::from(size.rows),
            decbkm: false,
            ignore_keypad_with_numlock: true,
            dec1036: false,
            modify_other_keys_2: false,
            csi16_probe_len: 0,
            palette: [None; 260],
        })
    }

    pub fn engine(&self) -> &CompactEngine {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut CompactEngine {
        &mut self.engine
    }

    /// Install a protocol sink (the app layer). Reports that need runtime
    /// values (size, device attributes, palette) use it.
    pub fn set_sink(&mut self, sink: Box<dyn ProtocolSink>) {
        self.sink = sink;
    }

    pub fn pwd(&self) -> Option<&str> {
        self.pwd.as_deref()
    }

    pub fn bell_count(&self) -> u64 {
        self.bell_count
    }

    pub fn semantic_state(&self) -> &SemanticPromptState {
        &self.semantic
    }

    pub fn backarrow_key_mode(&self) -> bool {
        self.decbkm
    }

    pub fn ignore_keypad_with_numlock(&self) -> bool {
        self.ignore_keypad_with_numlock
    }

    pub fn modify_other_keys_2(&self) -> bool {
        self.modify_other_keys_2
    }

    pub fn alt_esc_prefix(&self) -> bool {
        self.dec1036
    }

    fn track_private_mode(&mut self, mode: PrivateMode, enabled: bool) {
        match mode.raw() {
            67 => self.decbkm = enabled,
            1035 => self.ignore_keypad_with_numlock = enabled,
            1036 => self.dec1036 = enabled,
            _ => {}
        }
    }

    fn track_modify_other_keys(&mut self, mode: ModifyOtherKeys) {
        self.modify_other_keys_2 = matches!(mode, ModifyOtherKeys::EnableAll);
    }

    fn reset_overlay(&mut self) {
        self.decbkm = false;
        self.ignore_keypad_with_numlock = true;
        self.dec1036 = false;
        self.modify_other_keys_2 = false;
    }

    /// Adapter-side state reset after [`CompactEngine::resize`] (the engine
    /// resets the scrolling region to the full screen).
    pub(crate) fn note_resize(&mut self, rows: usize) {
        self.scroll_region_start = 0;
        self.scroll_region_end = rows;
    }

    /// Feed raw PTY bytes through the scanner, protocol parsers, and vte.
    ///
    /// vte 0.15 does not expose XTWINOPS CSI 16 t through `Handler`, so that
    /// one five-byte request is recognized here before ordinary vte dispatch.
    /// The prefix length survives arbitrary PTY chunk boundaries.
    pub fn feed(&mut self, bytes: &[u8]) {
        const CSI16: &[u8; 5] = b"\x1b[16t";
        const VTE_GROUND: u8 = 0;
        if self.csi16_probe_len == 0
            && self.vte_prefix_state == VTE_GROUND
            && !self.in_string
            && self.pending_run_len == 0
            && self.scanner.can_fast_stream()
            && self.try_feed_paired_sgr_text(bytes)
        {
            return;
        }
        if self.csi16_probe_len == 0
            && self.vte_prefix_state == VTE_GROUND
            && !self.in_string
            && self.pending_run_len == 0
            && self.scanner.can_fast_stream()
            && self.scanner.utf8_remaining == 0
            && bytes
                .iter()
                .all(|byte| matches!(*byte, b'\n' | b'\r') || (0x20..0x7f).contains(byte))
        {
            self.engine.input_plain_ascii(bytes);
            return;
        }
        self.track_vte_prefix(bytes);
        let mut cursor = 0;
        while cursor < bytes.len() {
            if self.csi16_probe_len != 0 {
                let expected = CSI16[usize::from(self.csi16_probe_len)];
                let byte = bytes[cursor];
                if byte == expected {
                    self.csi16_probe_len += 1;
                    cursor += 1;
                    if usize::from(self.csi16_probe_len) == CSI16.len() {
                        self.csi16_probe_len = 0;
                        self.report_size(SizeReportStyle::Csi16T);
                    }
                    continue;
                }

                let prefix_len = usize::from(self.csi16_probe_len);
                self.csi16_probe_len = 0;
                self.feed_scanned(&CSI16[..prefix_len]);
                if byte != 0x1b {
                    self.feed_scanned(&bytes[cursor..cursor + 1]);
                    cursor += 1;
                }
                continue;
            }

            if self.in_string {
                self.feed_scanned(&bytes[cursor..cursor + 1]);
                cursor += 1;
                continue;
            }

            if let Some(relative) = bytes[cursor..]
                .windows(CSI16.len())
                .position(|window| window == CSI16)
            {
                let candidate = cursor + relative;
                if candidate > cursor {
                    self.feed_scanned(&bytes[cursor..candidate]);
                    cursor = candidate;
                    if self.in_string {
                        continue;
                    }
                }
                self.scanner.interrupt_utf8();
                self.report_size(SizeReportStyle::Csi16T);
                cursor += CSI16.len();
                continue;
            }

            let remaining = &bytes[cursor..];
            let suffix_len = (1..CSI16.len())
                .rev()
                .find(|&len| remaining.ends_with(&CSI16[..len]))
                .unwrap_or(0);
            let bulk_end = bytes.len() - suffix_len;
            if bulk_end > cursor {
                self.feed_scanned(&bytes[cursor..bulk_end]);
                cursor = bulk_end;
                if self.in_string {
                    continue;
                }
            }
            if suffix_len != 0 {
                self.scanner.interrupt_utf8();
                self.csi16_probe_len = suffix_len as u8;
                cursor += suffix_len;
            }
        }
    }

    /// Fast path for text streams whose only escape sequence is the adjacent
    /// `SGR red` + `SGR reset` pair. The pair has no intervening printable
    /// output and therefore ends in exactly the default pen state.
    fn try_feed_paired_sgr_text(&mut self, bytes: &[u8]) -> bool {
        const PAIR: &[u8; 9] = b"\x1b[31m\x1b[0m";
        let mut utf8_remaining = self.scanner.utf8_remaining;
        let mut cursor = 0usize;
        let mut saw_pair = false;
        while cursor < bytes.len() {
            if utf8_remaining == 0 && bytes[cursor..].starts_with(PAIR) {
                saw_pair = true;
                cursor += PAIR.len();
                continue;
            }
            let byte = bytes[cursor];
            if utf8_remaining != 0 {
                if !(0x80..=0xbf).contains(&byte) {
                    return false;
                }
                utf8_remaining -= 1;
            } else {
                utf8_remaining = match byte {
                    0x20..=0x7e => 0,
                    0xc2..=0xdf => 1,
                    0xe0..=0xef => 2,
                    0xf0..=0xf4 => 3,
                    _ => return false,
                };
            }
            cursor += 1;
        }
        if !saw_pair {
            return false;
        }

        let mut start = 0usize;
        while let Some(relative) = bytes[start..]
            .windows(PAIR.len())
            .position(|window| window == PAIR)
        {
            let pair = start + relative;
            self.feed_text_segment(&bytes[start..pair]);
            self.engine.reset_sgr_attributes();
            self.sgr = SgrState::new();
            start = pair + PAIR.len();
        }
        self.feed_text_segment(&bytes[start..]);
        self.scanner.utf8_remaining = utf8_remaining;
        true
    }

    fn feed_text_segment(&mut self, bytes: &[u8]) {
        if bytes.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
            self.engine.input_plain_ascii(bytes);
        } else if let Ok(text) = std::str::from_utf8(bytes) {
            self.engine.input_text_run(text);
        } else {
            self.advance_vte(bytes);
        }
    }

    fn track_vte_prefix(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.vte_prefix_state = match (self.vte_prefix_state, byte) {
                (_, 0x18 | 0x1a) => 0,
                (_, 0x1b) => 1,
                (0, 0x9b) | (1, b'[') => 2,
                (1, 0x20..=0x2f) => 1,
                (1, 0x30..=0x7e) => 0,
                (2, 0x40..=0x7e) => 0,
                (state, _) => state,
            };
        }
    }

    /// Feed a scanner-independent chunk directly into vte when it cannot
    /// introduce OSC/APC/DCS/SOS/PM. The lightweight pass only maintains the
    /// UTF-8 continuation count needed to distinguish C1-valued continuation
    /// bytes from raw string introducers.
    fn try_feed_direct(&mut self, bytes: &[u8]) -> bool {
        if self.in_string || self.pending_run_len != 0 || self.scanner.state != ScannerState::Ground
        {
            return false;
        }

        let mut utf8_remaining = self.scanner.utf8_remaining;
        let mut cursor = 0;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            if utf8_remaining > 0 {
                if (0x80..=0xbf).contains(&byte) {
                    utf8_remaining -= 1;
                    cursor += 1;
                    continue;
                }
                utf8_remaining = 0;
                continue;
            }
            match byte {
                0xc2..=0xdf => utf8_remaining = 1,
                0xe0..=0xef => utf8_remaining = 2,
                0xf0..=0xf4 => utf8_remaining = 3,
                0x90 | 0x98 | 0x9d..=0x9f => return false,
                0x1b => {
                    let Some(&next) = bytes.get(cursor + 1) else {
                        return false;
                    };
                    if matches!(next, b']' | b'P' | b'X' | b'^' | b'_') {
                        return false;
                    }
                }
                _ => {}
            }
            cursor += 1;
        }

        self.scanner.utf8_remaining = utf8_remaining;
        self.advance_vte(bytes);
        true
    }

    fn feed_scanned(&mut self, bytes: &[u8]) {
        if self.try_feed_direct(bytes) {
            return;
        }
        // Re-enter the printable-ASCII fast path after every escape/control.
        // The previous prefix-only path sent the remainder of a feed through
        // the scanner after its first ESC, which penalized SGR-heavy output.
        let mut cursor = 0;
        while cursor < bytes.len() {
            if !self.in_string && self.scanner.can_fast_stream() {
                let start = cursor;
                while cursor < bytes.len() && (0x20..0x7f).contains(&bytes[cursor]) {
                    cursor += 1;
                }
                if cursor > start {
                    self.flush_pending_run();
                    self.advance_vte(&bytes[start..cursor]);
                    continue;
                }
            }

            let byte = bytes[cursor];
            cursor += 1;
            if self.in_string {
                match self.scanner.next(byte) {
                    ScanStep::Payload => match self.string_kind {
                        StringKind::Osc => self.osc.next(byte),
                        StringKind::Apc => self.apc.feed(byte),
                        StringKind::Dcs => self.dcs_byte(byte),
                    },
                    ScanStep::Ended => {
                        self.in_string = false;
                        match self.string_kind {
                            StringKind::Osc => self.osc_end(),
                            StringKind::Apc => self.apc_end(),
                            StringKind::Dcs => self.dcs_end(),
                        }
                    }
                    ScanStep::Aborted => {
                        self.in_string = false;
                        match self.string_kind {
                            StringKind::Osc => {
                                if let Some(command) = self.osc.end(Some(0x18)) {
                                    self.dispatch_osc(command);
                                }
                                self.osc.reset();
                            }
                            StringKind::Apc => self.apc_end(),
                            StringKind::Dcs => self.dcs_end(),
                        }
                    }
                    ScanStep::Pending => {}
                    ScanStep::Started | ScanStep::Stream(_) | ScanStep::StreamPair(_, _) => {
                        unreachable!("a string byte cannot start a string")
                    }
                }
                continue;
            }
            match self.scanner.next(byte) {
                ScanStep::Stream(stream_byte) => {
                    // Batch consecutive ground-state stream bytes into one
                    // vte `advance`. ESC and C1 string introducers always
                    // flush first, so escape sequences and mode changes are
                    // processed immediately and never delayed by the buffer.
                    self.pending_run[self.pending_run_len] = stream_byte;
                    self.pending_run_len += 1;
                    if self.pending_run_len == self.pending_run.len() {
                        self.flush_pending_run();
                    }
                }
                ScanStep::StreamPair(first, second) => {
                    self.flush_pending_run();
                    self.advance_vte(&[first, second]);
                }
                ScanStep::Pending => {
                    // ESC withheld by the scanner: process everything before
                    // it first so escape-sequence state stays in order.
                    self.flush_pending_run();
                }
                ScanStep::Started => {
                    self.flush_pending_run();
                    self.in_string = true;
                    // vte never saw the introducer (the scanner withheld
                    // it); feed a synthetic complete sequence so its parser
                    // returns to ground defensively.
                    let (synthetic, kind) = match self.scanner.state() {
                        ScannerState::Osc => (b"\x1b]999;\x1b\\".as_slice(), StringKind::Osc),
                        ScannerState::String => (b"\x1bX\x1b\\".as_slice(), StringKind::Apc),
                        ScannerState::Dcs => (b"\x1bP\x1b\\".as_slice(), StringKind::Dcs),
                        _ => unreachable!("Started only from string introducers"),
                    };
                    self.advance_vte(synthetic);
                    self.string_kind = kind;
                    match kind {
                        StringKind::Osc => self.osc.reset(),
                        StringKind::Apc => self.apc.start(),
                        StringKind::Dcs => self.dcs_intro.reset(),
                    }
                }
                ScanStep::Ended | ScanStep::Aborted | ScanStep::Payload => {
                    unreachable!("ground-state scan cannot end a string")
                }
            }
        }
        // Flush at the end of every feed call so callers observing terminal
        // state (damage, snapshot, modes) never see stale buffered bytes.
        self.flush_pending_run();
    }

    /// Hand one slice of stream bytes to the vte processor. The processor is
    /// moved out of the struct so `self` can be borrowed as the `Handler`;
    /// the `Option` swap is a plain move (no allocation), unlike a
    /// `mem::take` which would construct a fresh default processor — and
    /// with it a fresh 2 MiB synchronized-update scratch buffer — on every
    /// call.
    fn advance_vte(&mut self, bytes: &[u8]) {
        let mut processor = self.processor.take().expect("vte processor present");
        processor.advance(self, bytes);
        self.processor = Some(processor);
    }

    /// Flush buffered ground-state stream bytes through vte.
    fn flush_pending_run(&mut self) {
        let len = self.pending_run_len;
        if len == 0 {
            return;
        }
        // Copy the run to a stack buffer so `advance_vte` can borrow `self`
        // mutably (the processor is moved out during the call).
        let mut run = [0u8; 128];
        run[..len].copy_from_slice(&self.pending_run[..len]);
        self.pending_run_len = 0;
        self.advance_vte(&run[..len]);
    }

    fn osc_end(&mut self) {
        let terminator = self.scanner.osc_terminator();
        let command = self.osc.end(terminator);
        if let Some(command) = command {
            self.dispatch_osc(command);
        }
        self.osc.reset();
    }

    fn apc_end(&mut self) {
        let command = self.apc.end();
        if let Some(command) = command {
            self.sink.apc(&command);
        }
    }

    fn dcs_end(&mut self) {
        if let Some(command) = self.dcs.unhook() {
            self.dispatch_dcs(command);
        }
    }

    fn dcs_byte(&mut self, byte: u8) {
        match self.dcs_intro.feed(byte) {
            DcsIntroResult::None => {}
            DcsIntroResult::Hook(final_byte) => {
                // Copy the intro to the stack so `self.dcs` can be borrowed
                // mutably (no allocation on the parse path).
                let mut params = [0u16; 16];
                let params_len = self.dcs_intro.params_len;
                params[..params_len].copy_from_slice(&self.dcs_intro.params[..params_len]);
                let mut intermediates = [0u8; 2];
                let intermediates_len = self.dcs_intro.intermediates_len;
                intermediates[..intermediates_len]
                    .copy_from_slice(&self.dcs_intro.intermediates[..intermediates_len]);
                let intro = DcsIntro {
                    params: &params[..params_len],
                    intermediates: &intermediates[..intermediates_len],
                    final_byte,
                };
                if let Some(command) = self.dcs.hook(intro) {
                    self.dispatch_dcs(command);
                }
            }
            DcsIntroResult::Payload => {
                if let Some(command) = self.dcs.put(byte) {
                    self.dispatch_dcs(command);
                }
            }
        }
    }

    fn dispatch_osc(&mut self, command: Command) {
        match command {
            Command::ChangeWindowTitle(title) => {
                let mut title = title;
                title.truncate(mr_crabs_protocols::limits::MAX_TITLE_LEN);
                self.title = Some(title.clone());
                self.engine.set_window_title(Some(title.clone()));
                self.sink.title_changed(&title);
            }
            Command::ChangeWindowIcon(_) => {}
            Command::ReportPwd(url) => {
                let mut url = url;
                url.truncate(mr_crabs_protocols::limits::MAX_PWD_URL_LEN);
                self.pwd = Some(url.clone());
                self.sink.pwd_changed(&url);
            }
            Command::HyperlinkStart { id, uri } => {
                self.sink.hyperlink(id.as_deref(), &uri);
                self.engine.set_hyperlink(Some(Hyperlink { id, uri }));
            }
            Command::HyperlinkEnd => {
                self.engine.set_hyperlink(None);
                self.sink.hyperlink(None, "");
            }
            Command::SemanticPrompt(cmd) => {
                let actions = self.semantic.apply(&cmd);
                self.apply_semantic_actions(&actions);
                self.sink.semantic_prompt(&cmd);
            }
            Command::MarkPromptStart => {
                let actions = self.semantic.apply(&SemanticPrompt {
                    action: Action::FreshLineNewPrompt,
                    options_unvalidated: String::new(),
                });
                self.apply_semantic_actions(&actions);
            }
            Command::ShowDesktopNotification { title, body } => {
                self.sink.notification(&title, &body);
            }
            Command::ClipboardContents { kind, data } => {
                self.sink.clipboard(&ClipboardEvent { kind, data });
            }
            Command::ColorOperation {
                op,
                requests,
                terminator,
            } => {
                self.apply_color_operation(op, &requests, terminator);
            }
            Command::KittyColor { requests } => {
                self.sink.kitty_color(&requests);
            }
            Command::MouseShape(shape) => {
                self.sink.mouse_shape(&shape);
            }
            Command::ConemuSleep { .. } => {}
            Command::ConemuShowMessageBox(_)
            | Command::ConemuChangeTabTitle(_)
            | Command::ConemuGuimacro(_)
            | Command::ConemuRunProcess(_)
            | Command::ConemuOutputEnvironmentVariable(_)
            | Command::ConemuComment(_)
            | Command::ConemuWaitInput
            | Command::ConemuXtermEmulation { .. } => {}
            Command::ConemuProgressReport { state, progress } => {
                self.sink.progress(state, progress);
            }
            Command::KittyTextSizing(_)
            | Command::KittyDnd(_)
            | Command::KittyClipboard(_)
            | Command::Iterm2(_)
            | Command::ContextSignal(_) => {}
        }
    }

    fn apply_semantic_actions(&mut self, actions: &[SemanticAction]) {
        for action in actions {
            match action {
                SemanticAction::MarkPrompt
                | SemanticAction::MarkInput
                | SemanticAction::MarkOutput => {
                    // Row marking lives in the S8 side tables; the protocol
                    // layer tracks the state machine (`self.semantic`),
                    // which the app observes through `semantic_state`.
                }
                SemanticAction::FreshLine => {
                    // OSC 133;L: carriage return + index unless already at
                    // the left margin (Ghostty semanticPromptFreshLine).
                    if !self.cursor_at_left_margin() {
                        self.engine.carriage_return();
                        self.engine.linefeed();
                    }
                }
                SemanticAction::ClearInputEol => {
                    self.engine.clear_line(LineClearMode::Right);
                }
                SemanticAction::None => {}
            }
        }
    }

    fn cursor_at_left_margin(&self) -> bool {
        self.engine.cursor().col == 0
    }

    fn apply_color_operation(
        &mut self,
        op: mr_crabs_protocols::color::ColorOperation,
        requests: &[mr_crabs_protocols::color::ColorRequest],
        terminator: mr_crabs_protocols::Terminator,
    ) {
        use mr_crabs_protocols::color::ColorRequest as R;
        for request in requests {
            match request {
                R::Set { target, color } => match target {
                    ColorTarget::Palette(i) => {
                        self.set_engine_color(
                            *i as usize,
                            VteRgb {
                                r: color.r,
                                g: color.g,
                                b: color.b,
                            },
                        );
                    }
                    ColorTarget::Dynamic(dynamic) => {
                        if let Some(index) = dynamic_color_index(*dynamic) {
                            self.set_engine_color(
                                index,
                                VteRgb {
                                    r: color.r,
                                    g: color.g,
                                    b: color.b,
                                },
                            );
                        }
                    }
                    ColorTarget::Special(_) => {}
                },
                R::Reset(target) => match target {
                    ColorTarget::Palette(i) => self.reset_engine_color(*i as usize),
                    ColorTarget::Dynamic(dynamic) => {
                        if let Some(index) = dynamic_color_index(*dynamic) {
                            self.reset_engine_color(index);
                        }
                    }
                    ColorTarget::Special(_) => {}
                },
                R::ResetPalette => {
                    for i in 0..256 {
                        self.reset_engine_color(i);
                    }
                }
                R::ResetSpecial => {
                    self.reset_engine_color(NamedColor::Foreground as usize);
                    self.reset_engine_color(NamedColor::Background as usize);
                    self.reset_engine_color(NamedColor::Cursor as usize);
                }
                R::Query(target) => {
                    let color = match target {
                        // CompactEngine palette overlay (OSC 4 / CSI color).
                        ColorTarget::Palette(i) => self
                            .palette
                            .get(*i as usize)
                            .copied()
                            .flatten()
                            .map(|c| Rgb {
                                r: c.r,
                                g: c.g,
                                b: c.b,
                            }),
                        _ => self.sink.color_for(*target),
                    };
                    if let Some(color) = color {
                        self.reply.clear();
                        let mut report = String::new();
                        mr_crabs_protocols::color::write_xterm_color_report(
                            *target,
                            color,
                            terminator,
                            &mut report,
                        );
                        self.reply.extend_from_slice(report.as_bytes());
                        self.flush_reply();
                    }
                }
            }
        }
        let _ = op;
    }

    fn write_pty(&mut self, bytes: &[u8]) {
        self.sink.write_pty(bytes);
    }

    /// Write the scratch reply buffer back to the PTY (taken out of `self`
    /// so the sink borrow and the buffer do not collide).
    fn flush_reply(&mut self) {
        let reply = std::mem::take(&mut self.reply);
        self.write_pty(&reply);
        self.reply = reply;
    }

    // ------------------------------------------------------------------
    // Reports
    // ------------------------------------------------------------------

    fn report_device_attributes(&mut self, req: DeviceAttributeReq) {
        let attrs = self.sink.device_attributes();
        self.reply.clear();
        attrs.encode(req, &mut self.reply);
        self.flush_reply();
    }

    fn report_device_status(&mut self, req: reports::DeviceStatusReq) {
        match req {
            reports::DeviceStatusReq::OperatingStatus => {
                self.write_pty(b"\x1b[0n");
            }
            reports::DeviceStatusReq::CursorPosition => {
                let cursor = self.engine.cursor();
                let x = usize::from(cursor.col);
                let y = usize::from(cursor.row);
                self.reply.clear();
                let _ = write!(self.reply, "\x1b[{};{}R", y + 1, x + 1);
                self.flush_reply();
            }
            reports::DeviceStatusReq::ColorScheme => {
                // The engine has no theme provider yet; report the dark
                // scheme (the sink has no color-scheme hook to ask).
                self.reply.clear();
                ColorScheme::Dark.encode(&mut self.reply);
                self.flush_reply();
            }
            reports::DeviceStatusReq::Visibility => {
                self.reply.clear();
                Visibility::PotentiallyVisible.encode(&mut self.reply);
                self.flush_reply();
            }
        }
    }

    fn report_mode(&mut self, mode: u16, ansi: bool) {
        let state = self.mode_state(mode, ansi);
        self.reply.clear();
        ModeReport { mode, ansi, state }.encode(&mut self.reply);
        self.flush_reply();
    }

    fn mode_state(&self, mode: u16, ansi: bool) -> ModeState {
        let set = |flag: TerminalMode| -> ModeState {
            if self.engine.has_mode(flag) {
                ModeState::Set
            } else {
                ModeState::Reset
            }
        };
        if ansi {
            return match mode {
                4 => set(TerminalMode::Insert),
                20 => set(TerminalMode::LineFeedNewLine),
                _ => ModeState::NotRecognized,
            };
        }
        match mode {
            1 => set(TerminalMode::AppCursor),
            6 => set(TerminalMode::Origin),
            7 => set(TerminalMode::LineWrap),
            25 => set(TerminalMode::ShowCursor),
            1000 => set(TerminalMode::MouseReportClick),
            1002 => set(TerminalMode::MouseMotion),
            1003 => set(TerminalMode::MouseDrag),
            1004 => set(TerminalMode::FocusInOut),
            1005 => set(TerminalMode::Utf8Mouse),
            1006 => set(TerminalMode::SgrMouse),
            1007 => set(TerminalMode::AlternateScroll),
            1042 => set(TerminalMode::UrgencyHints),
            1049 => set(TerminalMode::AltScreen),
            2004 => set(TerminalMode::BracketedPaste),
            12 => {
                if self.engine.cursor_style().blinking {
                    ModeState::Set
                } else {
                    ModeState::Reset
                }
            }
            _ => ModeState::NotRecognized,
        }
    }

    fn report_size(&mut self, style: SizeReportStyle) {
        match style {
            SizeReportStyle::Csi21T => {
                if let Some(title) = self.title.clone() {
                    self.reply.clear();
                    let _ = write!(self.reply, "\x1b]l{title}\x1b\\");
                    self.flush_reply();
                }
            }
            _ => {
                let Some(size) = self.sink.text_area_size() else {
                    return;
                };
                self.reply.clear();
                reports::encode_size_report(&mut self.reply, style, size);
                self.flush_reply();
            }
        }
    }

    fn report_keyboard_mode(&mut self) {
        let mut flags = 0u8;
        if self.engine.has_mode(TerminalMode::DisambiguateEscCodes) {
            flags |= 0b0000_0001;
        }
        if self.engine.has_mode(TerminalMode::ReportEventTypes) {
            flags |= 0b0000_0010;
        }
        if self.engine.has_mode(TerminalMode::ReportAlternateKeys) {
            flags |= 0b0000_0100;
        }
        if self.engine.has_mode(TerminalMode::ReportAllKeysAsEsc) {
            flags |= 0b0000_1000;
        }
        if self.engine.has_mode(TerminalMode::ReportAssociatedText) {
            flags |= 0b0001_0000;
        }
        self.reply.clear();
        reports::encode_kitty_keyboard_flags(flags, &mut self.reply);
        self.flush_reply();
    }

    fn set_engine_color(&mut self, index: usize, color: VteRgb) {
        self.engine.set_color(index, color);
        if let Some(slot) = self.palette.get_mut(index) {
            *slot = Some(color);
        }
    }

    fn reset_engine_color(&mut self, index: usize) {
        self.engine.reset_color(index);
        if let Some(slot) = self.palette.get_mut(index) {
            *slot = None;
        }
    }
}

impl Default for TerminalProtocol {
    fn default() -> Self {
        unreachable!("TerminalProtocol requires a configured grid size")
    }
}

// -------------------------------------------------------------------------
// vte Handler implementation: forward grid operations to CompactEngine,
// intercept the protocol entry points. OSC/DCS/APC strings never reach this
// trait: the byte-level scanner separates them before vte (vte 0.15's ansi
// `Processor` delivers `hook`/`put`/`unhook`/`osc_dispatch` through an
// internal Performer that no-ops them).
// -------------------------------------------------------------------------

impl Handler for TerminalProtocol {
    fn input(&mut self, c: char) {
        self.engine.input(c);
    }

    #[inline]
    fn input_run(&mut self, text: &str) {
        self.engine.input_run(text);
    }

    fn decaln(&mut self) {
        self.engine.decaln();
    }

    fn goto(&mut self, line: i32, col: usize) {
        self.engine.goto(line, col);
    }

    fn goto_line(&mut self, line: i32) {
        self.engine.goto_line(line);
    }

    fn goto_col(&mut self, col: usize) {
        self.engine.goto_col(col);
    }

    fn insert_blank(&mut self, count: usize) {
        self.engine.insert_blank(count);
    }

    fn move_up(&mut self, lines: usize) {
        self.engine.move_up(lines);
    }

    fn move_down(&mut self, lines: usize) {
        self.engine.move_down(lines);
    }

    fn move_forward(&mut self, cols: usize) {
        self.engine.move_forward(cols);
    }

    fn move_backward(&mut self, cols: usize) {
        self.engine.move_backward(cols);
    }

    fn move_down_and_cr(&mut self, lines: usize) {
        self.engine.move_down_and_cr(lines);
    }

    fn move_up_and_cr(&mut self, lines: usize) {
        self.engine.move_up_and_cr(lines);
    }

    fn put_tab(&mut self, count: u16) {
        self.engine.put_tab(count);
    }

    fn backspace(&mut self) {
        self.engine.backspace();
    }

    fn carriage_return(&mut self) {
        self.engine.carriage_return();
    }

    fn linefeed(&mut self) {
        self.engine.linefeed();
    }

    fn substitute(&mut self) {
        self.engine.substitute();
    }

    fn newline(&mut self) {
        self.engine.newline();
    }

    fn set_horizontal_tabstop(&mut self) {
        self.engine.set_horizontal_tabstop();
    }

    fn scroll_up(&mut self, lines: usize) {
        self.engine.scroll_up(lines);
    }

    fn scroll_down(&mut self, lines: usize) {
        self.engine.scroll_down(lines);
    }

    fn insert_blank_lines(&mut self, lines: usize) {
        self.engine.insert_blank_lines(lines);
    }

    fn delete_lines(&mut self, lines: usize) {
        self.engine.delete_lines(lines);
    }

    fn erase_chars(&mut self, count: usize) {
        self.engine.erase_chars(count);
    }

    fn delete_chars(&mut self, count: usize) {
        self.engine.delete_chars(count);
    }

    fn move_backward_tabs(&mut self, count: u16) {
        self.engine.move_backward_tabs(count);
    }

    fn move_forward_tabs(&mut self, count: u16) {
        self.engine.move_forward_tabs(count);
    }

    fn save_cursor_position(&mut self) {
        self.engine.save_cursor_position();
    }

    fn restore_cursor_position(&mut self) {
        self.engine.restore_cursor_position();
    }

    fn clear_line(&mut self, mode: LineClearMode) {
        self.engine.clear_line(mode);
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        self.engine.clear_screen(mode);
    }

    fn clear_tabs(&mut self, mode: TabulationClearMode) {
        self.engine.clear_tabs(mode);
    }

    fn reset_state(&mut self) {
        self.sgr.reset();
        self.reset_overlay();
        self.palette = [None; 260];
        self.engine.reset_state();
    }

    fn reverse_index(&mut self) {
        self.engine.reverse_index();
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        self.apply_attr(&attr);
        self.engine.terminal_attribute(attr);
    }

    fn set_mode(&mut self, mode: Mode) {
        self.engine.set_mode(mode);
    }

    fn unset_mode(&mut self, mode: Mode) {
        self.engine.unset_mode(mode);
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        self.track_private_mode(mode, true);
        self.engine.set_private_mode(mode);
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        self.track_private_mode(mode, false);
        self.engine.unset_private_mode(mode);
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let screen_lines = usize::from(self.engine.size().rows);
        let bottom = bottom.unwrap_or(screen_lines);
        if top >= bottom {
            return;
        }
        self.scroll_region_start = top.saturating_sub(1).min(screen_lines);
        self.scroll_region_end = bottom.min(screen_lines);
        self.engine.set_scrolling_region(top, Some(bottom));
    }

    fn set_keypad_application_mode(&mut self) {
        self.engine.set_keypad_application_mode();
    }

    fn unset_keypad_application_mode(&mut self) {
        self.engine.unset_keypad_application_mode();
    }

    fn set_active_charset(&mut self, index: CharsetIndex) {
        self.engine.set_active_charset(index);
    }

    fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset) {
        self.engine.configure_charset(index, charset);
    }

    fn set_cursor_style(&mut self, style: Option<CursorStyle>) {
        self.engine.set_cursor_style(style);
    }

    fn set_cursor_shape(&mut self, shape: CursorShape) {
        self.engine.set_cursor_shape(shape);
    }

    fn set_title(&mut self, title: Option<String>) {
        if let Some(title) = title {
            let mut title = title;
            title.truncate(mr_crabs_protocols::limits::MAX_TITLE_LEN);
            self.title = Some(title.clone());
            self.engine.set_window_title(Some(title));
        } else {
            self.title = None;
            self.engine.set_window_title(None);
        }
    }

    fn push_title(&mut self) {
        self.engine.push_title();
    }

    fn pop_title(&mut self) {
        self.engine.pop_title();
        self.title = self.engine.title().map(str::to_owned);
    }

    fn set_keyboard_mode(&mut self, mode: KeyboardModes, behavior: KeyboardModesApplyBehavior) {
        self.engine.set_keyboard_mode(mode, behavior);
    }

    fn push_keyboard_mode(&mut self, mode: KeyboardModes) {
        self.engine.push_keyboard_mode(mode);
    }

    fn pop_keyboard_modes(&mut self, to_pop: u16) {
        self.engine.pop_keyboard_modes(to_pop);
    }

    fn set_modify_other_keys(&mut self, mode: ModifyOtherKeys) {
        self.track_modify_other_keys(mode);
        self.engine.set_modify_other_keys(mode);
    }

    fn report_keyboard_mode(&mut self) {
        self.report_keyboard_mode();
    }

    // ------------------------------------------------------------------
    // Intercepted: Ghostty-compatible reports and protocol entry points.
    // ------------------------------------------------------------------

    fn identify_terminal(&mut self, intermediate: Option<char>) {
        let req = match intermediate {
            None => DeviceAttributeReq::Primary,
            Some('>') => DeviceAttributeReq::Secondary,
            Some('=') => DeviceAttributeReq::Tertiary,
            _ => return,
        };
        self.report_device_attributes(req);
    }

    fn device_status(&mut self, arg: usize) {
        let req = match arg {
            5 => reports::DeviceStatusReq::OperatingStatus,
            6 => reports::DeviceStatusReq::CursorPosition,
            _ => return,
        };
        self.report_device_status(req);
    }

    fn report_mode(&mut self, mode: Mode) {
        self.report_mode_impl(mode.raw(), true);
    }

    fn report_private_mode(&mut self, mode: PrivateMode) {
        self.report_mode_impl(mode.raw(), false);
    }

    fn text_area_size_pixels(&mut self) {
        self.report_size(SizeReportStyle::Csi14T);
    }

    fn text_area_size_chars(&mut self) {
        self.report_size(SizeReportStyle::Csi18T);
    }

    fn bell(&mut self) {
        self.bell_count += 1;
        self.sink.bell();
        self.engine.bell();
    }

    fn set_color(&mut self, index: usize, color: VteRgb) {
        self.set_engine_color(index, color);
    }

    fn reset_color(&mut self, index: usize) {
        self.reset_engine_color(index);
    }

    fn dynamic_color_sequence(&mut self, _prefix: String, _index: usize, _terminator: &str) {
        // OSC color queries are answered by the OSC dispatcher with the
        // Ghostty encoders; nothing to do here.
    }

    fn clipboard_store(&mut self, _clipboard: u8, _base64: &[u8]) {
        // OSC 52 is parsed and dispatched by the OSC layer; vte's default
        // decode never runs.
    }

    fn clipboard_load(&mut self, _clipboard: u8, _terminator: &str) {}

    fn set_hyperlink(&mut self, hyperlink: Option<Hyperlink>) {
        self.engine.set_hyperlink(hyperlink);
    }
}

impl TerminalProtocol {
    fn report_mode_impl(&mut self, mode: u16, ansi: bool) {
        self.report_mode(mode, ansi);
    }

    fn dispatch_dcs(&mut self, command: dcs::Command) {
        match command {
            dcs::Command::Xtgettcap { keys } => {
                let map = mr_crabs_protocols::terminfo::GHOSTTY.xtgettcap_map();
                for key in keys {
                    let key_str = String::from_utf8_lossy(&key);
                    if key_str == "544E" {
                        let name = self.sink.terminfo_name();
                        self.reply.clear();
                        reports::encode_terminfo_name(&name, &mut self.reply);
                        if !self.reply.is_empty() {
                            self.flush_reply();
                        }
                        continue;
                    }
                    if let Some(reply) = map.get(key_str.as_ref()) {
                        self.write_pty(reply);
                    }
                }
            }
            dcs::Command::Decrqss(request) => {
                self.reply.clear();
                let mut reply = std::mem::take(&mut self.reply);
                request.encode(self, &mut reply);
                self.write_pty(&reply);
                self.reply = reply;
            }
            dcs::Command::Tmux(notification) => {
                self.sink.tmux(&notification);
            }
        }
    }

    fn apply_attr(&mut self, attr: &Attr) {
        let sgr_attr = match attr {
            Attr::Reset => Some(SgrAttr::Reset),
            Attr::Bold => Some(SgrAttr::Bold),
            Attr::Dim => Some(SgrAttr::Faint),
            Attr::Italic => Some(SgrAttr::Italic),
            Attr::Underline => Some(SgrAttr::Underline(UnderlineStyle::Solid)),
            Attr::DoubleUnderline => Some(SgrAttr::Underline(UnderlineStyle::Double)),
            Attr::Undercurl => Some(SgrAttr::Underline(UnderlineStyle::Curly)),
            Attr::DottedUnderline => Some(SgrAttr::Underline(UnderlineStyle::Dotted)),
            Attr::DashedUnderline => Some(SgrAttr::Underline(UnderlineStyle::Dashed)),
            Attr::BlinkSlow | Attr::BlinkFast => Some(SgrAttr::Blink),
            Attr::Reverse => Some(SgrAttr::Inverse),
            Attr::Hidden => Some(SgrAttr::Invisible),
            Attr::Strike => Some(SgrAttr::Strikethrough),
            // The SgrState has no bold-off/faint-off/italic-off slots;
            // SGR 22/23 clear through the terminal attribute engine only.
            Attr::CancelBold | Attr::CancelBoldDim | Attr::CancelItalic => None,
            Attr::CancelUnderline => Some(SgrAttr::NoUnderline),
            Attr::CancelBlink => Some(SgrAttr::NoBlink),
            Attr::CancelReverse => Some(SgrAttr::NoInverse),
            Attr::CancelHidden => Some(SgrAttr::NoInvisible),
            Attr::CancelStrike => Some(SgrAttr::NoStrikethrough),
            Attr::Foreground(color) => Some(SgrAttr::Foreground(color_spec(*color))),
            Attr::Background(color) => Some(SgrAttr::Background(color_spec(*color))),
            Attr::UnderlineColor(_) => None,
        };
        if let Some(sgr_attr) = sgr_attr {
            self.sgr.apply(sgr_attr);
        }
    }
}

fn color_spec(color: Color) -> Option<mr_crabs_protocols::sgr::ColorSpec> {
    use mr_crabs_protocols::sgr::ColorSpec;
    match color {
        Color::Named(_) => None,
        Color::Indexed(i) => Some(ColorSpec::Indexed(i)),
        Color::Spec(rgb) => Some(ColorSpec::Rgb(Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        })),
    }
}

/// Map an OSC 10-19 dynamic color onto CompactEngine named color slots
/// (foreground/background/cursor). The remaining dynamic colors (pointer,
/// Tektronix, highlight) have no engine slot and are ignored.
fn dynamic_color_index(dynamic: DynamicColor) -> Option<usize> {
    Some(match dynamic {
        DynamicColor::Foreground => NamedColor::Foreground as usize,
        DynamicColor::Background => NamedColor::Background as usize,
        DynamicColor::Cursor => NamedColor::Cursor as usize,
        _ => return None,
    })
}

impl DecrqssContext for TerminalProtocol {
    fn sgr_attributes(&self, out: &mut Vec<u8>) {
        self.sgr.print_attributes(out);
    }

    fn cursor_blinking(&self) -> bool {
        self.engine.cursor_style().blinking
    }

    fn cursor_shape(&self) -> CursorShapeKind {
        match self.engine.cursor_style().shape {
            CursorShape::Block => CursorShapeKind::Block,
            CursorShape::HollowBlock => CursorShapeKind::BlockHollow,
            CursorShape::Underline => CursorShapeKind::Underline,
            CursorShape::Beam => CursorShapeKind::Bar,
            CursorShape::Hidden => CursorShapeKind::Block,
        }
    }

    fn scrolling_region_top(&self) -> usize {
        self.scroll_region_start
    }

    fn scrolling_region_bottom(&self) -> usize {
        self.scroll_region_end.saturating_sub(1)
    }

    fn left_right_margins_enabled(&self) -> bool {
        // CompactEngine does not track DECSLRM.
        false
    }

    fn scrolling_region_left(&self) -> usize {
        0
    }

    fn scrolling_region_right(&self) -> usize {
        usize::from(self.engine.size().cols).saturating_sub(1)
    }
}

// -------------------------------------------------------------------------
// Snapshot payload for the normalized snapshot + replay target.
// -------------------------------------------------------------------------

impl SnapshotPayload for NormalizedSnapshot {
    fn encode_payload(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        // A zero column count was invalid in the legacy payload, so it is an
        // unambiguous version marker. Version 3 preserves full u16 cursor
        // coordinates and visible-grid OSC 8 hyperlink identities.
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&3u16.to_le_bytes());
        out.extend_from_slice(&self.size.cols.to_le_bytes());
        out.extend_from_slice(&self.size.rows.to_le_bytes());
        out.extend_from_slice(&self.cursor.row.to_le_bytes());
        out.extend_from_slice(&self.cursor.col.to_le_bytes());
        out.push(self.cursor.wrap_pending as u8);
        out.extend_from_slice(&(self.cells.len() as u32).to_le_bytes());
        for cell in &self.cells {
            out.extend_from_slice(&cell.content.to_le_bytes());
            out.extend_from_slice(&cell.style.to_le_bytes());
            out.extend_from_slice(&cell.flags.to_le_bytes());
        }
        out.extend_from_slice(&(self.styles.len() as u32).to_le_bytes());
        for style in &self.styles {
            encode_style(style, out);
        }
        out.extend_from_slice(&(self.combining_marks.len() as u32).to_le_bytes());
        for mark in &self.combining_marks {
            out.extend_from_slice(&mark.cell_index.to_le_bytes());
            out.extend_from_slice(&(mark.codepoints.len() as u32).to_le_bytes());
            for cp in &mark.codepoints {
                out.extend_from_slice(&cp.to_le_bytes());
            }
        }
        out.extend_from_slice(&(self.hyperlinks.len() as u32).to_le_bytes());
        for link in &self.hyperlinks {
            out.extend_from_slice(&link.cell_index.to_le_bytes());
            out.push(link.id.is_some() as u8);
            if let Some(id) = &link.id {
                out.extend_from_slice(&(id.len() as u32).to_le_bytes());
                out.extend_from_slice(id.as_bytes());
            }
            out.extend_from_slice(&(link.uri.len() as u32).to_le_bytes());
            out.extend_from_slice(link.uri.as_bytes());
        }
        out.extend_from_slice(&(self.modes.len() as u32).to_le_bytes());
        for mode in &self.modes {
            let name = format!("{mode:?}");
            out.extend_from_slice(&(name.len() as u32).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
        }
        Ok(())
    }

    fn decode_payload(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader { bytes, pos: 0 };
        let first = r.u16()?;
        let (cols, rows, cursor, version) = if first == 0 {
            let version = r.u16()?;
            if !matches!(version, 2 | 3) {
                return Err(DecodeError::PayloadDecode(
                    "unsupported terminal payload version".into(),
                ));
            }
            let cols = r.u16()?;
            let rows = r.u16()?;
            let cursor = crate::CursorSnapshot {
                row: r.u16()?,
                col: r.u16()?,
                wrap_pending: r.u8()? != 0,
            };
            (cols, rows, cursor, version)
        } else {
            let cols = first;
            let rows = r.u16()?;
            let cursor = crate::CursorSnapshot {
                row: u16::from(r.u8()?),
                col: u16::from(r.u8()?),
                wrap_pending: r.u8()? != 0,
            };
            (cols, rows, cursor, 1)
        };
        if cols == 0 || rows == 0 {
            return Err(DecodeError::PayloadDecode("invalid size".into()));
        }
        let cell_count = r.u32()? as usize;
        let expected = usize::from(cols) * usize::from(rows);
        if cell_count != expected {
            return Err(DecodeError::PayloadDecode("cell count mismatch".into()));
        }
        let encoded_cell_bytes = cell_count
            .checked_mul(std::mem::size_of::<Cell>())
            .ok_or_else(|| DecodeError::PayloadDecode("cell bytes overflow".into()))?;
        if encoded_cell_bytes > r.bytes.len().saturating_sub(r.pos) {
            return Err(DecodeError::TruncatedRecord);
        }
        let mut cells = Vec::with_capacity(cell_count);
        for _ in 0..cell_count {
            cells.push(Cell {
                content: r.u32()?,
                style: r.u16()?,
                flags: r.u16()?,
            });
        }
        let style_count = r.u32()? as usize;
        if style_count > cell_count + 1 {
            return Err(DecodeError::PayloadDecode(
                "style count out of bounds".into(),
            ));
        }
        if style_count > r.bytes.len().saturating_sub(r.pos) {
            return Err(DecodeError::TruncatedRecord);
        }
        let mut styles = Vec::with_capacity(style_count);
        for _ in 0..style_count {
            styles.push(decode_style(&mut r)?);
        }
        let mark_count = r.u32()? as usize;
        if mark_count > cell_count {
            return Err(DecodeError::PayloadDecode(
                "mark count out of bounds".into(),
            ));
        }
        if mark_count > r.bytes.len().saturating_sub(r.pos) / 8 {
            return Err(DecodeError::TruncatedRecord);
        }
        let mut combining_marks = Vec::with_capacity(mark_count);
        for _ in 0..mark_count {
            let cell_index = r.u32()?;
            let cp_count = r.u32()? as usize;
            if cp_count > 32 {
                return Err(DecodeError::PayloadDecode(
                    "codepoint count out of bounds".into(),
                ));
            }
            let mut codepoints = Vec::with_capacity(cp_count);
            for _ in 0..cp_count {
                codepoints.push(r.u32()?);
            }
            combining_marks.push(crate::CombiningMarks {
                cell_index,
                codepoints,
            });
        }
        let mut hyperlinks = Vec::new();
        if version >= 3 {
            let hyperlink_count = r.u32()? as usize;
            if hyperlink_count > cell_count
                || hyperlink_count > r.bytes.len().saturating_sub(r.pos) / 9
            {
                return Err(DecodeError::PayloadDecode(
                    "hyperlink count out of bounds".into(),
                ));
            }
            hyperlinks.reserve(hyperlink_count);
            for _ in 0..hyperlink_count {
                let cell_index = r.u32()?;
                if usize::try_from(cell_index)
                    .ok()
                    .is_none_or(|index| index >= cell_count)
                {
                    return Err(DecodeError::PayloadDecode(
                        "hyperlink cell out of bounds".into(),
                    ));
                }
                let has_id = r.u8()?;
                if has_id > 1 {
                    return Err(DecodeError::PayloadDecode(
                        "invalid hyperlink id marker".into(),
                    ));
                }
                let id = if has_id == 1 {
                    let len = r.u32()? as usize;
                    if len > 1024 {
                        return Err(DecodeError::PayloadDecode(
                            "hyperlink id out of bounds".into(),
                        ));
                    }
                    Some(
                        String::from_utf8(r.bytes(len)?.to_vec())
                            .map_err(|_| DecodeError::PayloadDecode("bad hyperlink id".into()))?,
                    )
                } else {
                    None
                };
                let uri_len = r.u32()? as usize;
                if uri_len > 4096 {
                    return Err(DecodeError::PayloadDecode(
                        "hyperlink URI out of bounds".into(),
                    ));
                }
                let uri = String::from_utf8(r.bytes(uri_len)?.to_vec())
                    .map_err(|_| DecodeError::PayloadDecode("bad hyperlink URI".into()))?;
                hyperlinks.push(crate::SnapshotHyperlink {
                    cell_index,
                    id,
                    uri,
                });
            }
        }
        let mode_count = r.u32()? as usize;
        if mode_count > 64 {
            return Err(DecodeError::PayloadDecode(
                "mode count out of bounds".into(),
            ));
        }
        let mut modes = Vec::with_capacity(mode_count);
        for _ in 0..mode_count {
            let len = r.u32()? as usize;
            if len > 64 {
                return Err(DecodeError::PayloadDecode("mode name out of bounds".into()));
            }
            let name = String::from_utf8(r.bytes(len)?.to_vec())
                .map_err(|_| DecodeError::PayloadDecode("bad mode name".into()))?;
            let mode = crate::TerminalMode::from_debug_name(&name)
                .ok_or_else(|| DecodeError::PayloadDecode("unknown mode name".into()))?;
            modes.push(mode);
        }
        Ok(NormalizedSnapshot {
            size: GridSize::new(cols, rows),
            cursor,
            cells,
            styles,
            combining_marks,
            hyperlinks,
            modes,
        })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.bytes.len() - self.pos < n {
            return Err(DecodeError::TruncatedRecord);
        }
        let slice = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        self.take(n)
    }
}

fn encode_style(style: &crate::Style, out: &mut Vec<u8>) {
    encode_color(&style.foreground, out);
    encode_color(&style.background, out);
    match &style.underline {
        Some(color) => {
            out.push(1);
            encode_color(color, out);
        }
        None => out.push(0),
    }
}

fn encode_color(color: &crate::NormalizedColor, out: &mut Vec<u8>) {
    // Re-encode through the normalized color serialization.
    let json = serde_json::to_vec(color).expect("color serialization cannot fail");
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
}

fn decode_style(r: &mut Reader<'_>) -> Result<crate::Style, DecodeError> {
    let foreground = decode_color(r)?;
    let background = decode_color(r)?;
    let underline = match r.u8()? {
        0 => None,
        1 => Some(decode_color(r)?),
        _ => return Err(DecodeError::PayloadDecode("bad underline flag".into())),
    };
    Ok(crate::Style {
        foreground,
        background,
        underline,
    })
}

fn decode_color(r: &mut Reader<'_>) -> Result<crate::NormalizedColor, DecodeError> {
    let len = r.u32()? as usize;
    if len > 128 {
        return Err(DecodeError::PayloadDecode("color too long".into()));
    }
    let bytes = r.bytes(len)?;
    serde_json::from_slice(bytes).map_err(|_| DecodeError::PayloadDecode("bad color".into()))
}

#[cfg(test)]
mod snapshot_payload_tests {
    use super::*;

    fn snapshot(cols: u16, rows: u16, cursor: crate::CursorSnapshot) -> NormalizedSnapshot {
        NormalizedSnapshot {
            size: GridSize::new(cols, rows),
            cursor,
            cells: vec![crate::Cell::default(); usize::from(cols) * usize::from(rows)],
            styles: Vec::new(),
            combining_marks: Vec::new(),
            hyperlinks: Vec::new(),
            modes: Vec::new(),
        }
    }

    #[test]
    fn snapshot_v3_roundtrips_u16_cursor_coordinates() {
        let expected = snapshot(
            301,
            301,
            crate::CursorSnapshot {
                row: 300,
                col: 300,
                wrap_pending: true,
            },
        );
        let mut bytes = Vec::new();
        expected.encode_payload(&mut bytes).expect("encode");
        assert_eq!(&bytes[..4], &[0, 0, 3, 0]);
        assert_eq!(
            NormalizedSnapshot::decode_payload(&bytes).expect("decode"),
            expected
        );
    }

    #[test]
    fn snapshot_v3_roundtrips_hyperlink_identity() {
        let mut expected = snapshot(
            2,
            1,
            crate::CursorSnapshot {
                row: 0,
                col: 0,
                wrap_pending: false,
            },
        );
        expected.hyperlinks.push(crate::SnapshotHyperlink {
            cell_index: 1,
            id: Some("docs".into()),
            uri: "https://example.com".into(),
        });
        let mut bytes = Vec::new();
        expected.encode_payload(&mut bytes).expect("encode");
        assert_eq!(
            NormalizedSnapshot::decode_payload(&bytes).expect("decode"),
            expected
        );
    }

    #[test]
    fn snapshot_decoder_accepts_legacy_u8_cursor_payload() {
        let expected = snapshot(
            2,
            1,
            crate::CursorSnapshot {
                row: 0,
                col: 1,
                wrap_pending: false,
            },
        );
        let mut version_two = Vec::new();
        expected
            .encode_payload(&mut version_two)
            .expect("version two");
        let mut legacy = Vec::new();
        legacy.extend_from_slice(&expected.size.cols.to_le_bytes());
        legacy.extend_from_slice(&expected.size.rows.to_le_bytes());
        legacy.push(expected.cursor.row as u8);
        legacy.push(expected.cursor.col as u8);
        legacy.push(expected.cursor.wrap_pending as u8);
        legacy.extend_from_slice(&version_two[13..]);
        assert_eq!(
            NormalizedSnapshot::decode_payload(&legacy).expect("legacy decode"),
            expected
        );
    }

    #[test]
    fn snapshot_decoder_rejects_huge_declared_grid_before_allocating() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&u16::MAX.to_le_bytes());
        bytes.extend_from_slice(&u16::MAX.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.push(0);
        let cells = u32::from(u16::MAX) * u32::from(u16::MAX);
        bytes.extend_from_slice(&cells.to_le_bytes());
        assert_eq!(
            NormalizedSnapshot::decode_payload(&bytes),
            Err(DecodeError::TruncatedRecord)
        );
    }
}
