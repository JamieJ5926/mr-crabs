//! Kitty graphics protocol command parser and response encoder.
//!
//! Faithful port of `src/terminal/kitty/graphics_command.zig` (Ghostty
//! `d2c70a8c7b9b6893c13640c02d7b6f9a1624f3f0`). The parser is a byte
//! state machine fed the APC payload after the `G` (i.e. `\x1b_G` + payload
//! + `\x1b\\`). Key/value control data is a dense `[52]u32` table plus a
//!   presence bitmap; values are single printable ASCII codes or parsed
//!   unsigned/signed 32-bit integers exactly as the oracle does.

use crate::image::{Compression, ImageFormat, Medium};

/// Parser errors. After any error the parser must be discarded; a command
/// that errored mid-feed can never complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Structural problem (bad key/value shape, unknown action, invalid
    /// enum value, missing required fields, range with x > y, ...).
    InvalidFormat,
    /// A numeric value did not fit its 32-bit field.
    Overflow,
    /// The data payload exceeded `max_bytes`.
    OutOfMemory,
    /// The data payload was not valid base64.
    InvalidData,
}

/// Key/value pairs for the control information of a command. Keys are always
/// single ASCII letters; values are single characters or 32-bit integers.
/// Unknown keys are ignored (the oracle's `KV.put` drops them).
#[derive(Clone, Copy, Debug)]
pub struct Kvs {
    values: [u32; 52],
    present: u64,
}

impl Default for Kvs {
    fn default() -> Self {
        Self {
            values: [0; 52],
            present: 0,
        }
    }
}

fn index(key: u8) -> Option<usize> {
    match key {
        b'a'..=b'z' => Some((key - b'a') as usize),
        b'A'..=b'Z' => Some(26 + (key - b'A') as usize),
        _ => None,
    }
}

impl Kvs {
    pub fn get(&self, key: u8) -> Option<u32> {
        let idx = index(key)?;
        if self.present & (1u64 << idx) == 0 {
            return None;
        }
        Some(self.values[idx])
    }

    fn put(&mut self, key: u8, value: u32) {
        if let Some(idx) = index(key) {
            self.values[idx] = value;
            self.present |= 1u64 << idx;
        }
    }
}

/// Command action (`a=` key, default `t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Query,
    Transmit,
    TransmitAndDisplay,
    Display,
    Delete,
    TransmitAnimationFrame,
    ControlAnimation,
    ComposeAnimation,
}

/// Quiet setting (`q=` key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quiet {
    No,
    Ok,
    Failures,
}

/// Transmission control data (`graphics_command.zig` `Transmission`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transmission {
    pub format: ImageFormat,
    pub medium: Medium,
    pub width: u32,
    pub height: u32,
    pub size: u32,
    pub offset: u32,
    pub image_id: u32,
    pub image_number: u32,
    pub placement_id: u32,
    pub compression: Compression,
    pub more_chunks: bool,
    pub transient: bool,
}

impl Default for Transmission {
    fn default() -> Self {
        Self {
            format: ImageFormat::Rgba,
            medium: Medium::Direct,
            width: 0,
            height: 0,
            size: 0,
            offset: 0,
            image_id: 0,
            image_number: 0,
            placement_id: 0,
            compression: Compression::None,
            more_chunks: false,
            transient: false,
        }
    }
}

/// Display control data (`graphics_command.zig` `Display`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Display {
    pub image_id: u32,
    pub image_number: u32,
    pub placement_id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub columns: u32,
    pub rows: u32,
    pub cursor_movement: CursorMovement,
    pub virtual_placement: bool,
    pub parent_id: u32,
    pub parent_placement_id: u32,
    pub horizontal_offset: i32,
    pub vertical_offset: i32,
    pub z: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorMovement {
    After,
    None,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            image_id: 0,
            image_number: 0,
            placement_id: 0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            x_offset: 0,
            y_offset: 0,
            columns: 0,
            rows: 0,
            cursor_movement: CursorMovement::After,
            virtual_placement: false,
            parent_id: 0,
            parent_placement_id: 0,
            horizontal_offset: 0,
            vertical_offset: 0,
            z: 0,
        }
    }
}

/// Delete command control data (`graphics_command.zig` `Delete`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delete {
    /// `d=a` / `d=A`: delete all placements; uppercase also deletes images.
    All { delete_images: bool },
    /// `d=i` / `d=I`: by image id (and optional placement id).
    Id {
        delete: bool,
        image_id: u32,
        placement_id: u32,
    },
    /// `d=n` / `d=N`: by image number.
    Newest {
        delete: bool,
        image_number: u32,
        placement_id: u32,
    },
    /// `d=c` / `d=C`: intersect cursor cell.
    IntersectCursor { delete_images: bool },
    /// `d=f` / `d=F`: animation frames (accepted, no-op: we hold no frames).
    AnimationFrames,
    /// `d=p` / `d=P`: intersect cell.
    IntersectCell { delete: bool, x: u32, y: u32 },
    /// `d=q` / `d=Q`: intersect cell with z filter.
    IntersectCellZ {
        delete: bool,
        x: u32,
        y: u32,
        z: i32,
    },
    /// `d=r` / `d=R`: image id range.
    Range { delete: bool, first: u32, last: u32 },
    /// `d=x` / `d=X`: intersect column.
    Column { delete: bool, x: u32 },
    /// `d=y` / `d=Y`: intersect row.
    Row { delete: bool, y: u32 },
    /// `d=z` / `d=Z`: by z index.
    Z { delete: bool, z: i32 },
}

/// Animation frame loading control data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationFrameLoading {
    pub x: u32,
    pub y: u32,
    pub create_frame: u32,
    pub edit_frame: u32,
    pub gap_ms: u32,
    pub composition_mode: CompositionMode,
    pub background: [u8; 4],
}

/// Animation frame composition control data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationFrameComposition {
    pub frame: u32,
    pub edit_frame: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub left_edge: u32,
    pub top_edge: u32,
    pub composition_mode: CompositionMode,
}

/// Animation control data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationControl {
    pub action: AnimationAction,
    pub frame: u32,
    pub gap_ms: u32,
    pub current_frame: u32,
    pub loops: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationAction {
    Invalid,
    Stop,
    RunWait,
    Run,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositionMode {
    AlphaBlend,
    Overwrite,
}

/// The parsed control half of a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Query(Transmission),
    Transmit(Transmission),
    TransmitAndDisplay {
        transmission: Transmission,
        display: Display,
    },
    Display(Display),
    Delete(Delete),
    TransmitAnimationFrame(AnimationFrameLoading),
    ControlAnimation(AnimationControl),
    ComposeAnimation(AnimationFrameComposition),
}

/// A fully parsed kitty graphics command: control data plus the base64
/// decoded payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub control: Control,
    pub quiet: Quiet,
    /// Base64-decoded payload bytes; empty when no data was transmitted.
    pub data: Vec<u8>,
}

impl Command {
    /// The transmission data if this command carries one.
    pub fn transmission(&self) -> Option<Transmission> {
        match self.control {
            Control::Query(t) | Control::Transmit(t) => Some(t),
            Control::TransmitAndDisplay { transmission, .. } => Some(transmission),
            _ => None,
        }
    }

    /// The display data if this command carries one.
    pub fn display(&self) -> Option<Display> {
        match self.control {
            Control::Display(d) => Some(d),
            Control::TransmitAndDisplay { display, .. } => Some(display),
            _ => None,
        }
    }
}

/// A possible response to a command (`graphics_command.zig` `Response`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Response {
    pub id: u32,
    pub image_number: u32,
    pub placement_id: u32,
    pub message: &'static str,
}

impl Default for Response {
    fn default() -> Self {
        Self {
            id: 0,
            image_number: 0,
            placement_id: 0,
            message: "OK",
        }
    }
}

impl Response {
    pub fn ok(self) -> bool {
        self.message == "OK"
    }

    /// An empty response carries neither an id nor an image number and must
    /// not be written to the terminal.
    pub fn empty(self) -> bool {
        self.id == 0 && self.image_number == 0
    }

    /// Encode into the APC response wire format: `ESC _ G <i/I/p>;msg ESC \`.
    /// Encodes nothing when the response is empty.
    pub fn encode(self, out: &mut Vec<u8>) {
        if self.empty() {
            return;
        }
        out.push(0x1b);
        out.push(b'_');
        out.push(b'G');
        let mut prior = false;
        if self.id > 0 {
            out.extend_from_slice(format!("i={}", self.id).as_bytes());
            prior = true;
        }
        if self.image_number > 0 {
            if prior {
                out.push(b',');
            }
            out.extend_from_slice(format!("I={}", self.image_number).as_bytes());
            prior = true;
        }
        if self.placement_id > 0 {
            if prior {
                out.push(b',');
            }
            out.extend_from_slice(format!("p={}", self.placement_id).as_bytes());
        }
        out.push(b';');
        out.extend_from_slice(self.message.as_bytes());
        out.push(0x1b);
        out.push(b'\\');
    }
}

/// Byte state machine parsing the payload after `\x1b_G`.
///
/// The first byte fed must be the byte immediately following the `G`, i.e.
/// for `\x1b_G123` the first byte is `1`.
pub struct CommandParser {
    kv: Kvs,
    /// Buffer for the key/value currently being accumulated. Values are at
    /// most 11 characters (u32 max plus sign), exactly like the oracle.
    kv_temp: [u8; 11],
    kv_temp_len: u8,
    kv_current: u8,
    data: Vec<u8>,
    max_bytes: usize,
    state: State,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    ControlKey,
    ControlKeyIgnore,
    ControlValue,
    ControlValueIgnore,
    Data,
}

impl Default for CommandParser {
    fn default() -> Self {
        Self::new(1024 * 1024)
    }
}

impl CommandParser {
    /// Create a parser with an explicit payload byte bound (the oracle
    /// defaults to 65 MiB for kitty via `Protocol.defaultMaxBytes(.kitty)`).
    pub fn new(max_bytes: usize) -> Self {
        Self {
            kv: Kvs::default(),
            kv_temp: [0; 11],
            kv_temp_len: 0,
            kv_current: 0,
            data: Vec::new(),
            max_bytes,
            state: State::ControlKey,
        }
    }

    /// Feed a single byte. On error the parser must be discarded.
    pub fn feed(&mut self, c: u8) -> Result<(), ParseError> {
        match self.state {
            State::ControlKey => match c {
                b'=' => {
                    if self.kv_temp_len != 1 {
                        // All control keys are a single character; ignore
                        // follow-up data for this key.
                        self.state = State::ControlValueIgnore;
                        self.kv_temp_len = 0;
                    } else {
                        self.kv_current = self.kv_temp[0];
                        self.kv_temp_len = 0;
                        self.state = State::ControlValue;
                    }
                }
                // No control data, only payload: `\x1b_G;<data>` is valid.
                b';' => self.state = State::Data,
                _ => self.accumulate_value(c, State::ControlKeyIgnore),
            },
            State::ControlKeyIgnore => {
                if c == b'=' {
                    self.state = State::ControlValueIgnore
                }
            }
            State::ControlValue => match c {
                b',' => {
                    self.finish_value(State::ControlKey)?;
                }
                b';' => {
                    self.finish_value(State::Data)?;
                }
                _ => self.accumulate_value(c, State::ControlValueIgnore),
            },
            State::ControlValueIgnore => match c {
                b',' => self.state = State::ControlKeyIgnore,
                b';' => self.state = State::Data,
                _ => {}
            },
            State::Data => {
                if self.data.len() >= self.max_bytes {
                    return Err(ParseError::OutOfMemory);
                }
                self.data.push(c);
            }
        }
        Ok(())
    }

    /// Feed a slice; the data state appends the remainder in bulk. On error
    /// the parser must be discarded.
    pub fn feed_slice(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        let mut rem = bytes;
        while !rem.is_empty() {
            if self.state == State::Data {
                if self.data.len().saturating_add(rem.len()) > self.max_bytes {
                    return Err(ParseError::OutOfMemory);
                }
                self.data.extend_from_slice(rem);
                return Ok(());
            }
            self.feed(rem[0])?;
            rem = &rem[1..];
        }
        Ok(())
    }

    /// Complete parsing, decoding the payload from base64. The returned
    /// command owns its data.
    pub fn complete(&mut self) -> Result<Command, ParseError> {
        match self.state {
            // We can't ever end in the control key state and be valid.
            State::ControlKey | State::ControlKeyIgnore => return Err(ParseError::InvalidFormat),
            State::ControlValue => self.finish_value(State::Data)?,
            State::ControlValueIgnore => {}
            State::Data => {}
        }

        let action: u8 = match self.kv.get(b'a') {
            Some(v) => u8::try_from(v).map_err(|_| ParseError::InvalidFormat)?,
            None => b't',
        };
        let control = match action {
            b'q' => Control::Query(parse_transmission(&self.kv)?),
            b't' => Control::Transmit(parse_transmission(&self.kv)?),
            b'T' => Control::TransmitAndDisplay {
                transmission: parse_transmission(&self.kv)?,
                display: parse_display(&self.kv)?,
            },
            b'p' => Control::Display(parse_display(&self.kv)?),
            b'd' => Control::Delete(parse_delete(&self.kv)?),
            b'f' => Control::TransmitAnimationFrame(parse_animation_frame_loading(&self.kv)?),
            b'a' => Control::ControlAnimation(parse_animation_control(&self.kv)?),
            b'c' => Control::ComposeAnimation(parse_animation_frame_composition(&self.kv)?),
            _ => return Err(ParseError::InvalidFormat),
        };

        let quiet = match self.kv.get(b'q') {
            None => Quiet::No,
            Some(0) => Quiet::No,
            Some(1) => Quiet::Ok,
            Some(2) => Quiet::Failures,
            Some(_) => return Err(ParseError::InvalidFormat),
        };

        Ok(Command {
            control,
            quiet,
            data: self.decode_data()?,
        })
    }

    /// Decode the collected payload from base64, tolerating a missing
    /// final padding quantum exactly like the oracle's decoder.
    fn decode_data(&mut self) -> Result<Vec<u8>, ParseError> {
        if self.data.is_empty() {
            return Ok(Vec::new());
        }
        crate::image::decode_base64_lenient(&self.data).map_err(|_| ParseError::InvalidData)
    }

    fn accumulate_value(&mut self, c: u8, overflow_state: State) {
        let idx = self.kv_temp_len as usize;
        self.kv_temp_len += 1;
        if self.kv_temp_len as usize > self.kv_temp.len() {
            self.state = overflow_state;
            self.kv_temp_len = 0;
            return;
        }
        self.kv_temp[idx] = c;
    }

    fn finish_value(&mut self, next_state: State) -> Result<(), ParseError> {
        self.state = next_state;

        // Single non-digit characters are stored as their ASCII code.
        if self.kv_temp_len == 1 {
            let c = self.kv_temp[0];
            if !c.is_ascii_digit() {
                self.kv.put(self.kv_current, c as u32);
                self.kv_temp_len = 0;
                return Ok(());
            }
        }

        let text = std::str::from_utf8(&self.kv_temp[..self.kv_temp_len as usize])
            .map_err(|_| ParseError::InvalidFormat)?;
        let v: u32 = match self.kv_current {
            // Signed fields, stored bitcast.
            b'z' | b'H' | b'V' => {
                let signed: i32 = text.parse().map_err(|_| ParseError::Overflow)?;
                signed as u32
            }
            _ => text.parse().map_err(|_| ParseError::Overflow)?,
        };
        self.kv.put(self.kv_current, v);
        self.kv_temp_len = 0;
        Ok(())
    }
}

fn parse_transmission(kv: &Kvs) -> Result<Transmission, ParseError> {
    let mut result = Transmission::default();
    if let Some(v) = kv.get(b'f') {
        result.format = ImageFormat::from_code(v).ok_or(ParseError::InvalidFormat)?;
    }
    if let Some(v) = kv.get(b't') {
        let c = u8::try_from(v).map_err(|_| ParseError::InvalidFormat)?;
        result.medium = Medium::from_code(c).ok_or(ParseError::InvalidFormat)?;
    }
    if let Some(v) = kv.get(b's') {
        result.width = v;
    }
    if let Some(v) = kv.get(b'v') {
        result.height = v;
    }
    if let Some(v) = kv.get(b'S') {
        result.size = v;
    }
    if let Some(v) = kv.get(b'O') {
        result.offset = v;
    }
    if let Some(v) = kv.get(b'i') {
        result.image_id = v;
    }
    if let Some(v) = kv.get(b'I') {
        result.image_number = v;
    }
    if let Some(v) = kv.get(b'p') {
        result.placement_id = v;
    }
    if let Some(v) = kv.get(b'o') {
        let c = u8::try_from(v).map_err(|_| ParseError::InvalidFormat)?;
        result.compression = match c {
            b'z' => Compression::ZlibDeflate,
            _ => return Err(ParseError::InvalidFormat),
        };
    }
    // The `m` key only applies to the direct medium; local-only mediums
    // ignore it (Kitty + mpv compatibility, see the oracle comment).
    if result.medium == Medium::Direct {
        if let Some(v) = kv.get(b'm') {
            result.more_chunks = v > 0;
        }
    }
    if let Some(v) = kv.get(b'N') {
        result.transient = v & 1 != 0;
    }
    Ok(result)
}

fn parse_display(kv: &Kvs) -> Result<Display, ParseError> {
    let mut result = Display::default();
    if let Some(v) = kv.get(b'i') {
        result.image_id = v;
    }
    if let Some(v) = kv.get(b'I') {
        result.image_number = v;
    }
    if let Some(v) = kv.get(b'p') {
        result.placement_id = v;
    }
    if let Some(v) = kv.get(b'x') {
        result.x = v;
    }
    if let Some(v) = kv.get(b'y') {
        result.y = v;
    }
    if let Some(v) = kv.get(b'w') {
        result.width = v;
    }
    if let Some(v) = kv.get(b'h') {
        result.height = v;
    }
    if let Some(v) = kv.get(b'X') {
        result.x_offset = v;
    }
    if let Some(v) = kv.get(b'Y') {
        result.y_offset = v;
    }
    if let Some(v) = kv.get(b'c') {
        result.columns = v;
    }
    if let Some(v) = kv.get(b'r') {
        result.rows = v;
    }
    if let Some(v) = kv.get(b'C') {
        result.cursor_movement = match v {
            0 => CursorMovement::After,
            1 => CursorMovement::None,
            _ => return Err(ParseError::InvalidFormat),
        };
    }
    if let Some(v) = kv.get(b'U') {
        result.virtual_placement = match v {
            0 => false,
            1 => true,
            _ => return Err(ParseError::InvalidFormat),
        };
    }
    if let Some(v) = kv.get(b'z') {
        result.z = v as i32;
    }
    if let Some(v) = kv.get(b'P') {
        result.parent_id = v;
    }
    if let Some(v) = kv.get(b'Q') {
        result.parent_placement_id = v;
    }
    if let Some(v) = kv.get(b'H') {
        result.horizontal_offset = v as i32;
    }
    if let Some(v) = kv.get(b'V') {
        result.vertical_offset = v as i32;
    }
    Ok(result)
}

fn parse_delete(kv: &Kvs) -> Result<Delete, ParseError> {
    let what: u8 = match kv.get(b'd') {
        Some(v) => u8::try_from(v).map_err(|_| ParseError::InvalidFormat)?,
        None => b'a',
    };

    Ok(match what {
        b'a' | b'A' => Delete::All {
            delete_images: what == b'A',
        },
        b'i' | b'I' => Delete::Id {
            delete: what == b'I',
            image_id: kv.get(b'i').unwrap_or(0),
            placement_id: kv.get(b'p').unwrap_or(0),
        },
        b'n' | b'N' => Delete::Newest {
            delete: what == b'N',
            image_number: kv.get(b'I').unwrap_or(0),
            placement_id: kv.get(b'p').unwrap_or(0),
        },
        b'c' | b'C' => Delete::IntersectCursor {
            delete_images: what == b'C',
        },
        b'f' | b'F' => Delete::AnimationFrames,
        b'p' | b'P' => Delete::IntersectCell {
            delete: what == b'P',
            x: kv.get(b'x').unwrap_or(0),
            y: kv.get(b'y').unwrap_or(0),
        },
        b'q' | b'Q' => Delete::IntersectCellZ {
            delete: what == b'Q',
            x: kv.get(b'x').unwrap_or(0),
            y: kv.get(b'y').unwrap_or(0),
            z: kv.get(b'z').unwrap_or(0) as i32,
        },
        b'r' | b'R' => {
            let x = kv.get(b'x').ok_or(ParseError::InvalidFormat)?;
            let y = kv.get(b'y').ok_or(ParseError::InvalidFormat)?;
            if x > y {
                return Err(ParseError::InvalidFormat);
            }
            Delete::Range {
                delete: what == b'R',
                first: x,
                last: y,
            }
        }
        b'x' | b'X' => Delete::Column {
            delete: what == b'X',
            x: kv.get(b'x').unwrap_or(0),
        },
        b'y' | b'Y' => Delete::Row {
            delete: what == b'Y',
            y: kv.get(b'y').unwrap_or(0),
        },
        b'z' | b'Z' => Delete::Z {
            delete: what == b'Z',
            z: kv.get(b'z').unwrap_or(0) as i32,
        },
        _ => return Err(ParseError::InvalidFormat),
    })
}

fn parse_animation_frame_loading(kv: &Kvs) -> Result<AnimationFrameLoading, ParseError> {
    let mut result = AnimationFrameLoading {
        x: 0,
        y: 0,
        create_frame: 0,
        edit_frame: 0,
        gap_ms: 0,
        composition_mode: CompositionMode::AlphaBlend,
        background: [0; 4],
    };
    if let Some(v) = kv.get(b'x') {
        result.x = v;
    }
    if let Some(v) = kv.get(b'y') {
        result.y = v;
    }
    if let Some(v) = kv.get(b'c') {
        result.create_frame = v;
    }
    if let Some(v) = kv.get(b'r') {
        result.edit_frame = v;
    }
    if let Some(v) = kv.get(b'z') {
        result.gap_ms = v;
    }
    if let Some(v) = kv.get(b'X') {
        result.composition_mode = match v {
            0 => CompositionMode::AlphaBlend,
            1 => CompositionMode::Overwrite,
            _ => return Err(ParseError::InvalidFormat),
        };
    }
    if let Some(v) = kv.get(b'Y') {
        result.background = v.to_le_bytes();
    }
    Ok(result)
}

fn parse_animation_control(kv: &Kvs) -> Result<AnimationControl, ParseError> {
    let mut result = AnimationControl {
        action: AnimationAction::Invalid,
        frame: 0,
        gap_ms: 0,
        current_frame: 0,
        loops: 0,
    };
    if let Some(v) = kv.get(b's') {
        result.action = match v {
            0 => AnimationAction::Invalid,
            1 => AnimationAction::Stop,
            2 => AnimationAction::RunWait,
            3 => AnimationAction::Run,
            _ => return Err(ParseError::InvalidFormat),
        };
    }
    if let Some(v) = kv.get(b'r') {
        result.frame = v;
    }
    if let Some(v) = kv.get(b'z') {
        result.gap_ms = v;
    }
    if let Some(v) = kv.get(b'c') {
        result.current_frame = v;
    }
    if let Some(v) = kv.get(b'v') {
        result.loops = v;
    }
    Ok(result)
}

fn parse_animation_frame_composition(kv: &Kvs) -> Result<AnimationFrameComposition, ParseError> {
    let mut result = AnimationFrameComposition {
        frame: 0,
        edit_frame: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        left_edge: 0,
        top_edge: 0,
        composition_mode: CompositionMode::AlphaBlend,
    };
    for (key, field) in [
        (b'c', &mut result.frame),
        (b'r', &mut result.edit_frame),
        (b'x', &mut result.x),
        (b'y', &mut result.y),
        (b'w', &mut result.width),
        (b'h', &mut result.height),
        (b'X', &mut result.left_edge),
        (b'Y', &mut result.top_edge),
    ] {
        if let Some(v) = kv.get(key) {
            *field = v;
        }
    }
    if let Some(v) = kv.get(b'C') {
        result.composition_mode = match v {
            0 => CompositionMode::AlphaBlend,
            1 => CompositionMode::Overwrite,
            _ => return Err(ParseError::InvalidFormat),
        };
    }
    Ok(result)
}

/// Parse a complete command string in one shot (test/utility entry point,
/// mirrors the oracle's `Parser.parseString` with the default 1 MiB bound).
pub fn parse_string(input: &[u8]) -> Result<Command, ParseError> {
    let mut parser = CommandParser::new(1024 * 1024);
    parser.feed_slice(input)?;
    parser.complete()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{ImageFormat as F, Medium as M};

    fn parse(input: &str) -> Result<Command, ParseError> {
        parse_string(input.as_bytes())
    }

    #[test]
    fn transmission_command() {
        let cmd = parse("f=24,s=10,v=20").unwrap();
        assert!(matches!(cmd.control, Control::Transmit(_)));
        let t = cmd.transmission().unwrap();
        assert_eq!(t.format, F::Rgb);
        assert_eq!(t.width, 10);
        assert_eq!(t.height, 20);
        assert!(!t.transient);
    }

    #[test]
    fn transmission_command_with_transient_hint() {
        let cmd = parse("f=24,s=10,v=20,N=1").unwrap();
        let t = cmd.transmission().unwrap();
        assert_eq!(t.format, F::Rgb);
        assert!(t.transient);
    }

    #[test]
    fn feed_slice_matches_per_byte_feed() {
        let input = "f=24,s=10,v=20;aGVsbG8gd29ybGQ=";
        let mut p1 = CommandParser::new(1024 * 1024);
        for b in input.bytes() {
            p1.feed(b).unwrap();
        }
        let c1 = p1.complete().unwrap();

        let mut p2 = CommandParser::new(1024 * 1024);
        p2.feed_slice(input.as_bytes()).unwrap();
        let c2 = p2.complete().unwrap();

        assert_eq!(c1.control, c2.control);
        assert_eq!(c1.data, c2.data);
    }

    #[test]
    fn feed_slice_across_boundaries() {
        let mut p = CommandParser::new(1024 * 1024);
        p.feed_slice(b"f=24,s=10").unwrap();
        p.feed_slice(b",v=20;aGVsbG8g").unwrap();
        p.feed_slice(b"d29ybGQ=").unwrap();
        let cmd = p.complete().unwrap();
        assert!(matches!(cmd.control, Control::Transmit(_)));
        assert_eq!(cmd.data, b"hello world");
    }

    #[test]
    fn feed_slice_respects_max_bytes() {
        let mut p = CommandParser::new(4);
        p.feed_slice(b"f=24;ab").unwrap();
        assert_eq!(p.feed_slice(b"cde"), Err(ParseError::OutOfMemory));
    }

    #[test]
    fn transmission_ignores_m_for_local_medium() {
        let cmd = parse("a=t,t=t,m=1").unwrap();
        let t = cmd.transmission().unwrap();
        assert_eq!(t.medium, M::TemporaryFile);
        assert!(!t.more_chunks);
    }

    #[test]
    fn transmission_respects_m_for_direct() {
        let cmd = parse("a=t,t=d,m=1").unwrap();
        let t = cmd.transmission().unwrap();
        assert_eq!(t.medium, M::Direct);
        assert!(t.more_chunks);
    }

    #[test]
    fn query_command() {
        let cmd = parse("i=31,s=1,v=1,a=q,t=d,f=24;QUFBQQ").unwrap();
        assert!(matches!(cmd.control, Control::Query(_)));
        let t = cmd.transmission().unwrap();
        assert_eq!(t.medium, M::Direct);
        assert_eq!(t.width, 1);
        assert_eq!(t.height, 1);
        assert_eq!(t.image_id, 31);
        assert_eq!(cmd.data, b"AAAA");
    }

    #[test]
    fn display_command() {
        let cmd = parse("a=p,U=1,i=31,c=80,r=120").unwrap();
        assert!(matches!(cmd.control, Control::Display(_)));
        let d = cmd.display().unwrap();
        assert_eq!(d.columns, 80);
        assert_eq!(d.rows, 120);
        assert_eq!(d.image_id, 31);
    }

    #[test]
    fn delete_command() {
        let cmd = parse("a=d,d=p,x=3,y=4").unwrap();
        assert!(matches!(cmd.control, Control::Delete(_)));
        assert_eq!(
            cmd.control,
            Control::Delete(Delete::IntersectCell {
                delete: false,
                x: 3,
                y: 4
            })
        );
    }

    #[test]
    fn no_control_data() {
        let cmd = parse(";QUFBQQ").unwrap();
        assert!(matches!(cmd.control, Control::Transmit(_)));
        assert_eq!(cmd.data, b"AAAA");
    }

    #[test]
    fn ignore_unknown_keys() {
        for input in ["f=24,s=10,v=20,hello=world", "f=24,s=10,v=20,!=1"] {
            let cmd = parse(input).unwrap();
            let t = cmd.transmission().unwrap();
            assert_eq!(t.format, F::Rgb);
            assert_eq!(t.width, 10);
            assert_eq!(t.height, 20);
        }
    }

    #[test]
    fn ignore_very_long_values() {
        let cmd = parse("f=24,s=10,v=2000000000000000000000000000000000000000").unwrap();
        let t = cmd.transmission().unwrap();
        assert_eq!(t.format, F::Rgb);
        assert_eq!(t.width, 10);
        assert_eq!(t.height, 0);
    }

    #[test]
    fn very_large_negative_values_not_skipped() {
        let cmd = parse("a=p,i=1,z=-2000000000").unwrap();
        let d = cmd.display().unwrap();
        assert_eq!(d.image_id, 1);
        assert_eq!(d.z, -2000000000);
    }

    #[test]
    fn overflow_errors() {
        assert_eq!(parse("a=p,i=10000000000"), Err(ParseError::Overflow));
        assert_eq!(parse("a=p,i=1,z=-9999999999"), Err(ParseError::Overflow));
    }

    #[test]
    fn all_i32_values() {
        let cmd = parse("a=p,i=1,z=-1").unwrap();
        assert_eq!(cmd.display().unwrap().z, -1);
        let cmd = parse("a=p,i=1,H=-1").unwrap();
        assert_eq!(cmd.display().unwrap().horizontal_offset, -1);
        let cmd = parse("a=p,i=1,V=-1").unwrap();
        assert_eq!(cmd.display().unwrap().vertical_offset, -1);
    }

    #[test]
    fn response_encode_nothing_without_id_or_number() {
        let mut out = Vec::new();
        Response::default().encode(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn response_encode_with_only_image_id() {
        let mut out = Vec::new();
        Response {
            id: 4,
            ..Response::default()
        }
        .encode(&mut out);
        assert_eq!(out, b"\x1b_Gi=4;OK\x1b\\");
    }

    #[test]
    fn response_encode_with_only_image_number() {
        let mut out = Vec::new();
        Response {
            image_number: 4,
            ..Response::default()
        }
        .encode(&mut out);
        assert_eq!(out, b"\x1b_GI=4;OK\x1b\\");
    }

    #[test]
    fn response_encode_with_id_and_number() {
        let mut out = Vec::new();
        Response {
            id: 12,
            image_number: 4,
            ..Response::default()
        }
        .encode(&mut out);
        assert_eq!(out, b"\x1b_Gi=12,I=4;OK\x1b\\");
    }

    #[test]
    fn response_encode_error_message() {
        let mut out = Vec::new();
        Response {
            id: 1,
            message: "ENOENT: image not found",
            ..Response::default()
        }
        .encode(&mut out);
        assert_eq!(out, b"\x1b_Gi=1;ENOENT: image not found\x1b\\");
    }

    #[test]
    fn delete_range_commands() {
        let cmd = parse("a=d,d=r,x=3,y=4").unwrap();
        assert_eq!(
            cmd.control,
            Control::Delete(Delete::Range {
                delete: false,
                first: 3,
                last: 4
            })
        );
        let cmd = parse("a=d,d=R,x=5,y=11").unwrap();
        assert_eq!(
            cmd.control,
            Control::Delete(Delete::Range {
                delete: true,
                first: 5,
                last: 11
            })
        );
        assert_eq!(parse("a=d,d=R,x=5,y=4"), Err(ParseError::InvalidFormat));
        assert_eq!(parse("a=d,d=R,x=5"), Err(ParseError::InvalidFormat));
        assert_eq!(parse("a=d,d=R,y=5"), Err(ParseError::InvalidFormat));
    }

    #[test]
    fn animation_commands_parse() {
        let cmd = parse("a=f,c=1,r=2,z=50,X=1,Y=67305985").unwrap();
        assert!(matches!(cmd.control, Control::TransmitAnimationFrame(_)));
        if let Control::TransmitAnimationFrame(f) = cmd.control {
            assert_eq!(f.composition_mode, CompositionMode::Overwrite);
            // 67305985 = 0x04030201: packed r=1,g=2,b=3,a=4 little-endian.
            assert_eq!(f.background, [1, 2, 3, 4]);
        }
        let cmd = parse("a=a,s=3,r=1,z=30,c=0,v=5").unwrap();
        assert!(matches!(cmd.control, Control::ControlAnimation(_)));
        let cmd = parse("a=c,c=1,r=0,x=0,y=0,w=4,h=4,X=0,Y=0,C=1").unwrap();
        assert!(matches!(cmd.control, Control::ComposeAnimation(_)));
        assert_eq!(parse("a=x"), Err(ParseError::InvalidFormat));
    }

    #[test]
    fn compression_and_medium_errors() {
        assert_eq!(parse("a=t,o=q"), Err(ParseError::InvalidFormat));
        assert_eq!(parse("a=t,t=q"), Err(ParseError::InvalidFormat));
        assert_eq!(parse("a=t,f=999"), Err(ParseError::InvalidFormat));
        // A valid but non-direct medium ignores `m` rather than rejecting it.
        let cmd = parse("a=t,m=1,t=f").unwrap();
        let t = cmd.transmission().unwrap();
        assert_eq!(t.medium, M::File);
        assert!(!t.more_chunks);
    }

    #[test]
    fn signed_z_is_bitcast_not_ascii() {
        // `z` values parse as signed i32, so a single non-digit char is NOT
        // stored as its ASCII code for these keys (oracle finishValue).
        let cmd = parse("a=p,i=1,z=q").unwrap();
        assert_eq!(cmd.display().unwrap().z, 113); // ASCII 'q' as i32 value
    }

    #[test]
    fn default_transmission_medium_direct_format_rgba() {
        let cmd = parse(";////").unwrap();
        let t = cmd.transmission().unwrap();
        assert_eq!(t.medium, M::Direct);
        assert_eq!(t.format, F::Rgba);
        assert!(matches!(cmd.quiet, Quiet::No));
    }
}
