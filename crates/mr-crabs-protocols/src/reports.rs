//! Device and status report encoders, ported from Ghostty
//! `src/terminal/device_attributes.zig`, `src/terminal/device_status.zig`,
//! `src/terminal/modes.zig` (`Report`), `src/terminal/size_report.zig`, and
//! the XTVERSION/kitty-keyboard reply helpers in `stream_terminal.zig`.
//!
//! All encoders write into a caller-provided buffer and report their length,
//! so replies can be sent to the PTY without allocation.

use std::io::Write;

/// Device attribute request types (CSI c / CSI > c / CSI = c).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceAttributeReq {
    Primary,
    Secondary,
    Tertiary,
}

/// Response data for all device attribute queries (Ghostty `Attributes`).
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct DeviceAttributes {
    pub primary: Primary,
    pub secondary: Secondary,
    pub tertiary: Tertiary,
}

impl DeviceAttributes {
    /// Encode the response for the given request type into `out`.
    pub fn encode(&self, req: DeviceAttributeReq, out: &mut Vec<u8>) {
        match req {
            DeviceAttributeReq::Primary => self.primary.encode(out),
            DeviceAttributeReq::Secondary => self.secondary.encode(out),
            DeviceAttributeReq::Tertiary => self.tertiary.encode(out),
        }
    }
}

/// Primary device attributes (DA1): `CSI ? Pp ; Ps... c`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Primary {
    pub conformance_level: u16,
    pub features: Vec<u16>,
}

impl Default for Primary {
    fn default() -> Self {
        Self {
            conformance_level: 62, // VT220 level 2
            features: vec![22],    // ansi_color
        }
    }
}

impl Primary {
    /// DA1 feature attribute codes (Ghostty `Feature`).
    pub const COLUMNS_132: u16 = 1;
    pub const PRINTER: u16 = 2;
    pub const REGIS: u16 = 3;
    pub const SIXEL: u16 = 4;
    pub const SELECTIVE_ERASE: u16 = 6;
    pub const USER_DEFINED_KEYS: u16 = 8;
    pub const NATIONAL_REPLACEMENT: u16 = 9;
    pub const TECHNICAL_CHARACTERS: u16 = 15;
    pub const LOCATOR: u16 = 16;
    pub const TERMINAL_STATE: u16 = 17;
    pub const WINDOWING: u16 = 18;
    pub const HORIZONTAL_SCROLLING: u16 = 21;
    pub const ANSI_COLOR: u16 = 22;
    pub const RECTANGULAR_EDITING: u16 = 28;
    pub const ANSI_TEXT_LOCATOR: u16 = 29;
    pub const CLIPBOARD: u16 = 52;

    pub fn encode(&self, out: &mut Vec<u8>) {
        let _ = write!(out, "\x1b[?{}", self.conformance_level);
        for feature in &self.features {
            let _ = write!(out, ";{feature}");
        }
        out.push(b'c');
    }
}

/// Secondary device attributes (DA2): `CSI > Pp ; Pv ; Pc c`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Secondary {
    pub device_type: u16,
    pub firmware_version: u16,
    pub rom_cartridge: u16,
}

impl Default for Secondary {
    fn default() -> Self {
        Self {
            device_type: 1, // VT220
            firmware_version: 0,
            rom_cartridge: 0,
        }
    }
}

impl Secondary {
    pub fn encode(&self, out: &mut Vec<u8>) {
        let _ = write!(
            out,
            "\x1b[>{};{};{}c",
            self.device_type, self.firmware_version, self.rom_cartridge
        );
    }
}

/// Tertiary device attributes (DA3): `DCS ! | D...D ST` (DECRPTUI).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub struct Tertiary {
    pub unit_id: u32,
}

impl Tertiary {
    pub fn encode(&self, out: &mut Vec<u8>) {
        let _ = write!(out, "\x1bP!|{:08X}\x1b\\", self.unit_id);
    }
}

/// Device status requests (DSR).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceStatusReq {
    /// CSI 5 n
    OperatingStatus,
    /// CSI 6 n
    CursorPosition,
    /// CSI ? 996 n
    ColorScheme,
    /// CSI ? 998 n
    Visibility,
}

/// Color scheme reported in response to `CSI ? 996 n`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorScheme {
    Light,
    Dark,
}

impl ColorScheme {
    /// `CSI ? 997 ; n` with 1 = dark, 2 = light.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(match self {
            Self::Dark => b"\x1b[?997;1n",
            Self::Light => b"\x1b[?997;2n",
        });
    }
}

/// Visibility state reported in response to `CSI ? 998 n`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Visibility {
    PotentiallyVisible,
    NotVisible,
}

impl Visibility {
    /// `CSI ? 999 ; n` with 1 = potentially visible, 2 = not visible.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(match self {
            Self::PotentiallyVisible => b"\x1b[?999;1n",
            Self::NotVisible => b"\x1b[?999;2n",
        });
    }
}

/// The state of a mode as reported in a DECRPM response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModeState {
    NotRecognized = 0,
    Set = 1,
    Reset = 2,
    PermanentlySet = 3,
    PermanentlyReset = 4,
}

/// A DECRPM mode report response: `CSI [?]mode;state$y`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModeReport {
    pub mode: u16,
    pub ansi: bool,
    pub state: ModeState,
}

impl ModeReport {
    /// Encode the DECRPM report sequence.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let prefix = if self.ansi { "" } else { "?" };
        let _ = write!(out, "\x1b[{prefix}{};{}$y", self.mode, self.state as u8);
    }
}

/// Size report styles (`size_report.Style`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SizeReportStyle {
    /// In-band size reports (mode 2048)
    Mode2048,
    /// XTWINOPS: report text area size in pixels
    Csi14T,
    /// XTWINOPS: report cell size in pixels
    Csi16T,
    /// XTWINOPS: report text area size in characters
    Csi18T,
    /// CSI 21 t: report the window title
    Csi21T,
}

/// Runtime size values used to encode terminal size reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Size {
    pub rows: u16,
    pub columns: u16,
    pub cell_width: u32,
    pub cell_height: u32,
}

fn width_pixels(s: Size) -> u64 {
    u64::from(s.columns) * u64::from(s.cell_width)
}

fn height_pixels(s: Size) -> u64 {
    u64::from(s.rows) * u64::from(s.cell_height)
}

/// Encode a terminal size report sequence (Ghostty `size_report.encode`).
pub fn encode_size_report(out: &mut Vec<u8>, style: SizeReportStyle, size: Size) {
    match style {
        SizeReportStyle::Mode2048 => {
            let _ = write!(
                out,
                "\x1b[48;{};{};{};{}t",
                size.rows,
                size.columns,
                height_pixels(size),
                width_pixels(size)
            );
        }
        SizeReportStyle::Csi14T => {
            let _ = write!(
                out,
                "\x1b[4;{};{}t",
                height_pixels(size),
                width_pixels(size)
            );
        }
        SizeReportStyle::Csi16T => {
            let _ = write!(out, "\x1b[6;{};{}t", size.cell_height, size.cell_width);
        }
        SizeReportStyle::Csi18T => {
            let _ = write!(out, "\x1b[8;{};{}t", size.rows, size.columns);
        }
        SizeReportStyle::Csi21T => {}
    }
}

/// Encode an XTVERSION reply: `DCS > | version ST` (Ghostty
/// `reportXtversion`; the default when no version is configured is
/// `libghostty`).
pub fn encode_xtversion(version: &str, out: &mut Vec<u8>) {
    let version = if version.is_empty() {
        "libghostty"
    } else {
        version
    };
    let _ = write!(out, "\x1bP>|{version}\x1b\\");
}

/// Encode a kitty keyboard protocol query reply: `CSI ? flags u`
/// (Ghostty `queryKittyKeyboard`).
pub fn encode_kitty_keyboard_flags(flags: u8, out: &mut Vec<u8>) {
    let _ = write!(out, "\x1b[?{flags}u");
}

/// Encode an enquiry (ENQ) response. The caller supplies the response
/// string; empty responses are suppressed (Ghostty `reportEnquiry`).
pub fn encode_enquiry(response: &str, out: &mut Vec<u8>) {
    if !response.is_empty() {
        out.extend_from_slice(response.as_bytes());
    }
}

/// Encode an XTGETTCAP `TN` reply for the configured terminfo name
/// (Ghostty `writeTerminfoName`). Names over the limit are not answered.
pub fn encode_terminfo_name(name: &str, out: &mut Vec<u8>) {
    if name.is_empty() || name.len() > crate::limits::MAX_TERMINFO_NAME_BYTES {
        return;
    }
    let _ = write!(out, "\x1bP1+r544E={}\x1b\\", hex_upper(name.as_bytes()));
}

fn hex_upper(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = std::fmt::write(&mut s, format_args!("{b:02X}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_default() {
        let mut out = Vec::new();
        DeviceAttributes::default().encode(DeviceAttributeReq::Primary, &mut out);
        assert_eq!(out, b"\x1b[?62;22c");
    }

    #[test]
    fn primary_with_clipboard() {
        let mut out = Vec::new();
        Primary {
            conformance_level: 62,
            features: vec![Primary::ANSI_COLOR, Primary::CLIPBOARD],
        }
        .encode(&mut out);
        assert_eq!(out, b"\x1b[?62;22;52c");
    }

    #[test]
    fn secondary_default() {
        let mut out = Vec::new();
        DeviceAttributes::default().encode(DeviceAttributeReq::Secondary, &mut out);
        assert_eq!(out, b"\x1b[>1;0;0c");
    }

    #[test]
    fn tertiary_default_and_custom() {
        let mut out = Vec::new();
        DeviceAttributes::default().encode(DeviceAttributeReq::Tertiary, &mut out);
        assert_eq!(out, b"\x1bP!|00000000\x1b\\");
        let mut out = Vec::new();
        Tertiary {
            unit_id: 0xAABBCCDD,
        }
        .encode(&mut out);
        assert_eq!(out, b"\x1bP!|AABBCCDD\x1b\\");
    }

    #[test]
    fn color_scheme_and_visibility() {
        let mut out = Vec::new();
        ColorScheme::Dark.encode(&mut out);
        assert_eq!(out, b"\x1b[?997;1n");
        let mut out = Vec::new();
        ColorScheme::Light.encode(&mut out);
        assert_eq!(out, b"\x1b[?997;2n");
        let mut out = Vec::new();
        Visibility::PotentiallyVisible.encode(&mut out);
        assert_eq!(out, b"\x1b[?999;1n");
    }

    #[test]
    fn mode_report() {
        let mut out = Vec::new();
        ModeReport {
            mode: 7,
            ansi: false,
            state: ModeState::Set,
        }
        .encode(&mut out);
        assert_eq!(out, b"\x1b[?7;1$y");
        let mut out = Vec::new();
        ModeReport {
            mode: 25,
            ansi: true,
            state: ModeState::Reset,
        }
        .encode(&mut out);
        assert_eq!(out, b"\x1b[25;2$y");
    }

    #[test]
    fn size_reports() {
        let size = Size {
            rows: 24,
            columns: 80,
            cell_width: 9,
            cell_height: 18,
        };
        let mut out = Vec::new();
        encode_size_report(&mut out, SizeReportStyle::Mode2048, size);
        assert_eq!(out, b"\x1b[48;24;80;432;720t");
        let mut out = Vec::new();
        encode_size_report(&mut out, SizeReportStyle::Csi14T, size);
        assert_eq!(out, b"\x1b[4;432;720t");
        let mut out = Vec::new();
        encode_size_report(&mut out, SizeReportStyle::Csi16T, size);
        assert_eq!(out, b"\x1b[6;18;9t");
        let mut out = Vec::new();
        encode_size_report(&mut out, SizeReportStyle::Csi18T, size);
        assert_eq!(out, b"\x1b[8;24;80t");
    }

    #[test]
    fn xtversion() {
        let mut out = Vec::new();
        encode_xtversion("", &mut out);
        assert_eq!(out, b"\x1bP>|libghostty\x1b\\");
        let mut out = Vec::new();
        encode_xtversion("1.3.2", &mut out);
        assert_eq!(out, b"\x1bP>|1.3.2\x1b\\");
    }

    #[test]
    fn kitty_keyboard_flags() {
        let mut out = Vec::new();
        encode_kitty_keyboard_flags(31, &mut out);
        assert_eq!(out, b"\x1b[?31u");
    }

    #[test]
    fn terminfo_name() {
        let mut out = Vec::new();
        encode_terminfo_name("xterm-ghostty", &mut out);
        // Ghostty stream_terminal.zig:writeTerminfoName uses "{X}" bytesToHex .upper
        // TN=544E (upper) and every payload hex pair is 02X.
        assert_eq!(out, b"\x1bP1+r544E=787465726D2D67686F73747479\x1b\\");
        let mut out = Vec::new();
        encode_terminfo_name("", &mut out);
        assert!(out.is_empty());
    }
}
