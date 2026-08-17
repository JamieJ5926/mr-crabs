//! Terminfo source generation and install-safe paths, ported from Ghostty
//! `src/terminfo/Source.zig`, `src/terminfo/ghostty.zig`, and the XTGETTCAP
//! map logic.
//!
//! The crate never executes a shell. "Installation" writes the generated
//! terminfo source into a terminfo directory and returns the exact `tic`
//! command as a string for the user or package manager to run.

use std::io::Write;
use std::path::{Path, PathBuf};

/// A capability in a terminfo source file (Ghostty `Source.Capability`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Value {
    /// Canceled value (suffixed with `@`).
    Canceled,
    /// Boolean: always true if present.
    Boolean,
    /// Unsigned decimal integer.
    Numeric(u32),
    /// String value in terminfo source form (backslash escapes preserved).
    String(&'static str),
}

/// A single capability entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capability {
    pub name: &'static str,
    pub value: Value,
}

/// A terminfo source entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Source {
    pub names: &'static [&'static str],
    pub capabilities: &'static [Capability],
}

impl Source {
    /// Encode as a terminfo source file (Ghostty `Source.encode`): names in
    /// order joined with `|`, then one `\t{name}{value},` line per
    /// capability in order.
    pub fn encode(&self, out: &mut Vec<u8>) {
        for (i, name) in self.names.iter().enumerate() {
            if i != 0 {
                out.push(b'|');
            }
            out.extend_from_slice(name.as_bytes());
        }
        out.extend_from_slice(b",\n");
        for cap in self.capabilities {
            out.extend_from_slice(b"\t");
            out.extend_from_slice(cap.name.as_bytes());
            match cap.value {
                Value::Canceled => out.push(b'@'),
                Value::Boolean => {}
                Value::Numeric(v) => {
                    let _ = write!(out, "#{v}");
                }
                Value::String(v) => {
                    out.push(b'=');
                    out.extend_from_slice(v.as_bytes());
                }
            }
            out.extend_from_slice(b",\n");
        }
    }

    /// Build the XTGETTCAP reply map (Ghostty `xtgettcapMap`): every
    /// capability plus `TN`, `Co`, and `RGB`, keyed by hex-encoded name and
    /// valued by the full `DCS 1+r name=hexvalue ST` reply.
    pub fn xtgettcap_map(&self) -> std::collections::HashMap<String, Vec<u8>> {
        let mut map = std::collections::HashMap::new();
        map.insert(
            hex_upper(b"TN"),
            xtgettcap_reply(b"TN", self.names[0].as_bytes()),
        );
        map.insert(hex_upper(b"Co"), xtgettcap_reply(b"Co", b"256"));
        map.insert(hex_upper(b"RGB"), xtgettcap_reply(b"RGB", b"8"));
        for cap in self.capabilities {
            let value = match cap.value {
                Value::Canceled => continue,
                Value::Boolean => None,
                Value::Numeric(v) => Some(format!("{v}").into_bytes()),
                Value::String(v) => Some(expand_xtgettcap_string(v)),
            };
            map.insert(
                hex_upper(cap.name.as_bytes()),
                xtgettcap_reply(cap.name.as_bytes(), value.as_deref().unwrap_or(b"")),
            );
        }
        map
    }

    /// Look up a single XTGETTCAP reply by hex-encoded name.
    pub fn xtgettcap_get(&self, hex_name: &[u8]) -> Option<Vec<u8>> {
        self.xtgettcap_map()
            .get(std::str::from_utf8(hex_name).ok()?)
            .cloned()
    }
}

/// Expand a terminfo source string into the XTGETTCAP wire value (Ghostty
/// `xtgettcapMap`): strings with `%` parameters are returned raw; others
/// replace `\E` with ESC and `^X` with the control character.
fn expand_xtgettcap_string(v: &str) -> Vec<u8> {
    if v.contains('%') {
        return v.as_bytes().to_vec();
    }
    let mut result = Vec::with_capacity(v.len());
    let bytes = v.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'E' {
            result.push(0x1b);
            i += 2;
            continue;
        }
        if bytes[i] == b'^' && i + 1 < bytes.len() {
            let c = bytes[i + 1];
            result.push(if c == b'?' { 0x7F } else { c - 64 });
            i += 2;
            continue;
        }
        result.push(bytes[i]);
        i += 1;
    }
    result
}

fn hex_upper(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = std::fmt::write(&mut s, format_args!("{b:02X}"));
    }
    s
}

fn xtgettcap_reply(name: &[u8], value: &[u8]) -> Vec<u8> {
    let name_hex = hex_upper(name);
    if value.is_empty() {
        let mut reply = Vec::with_capacity(5 + name_hex.len() + 2);
        reply.extend_from_slice(b"\x1bP1+r");
        reply.extend_from_slice(name_hex.as_bytes());
        reply.extend_from_slice(b"\x1b\\");
        reply
    } else {
        let mut reply = Vec::with_capacity(5 + name_hex.len() + 1 + value.len() * 2 + 2);
        reply.extend_from_slice(b"\x1bP1+r");
        reply.extend_from_slice(name_hex.as_bytes());
        reply.push(b'=');
        reply.extend_from_slice(hex_upper(value).as_bytes());
        reply.extend_from_slice(b"\x1b\\");
        reply
    }
}

/// Ghostty's terminfo entry (Ghostty `terminfo/ghostty.zig`).
///
/// Names: `xterm-ghostty` first (tcell compatibility), then `ghostty`, then
/// the formal `Ghostty`.
pub const GHOSTTY: Source = Source {
    names: &["xterm-ghostty", "ghostty", "Ghostty"],
    capabilities: &[
        Capability {
            name: "am",
            value: Value::Boolean,
        },
        Capability {
            name: "bce",
            value: Value::Boolean,
        },
        Capability {
            name: "ccc",
            value: Value::Boolean,
        },
        Capability {
            name: "hs",
            value: Value::Boolean,
        },
        Capability {
            name: "km",
            value: Value::Boolean,
        },
        Capability {
            name: "mc5i",
            value: Value::Boolean,
        },
        Capability {
            name: "mir",
            value: Value::Boolean,
        },
        Capability {
            name: "msgr",
            value: Value::Boolean,
        },
        Capability {
            name: "npc",
            value: Value::Boolean,
        },
        Capability {
            name: "xenl",
            value: Value::Boolean,
        },
        Capability {
            name: "AX",
            value: Value::Boolean,
        },
        Capability {
            name: "Tc",
            value: Value::Boolean,
        },
        Capability {
            name: "Su",
            value: Value::Boolean,
        },
        Capability {
            name: "XT",
            value: Value::Boolean,
        },
        Capability {
            name: "fullkbd",
            value: Value::Boolean,
        },
        Capability {
            name: "colors",
            value: Value::Numeric(256),
        },
        Capability {
            name: "cols",
            value: Value::Numeric(80),
        },
        Capability {
            name: "it",
            value: Value::Numeric(8),
        },
        Capability {
            name: "lines",
            value: Value::Numeric(24),
        },
        Capability {
            name: "pairs",
            value: Value::Numeric(32767),
        },
        Capability {
            name: "acsc",
            value: Value::String(
                "++\\,\\,--..00``aaffgghhiijjkkllmmnnooppqqrrssttuuvvwwxxyyzz{{||}}~~",
            ),
        },
        Capability {
            name: "Smulx",
            value: Value::String("\\E[4:%p1%dm"),
        },
        Capability {
            name: "Smol",
            value: Value::String("\\E[53m"),
        },
        Capability {
            name: "Rmol",
            value: Value::String("\\E[55m"),
        },
        Capability {
            name: "Setulc",
            value: Value::String(
                "\\E[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m",
            ),
        },
        Capability {
            name: "Ss",
            value: Value::String("\\E[%p1%d q"),
        },
        Capability {
            name: "Se",
            value: Value::String("\\E[0 q"),
        },
        Capability {
            name: "Ms",
            value: Value::String("\\E]52;%p1%s;%p2%s\\007"),
        },
        Capability {
            name: "Sync",
            value: Value::String("\\E[?2026%?%p1%{1}%-%tl%eh%;"),
        },
        Capability {
            name: "BD",
            value: Value::String("\\E[?2004l"),
        },
        Capability {
            name: "BE",
            value: Value::String("\\E[?2004h"),
        },
        Capability {
            name: "PS",
            value: Value::String("\\E[200~"),
        },
        Capability {
            name: "PE",
            value: Value::String("\\E[201~"),
        },
        Capability {
            name: "XM",
            value: Value::String("\\E[?1006;1000%?%p1%{1}%=%th%el%;"),
        },
        Capability {
            name: "xm",
            value: Value::String("\\E[<%i%p3%d;%p1%d;%p2%d;%?%p4%tM%em%;"),
        },
        Capability {
            name: "RV",
            value: Value::String("\\E[>c"),
        },
        Capability {
            name: "rv",
            value: Value::String("\\E\\\\[[0-9]+;[0-9]+;[0-9]+c"),
        },
        Capability {
            name: "XR",
            value: Value::String("\\E[>0q"),
        },
        Capability {
            name: "xr",
            value: Value::String("\\EP>\\\\|[ -~]+a\\E\\\\"),
        },
        Capability {
            name: "Enmg",
            value: Value::String("\\E[?69h"),
        },
        Capability {
            name: "Dsmg",
            value: Value::String("\\E[?69l"),
        },
        Capability {
            name: "Clmg",
            value: Value::String("\\E[s"),
        },
        Capability {
            name: "Cmg",
            value: Value::String("\\E[%i%p1%d;%p2%ds"),
        },
        Capability {
            name: "clear",
            value: Value::String("\\E[H\\E[2J"),
        },
        Capability {
            name: "E3",
            value: Value::String("\\E[3J"),
        },
        Capability {
            name: "fe",
            value: Value::String("\\E[?1004h"),
        },
        Capability {
            name: "fd",
            value: Value::String("\\E[?1004l"),
        },
        Capability {
            name: "kxIN",
            value: Value::String("\\E[I"),
        },
        Capability {
            name: "kxOUT",
            value: Value::String("\\E[O"),
        },
        Capability {
            name: "bel",
            value: Value::String("^G"),
        },
        Capability {
            name: "blink",
            value: Value::String("\\E[5m"),
        },
        Capability {
            name: "bold",
            value: Value::String("\\E[1m"),
        },
        Capability {
            name: "cbt",
            value: Value::String("\\E[Z"),
        },
        Capability {
            name: "civis",
            value: Value::String("\\E[?25l"),
        },
        Capability {
            name: "cnorm",
            value: Value::String("\\E[?12l\\E[?25h"),
        },
        Capability {
            name: "cr",
            value: Value::String("\\r"),
        },
        Capability {
            name: "csr",
            value: Value::String("\\E[%i%p1%d;%p2%dr"),
        },
        Capability {
            name: "cub",
            value: Value::String("\\E[%p1%dD"),
        },
        Capability {
            name: "cub1",
            value: Value::String("^H"),
        },
        Capability {
            name: "cud",
            value: Value::String("\\E[%p1%dB"),
        },
        Capability {
            name: "cud1",
            value: Value::String("^J"),
        },
        Capability {
            name: "cuf",
            value: Value::String("\\E[%p1%dC"),
        },
        Capability {
            name: "cuf1",
            value: Value::String("\\E[C"),
        },
        Capability {
            name: "cup",
            value: Value::String("\\E[%i%p1%d;%p2%dH"),
        },
        Capability {
            name: "cuu",
            value: Value::String("\\E[%p1%dA"),
        },
        Capability {
            name: "cuu1",
            value: Value::String("\\E[A"),
        },
        Capability {
            name: "cvvis",
            value: Value::String("\\E[?12;25h"),
        },
        Capability {
            name: "dch",
            value: Value::String("\\E[%p1%dP"),
        },
        Capability {
            name: "dch1",
            value: Value::String("\\E[P"),
        },
        Capability {
            name: "dim",
            value: Value::String("\\E[2m"),
        },
        Capability {
            name: "dl",
            value: Value::String("\\E[%p1%dM"),
        },
        Capability {
            name: "dl1",
            value: Value::String("\\E[M"),
        },
        Capability {
            name: "dsl",
            value: Value::String("\\E]2;\\007"),
        },
        Capability {
            name: "ech",
            value: Value::String("\\E[%p1%dX"),
        },
        Capability {
            name: "ed",
            value: Value::String("\\E[J"),
        },
        Capability {
            name: "el",
            value: Value::String("\\E[K"),
        },
        Capability {
            name: "el1",
            value: Value::String("\\E[1K"),
        },
        Capability {
            name: "flash",
            value: Value::String("\\E[?5h$<100/>\\E[?5l"),
        },
        Capability {
            name: "fsl",
            value: Value::String("^G"),
        },
        Capability {
            name: "home",
            value: Value::String("\\E[H"),
        },
        Capability {
            name: "hpa",
            value: Value::String("\\E[%i%p1%dG"),
        },
        Capability {
            name: "ht",
            value: Value::String("^I"),
        },
        Capability {
            name: "hts",
            value: Value::String("\\EH"),
        },
        Capability {
            name: "ich",
            value: Value::String("\\E[%p1%d@"),
        },
        Capability {
            name: "ich1",
            value: Value::String("\\E[@"),
        },
        Capability {
            name: "il",
            value: Value::String("\\E[%p1%dL"),
        },
        Capability {
            name: "il1",
            value: Value::String("\\E[L"),
        },
        Capability {
            name: "ind",
            value: Value::String("\\n"),
        },
        Capability {
            name: "indn",
            value: Value::String("\\E[%p1%dS"),
        },
        Capability {
            name: "initc",
            value: Value::String(
                "\\E]4;%p1%d;rgb\\:%p2%{255}%*%{1000}%/%2.2X/%p3%{255}%*%{1000}%/%2.2X/%p4%{255}%*%{1000}%/%2.2X\\E\\\\",
            ),
        },
        Capability {
            name: "invis",
            value: Value::String("\\E[8m"),
        },
        Capability {
            name: "oc",
            value: Value::String("\\E]104\\007"),
        },
        Capability {
            name: "op",
            value: Value::String("\\E[39;49m"),
        },
        Capability {
            name: "rc",
            value: Value::String("\\E8"),
        },
        Capability {
            name: "rep",
            value: Value::String("%p1%c\\E[%p2%{1}%-%db"),
        },
        Capability {
            name: "rev",
            value: Value::String("\\E[7m"),
        },
        Capability {
            name: "ri",
            value: Value::String("\\EM"),
        },
        Capability {
            name: "rin",
            value: Value::String("\\E[%p1%dT"),
        },
        Capability {
            name: "ritm",
            value: Value::String("\\E[23m"),
        },
        Capability {
            name: "rmacs",
            value: Value::String("\\E(B"),
        },
        Capability {
            name: "rmam",
            value: Value::String("\\E[?7l"),
        },
        Capability {
            name: "rmcup",
            value: Value::String("\\E[?1049l"),
        },
        Capability {
            name: "rmir",
            value: Value::String("\\E[4l"),
        },
        Capability {
            name: "rmkx",
            value: Value::String("\\E[?1l\\E>"),
        },
        Capability {
            name: "rmso",
            value: Value::String("\\E[27m"),
        },
        Capability {
            name: "rmul",
            value: Value::String("\\E[24m"),
        },
        Capability {
            name: "rmxx",
            value: Value::String("\\E[29m"),
        },
        Capability {
            name: "setab",
            value: Value::String(
                "\\E[%?%p1%{8}%<%t4%p1%d%e%p1%{16}%<%t10%p1%{8}%-%d%e48;5;%p1%d%;m",
            ),
        },
        Capability {
            name: "setaf",
            value: Value::String(
                "\\E[%?%p1%{8}%<%t3%p1%d%e%p1%{16}%<%t9%p1%{8}%-%d%e38;5;%p1%d%;m",
            ),
        },
        Capability {
            name: "setrgbb",
            value: Value::String("\\E[48:2:%p1%d:%p2%d:%p3%dm"),
        },
        Capability {
            name: "setrgbf",
            value: Value::String("\\E[38:2:%p1%d:%p2%d:%p3%dm"),
        },
        Capability {
            name: "sgr",
            value: Value::String(
                "%?%p9%t\\E(0%e\\E(B%;\\E[0%?%p6%t;1%;%?%p5%t;2%;%?%p2%t;4%;%?%p1%p3%|%t;7%;%?%p4%t;5%;%?%p7%t;8%;m",
            ),
        },
        Capability {
            name: "sgr0",
            value: Value::String("\\E(B\\E[m"),
        },
        Capability {
            name: "sitm",
            value: Value::String("\\E[3m"),
        },
        Capability {
            name: "smacs",
            value: Value::String("\\E(0"),
        },
        Capability {
            name: "smam",
            value: Value::String("\\E[?7h"),
        },
        Capability {
            name: "smcup",
            value: Value::String("\\E[?1049h"),
        },
        Capability {
            name: "smir",
            value: Value::String("\\E[4h"),
        },
        Capability {
            name: "smkx",
            value: Value::String("\\E[?1h\\E="),
        },
        Capability {
            name: "smso",
            value: Value::String("\\E[7m"),
        },
        Capability {
            name: "smul",
            value: Value::String("\\E[4m"),
        },
        Capability {
            name: "smxx",
            value: Value::String("\\E[9m"),
        },
        Capability {
            name: "tbc",
            value: Value::String("\\E[3g"),
        },
        Capability {
            name: "tsl",
            value: Value::String("\\E]2;"),
        },
        Capability {
            name: "u6",
            value: Value::String("\\E[%i%d;%dR"),
        },
        Capability {
            name: "u7",
            value: Value::String("\\E[6n"),
        },
        Capability {
            name: "u8",
            value: Value::String("\\E[?%[;0123456789]c"),
        },
        Capability {
            name: "u9",
            value: Value::String("\\E[c"),
        },
        Capability {
            name: "vpa",
            value: Value::String("\\E[%i%p1%dd"),
        },
        // Function-key families with modifier variants.
        Capability {
            name: "kDC",
            value: Value::String("\\E[3;2~"),
        },
        Capability {
            name: "kDC3",
            value: Value::String("\\E[3;3~"),
        },
        Capability {
            name: "kDC4",
            value: Value::String("\\E[3;4~"),
        },
        Capability {
            name: "kDC5",
            value: Value::String("\\E[3;5~"),
        },
        Capability {
            name: "kDC6",
            value: Value::String("\\E[3;6~"),
        },
        Capability {
            name: "kDC7",
            value: Value::String("\\E[3;7~"),
        },
        Capability {
            name: "kDN",
            value: Value::String("\\E[1;2B"),
        },
        Capability {
            name: "kDN3",
            value: Value::String("\\E[1;3B"),
        },
        Capability {
            name: "kDN4",
            value: Value::String("\\E[1;4B"),
        },
        Capability {
            name: "kDN5",
            value: Value::String("\\E[1;5B"),
        },
        Capability {
            name: "kDN6",
            value: Value::String("\\E[1;6B"),
        },
        Capability {
            name: "kDN7",
            value: Value::String("\\E[1;7B"),
        },
        Capability {
            name: "kEND",
            value: Value::String("\\E[1;2F"),
        },
        Capability {
            name: "kEND3",
            value: Value::String("\\E[1;3F"),
        },
        Capability {
            name: "kEND4",
            value: Value::String("\\E[1;4F"),
        },
        Capability {
            name: "kEND5",
            value: Value::String("\\E[1;5F"),
        },
        Capability {
            name: "kEND6",
            value: Value::String("\\E[1;6F"),
        },
        Capability {
            name: "kEND7",
            value: Value::String("\\E[1;7F"),
        },
        Capability {
            name: "kHOM",
            value: Value::String("\\E[1;2H"),
        },
        Capability {
            name: "kHOM3",
            value: Value::String("\\E[1;3H"),
        },
        Capability {
            name: "kHOM4",
            value: Value::String("\\E[1;4H"),
        },
        Capability {
            name: "kHOM5",
            value: Value::String("\\E[1;5H"),
        },
        Capability {
            name: "kHOM6",
            value: Value::String("\\E[1;6H"),
        },
        Capability {
            name: "kHOM7",
            value: Value::String("\\E[1;7H"),
        },
        Capability {
            name: "kIC",
            value: Value::String("\\E[2;2~"),
        },
        Capability {
            name: "kIC3",
            value: Value::String("\\E[2;3~"),
        },
        Capability {
            name: "kIC4",
            value: Value::String("\\E[2;4~"),
        },
        Capability {
            name: "kIC5",
            value: Value::String("\\E[2;5~"),
        },
        Capability {
            name: "kIC6",
            value: Value::String("\\E[2;6~"),
        },
        Capability {
            name: "kIC7",
            value: Value::String("\\E[2;7~"),
        },
        Capability {
            name: "kLFT",
            value: Value::String("\\E[1;2D"),
        },
        Capability {
            name: "kLFT3",
            value: Value::String("\\E[1;3D"),
        },
        Capability {
            name: "kLFT4",
            value: Value::String("\\E[1;4D"),
        },
        Capability {
            name: "kLFT5",
            value: Value::String("\\E[1;5D"),
        },
        Capability {
            name: "kLFT6",
            value: Value::String("\\E[1;6D"),
        },
        Capability {
            name: "kLFT7",
            value: Value::String("\\E[1;7D"),
        },
        Capability {
            name: "kNXT",
            value: Value::String("\\E[6;2~"),
        },
        Capability {
            name: "kNXT3",
            value: Value::String("\\E[6;3~"),
        },
        Capability {
            name: "kNXT4",
            value: Value::String("\\E[6;4~"),
        },
        Capability {
            name: "kNXT5",
            value: Value::String("\\E[6;5~"),
        },
        Capability {
            name: "kNXT6",
            value: Value::String("\\E[6;6~"),
        },
        Capability {
            name: "kNXT7",
            value: Value::String("\\E[6;7~"),
        },
        Capability {
            name: "kPRV",
            value: Value::String("\\E[5;2~"),
        },
        Capability {
            name: "kPRV3",
            value: Value::String("\\E[5;3~"),
        },
        Capability {
            name: "kPRV4",
            value: Value::String("\\E[5;4~"),
        },
        Capability {
            name: "kPRV5",
            value: Value::String("\\E[5;5~"),
        },
        Capability {
            name: "kPRV6",
            value: Value::String("\\E[5;6~"),
        },
        Capability {
            name: "kPRV7",
            value: Value::String("\\E[5;7~"),
        },
        Capability {
            name: "kRIT",
            value: Value::String("\\E[1;2C"),
        },
        Capability {
            name: "kRIT3",
            value: Value::String("\\E[1;3C"),
        },
        Capability {
            name: "kRIT4",
            value: Value::String("\\E[1;4C"),
        },
        Capability {
            name: "kRIT5",
            value: Value::String("\\E[1;5C"),
        },
        Capability {
            name: "kRIT6",
            value: Value::String("\\E[1;6C"),
        },
        Capability {
            name: "kRIT7",
            value: Value::String("\\E[1;7C"),
        },
        Capability {
            name: "kUP",
            value: Value::String("\\E[1;2A"),
        },
        Capability {
            name: "kUP3",
            value: Value::String("\\E[1;3A"),
        },
        Capability {
            name: "kUP4",
            value: Value::String("\\E[1;4A"),
        },
        Capability {
            name: "kUP5",
            value: Value::String("\\E[1;5A"),
        },
        Capability {
            name: "kUP6",
            value: Value::String("\\E[1;6A"),
        },
        Capability {
            name: "kUP7",
            value: Value::String("\\E[1;7A"),
        },
        Capability {
            name: "kf1",
            value: Value::String("\\EOP"),
        },
        Capability {
            name: "kf2",
            value: Value::String("\\EOQ"),
        },
        Capability {
            name: "kf3",
            value: Value::String("\\EOR"),
        },
        Capability {
            name: "kf4",
            value: Value::String("\\EOS"),
        },
        Capability {
            name: "kf5",
            value: Value::String("\\E[15~"),
        },
        Capability {
            name: "kf6",
            value: Value::String("\\E[17~"),
        },
        Capability {
            name: "kf7",
            value: Value::String("\\E[18~"),
        },
        Capability {
            name: "kf8",
            value: Value::String("\\E[19~"),
        },
        Capability {
            name: "kf9",
            value: Value::String("\\E[20~"),
        },
        Capability {
            name: "kf10",
            value: Value::String("\\E[21~"),
        },
        Capability {
            name: "kf11",
            value: Value::String("\\E[23~"),
        },
        Capability {
            name: "kf12",
            value: Value::String("\\E[24~"),
        },
        Capability {
            name: "kf13",
            value: Value::String("\\E[1;2P"),
        },
        Capability {
            name: "kf14",
            value: Value::String("\\E[1;2Q"),
        },
        Capability {
            name: "kf15",
            value: Value::String("\\E[1;2R"),
        },
        Capability {
            name: "kf16",
            value: Value::String("\\E[1;2S"),
        },
        Capability {
            name: "kf17",
            value: Value::String("\\E[15;2~"),
        },
        Capability {
            name: "kf18",
            value: Value::String("\\E[17;2~"),
        },
        Capability {
            name: "kf19",
            value: Value::String("\\E[18;2~"),
        },
        Capability {
            name: "kf20",
            value: Value::String("\\E[19;2~"),
        },
        Capability {
            name: "kf21",
            value: Value::String("\\E[20;2~"),
        },
        Capability {
            name: "kf22",
            value: Value::String("\\E[21;2~"),
        },
        Capability {
            name: "kf23",
            value: Value::String("\\E[23;2~"),
        },
        Capability {
            name: "kf24",
            value: Value::String("\\E[24;2~"),
        },
        Capability {
            name: "kf25",
            value: Value::String("\\E[1;5P"),
        },
        Capability {
            name: "kf26",
            value: Value::String("\\E[1;5Q"),
        },
        Capability {
            name: "kf27",
            value: Value::String("\\E[1;5R"),
        },
        Capability {
            name: "kf28",
            value: Value::String("\\E[1;5S"),
        },
        Capability {
            name: "kf29",
            value: Value::String("\\E[15;5~"),
        },
        Capability {
            name: "kf30",
            value: Value::String("\\E[17;5~"),
        },
        Capability {
            name: "kf31",
            value: Value::String("\\E[18;5~"),
        },
        Capability {
            name: "kf32",
            value: Value::String("\\E[19;5~"),
        },
        Capability {
            name: "kf33",
            value: Value::String("\\E[20;5~"),
        },
        Capability {
            name: "kf34",
            value: Value::String("\\E[21;5~"),
        },
        Capability {
            name: "kf35",
            value: Value::String("\\E[23;5~"),
        },
        Capability {
            name: "kf36",
            value: Value::String("\\E[24;5~"),
        },
        Capability {
            name: "kf37",
            value: Value::String("\\E[1;6P"),
        },
        Capability {
            name: "kf38",
            value: Value::String("\\E[1;6Q"),
        },
        Capability {
            name: "kf39",
            value: Value::String("\\E[1;6R"),
        },
        Capability {
            name: "kf40",
            value: Value::String("\\E[1;6S"),
        },
        Capability {
            name: "kf41",
            value: Value::String("\\E[15;6~"),
        },
        Capability {
            name: "kf42",
            value: Value::String("\\E[17;6~"),
        },
        Capability {
            name: "kf43",
            value: Value::String("\\E[18;6~"),
        },
        Capability {
            name: "kf44",
            value: Value::String("\\E[19;6~"),
        },
        Capability {
            name: "kf45",
            value: Value::String("\\E[20;6~"),
        },
        Capability {
            name: "kf46",
            value: Value::String("\\E[21;6~"),
        },
        Capability {
            name: "kf47",
            value: Value::String("\\E[23;6~"),
        },
        Capability {
            name: "kf48",
            value: Value::String("\\E[24;6~"),
        },
        Capability {
            name: "kf49",
            value: Value::String("\\E[1;3P"),
        },
        Capability {
            name: "kf50",
            value: Value::String("\\E[1;3Q"),
        },
        Capability {
            name: "kf51",
            value: Value::String("\\E[1;3R"),
        },
        Capability {
            name: "kf52",
            value: Value::String("\\E[1;3S"),
        },
        Capability {
            name: "kf53",
            value: Value::String("\\E[15;3~"),
        },
        Capability {
            name: "kf54",
            value: Value::String("\\E[17;3~"),
        },
        Capability {
            name: "kf55",
            value: Value::String("\\E[18;3~"),
        },
        Capability {
            name: "kf56",
            value: Value::String("\\E[19;3~"),
        },
        Capability {
            name: "kf57",
            value: Value::String("\\E[20;3~"),
        },
        Capability {
            name: "kf58",
            value: Value::String("\\E[21;3~"),
        },
        Capability {
            name: "kf59",
            value: Value::String("\\E[23;3~"),
        },
        Capability {
            name: "kf60",
            value: Value::String("\\E[24;3~"),
        },
        Capability {
            name: "kf61",
            value: Value::String("\\E[1;4P"),
        },
        Capability {
            name: "kf62",
            value: Value::String("\\E[1;4Q"),
        },
        Capability {
            name: "kf63",
            value: Value::String("\\E[1;4R"),
        },
        Capability {
            name: "kbs",
            value: Value::String("^?"),
        },
        Capability {
            name: "kcbt",
            value: Value::String("\\E[Z"),
        },
        Capability {
            name: "kcub1",
            value: Value::String("\\EOD"),
        },
        Capability {
            name: "kcud1",
            value: Value::String("\\EOB"),
        },
        Capability {
            name: "kcuf1",
            value: Value::String("\\EOC"),
        },
        Capability {
            name: "kcuu1",
            value: Value::String("\\EOA"),
        },
        Capability {
            name: "kdch1",
            value: Value::String("\\E[3~"),
        },
        Capability {
            name: "kend",
            value: Value::String("\\EOF"),
        },
        Capability {
            name: "kent",
            value: Value::String("\\EOM"),
        },
        Capability {
            name: "khome",
            value: Value::String("\\EOH"),
        },
        Capability {
            name: "kich1",
            value: Value::String("\\E[2~"),
        },
        Capability {
            name: "kind",
            value: Value::String("\\E[1;2B"),
        },
        Capability {
            name: "kmous",
            value: Value::String("\\E[<"),
        },
        Capability {
            name: "knp",
            value: Value::String("\\E[6~"),
        },
        Capability {
            name: "kpp",
            value: Value::String("\\E[5~"),
        },
        Capability {
            name: "kri",
            value: Value::String("\\E[1;2A"),
        },
        Capability {
            name: "rs1",
            value: Value::String("\\E]\\E\\\\\\Ec"),
        },
        Capability {
            name: "sc",
            value: Value::String("\\E7"),
        },
    ],
};

/// Candidate terminfo directories in priority order (never executed).
pub fn install_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(terminfo) = std::env::var("TERMINFO") {
        if !terminfo.is_empty() {
            paths.push(PathBuf::from(terminfo));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(&home).join(".terminfo"));
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            paths.push(PathBuf::from(xdg).join("terminfo"));
        }
    } else if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(&home).join(".local/share/terminfo"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(&home).join("Library/terminfo"));
    }
    paths.push(PathBuf::from("/usr/local/share/terminfo"));
    paths
}

/// Write the generated terminfo source into `dir/ghostty.src`, creating
/// `dir` if needed. Returns the written path.
///
/// No shell command is executed; use [`compile_command`] to obtain the exact
/// `tic` invocation for the user or a package manager to run.
pub fn install_source_to(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let mut bytes = Vec::new();
    GHOSTTY.encode(&mut bytes);
    let path = dir.join("ghostty.src");
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// The exact `tic` command that compiles the generated source into `dir`
/// (returned as a string; never executed by this crate).
pub fn compile_command(dir: &Path) -> String {
    format!(
        "tic -o {} -x {}",
        dir.display(),
        dir.join("ghostty.src").display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_matches_oracle() {
        let src = Source {
            names: &["ghostty", "xterm-ghostty", "Ghostty"],
            capabilities: &[
                Capability {
                    name: "am",
                    value: Value::Boolean,
                },
                Capability {
                    name: "ccc",
                    value: Value::Canceled,
                },
                Capability {
                    name: "colors",
                    value: Value::Numeric(256),
                },
                Capability {
                    name: "bel",
                    value: Value::String("^G"),
                },
            ],
        };
        let mut out = Vec::new();
        src.encode(&mut out);
        let expected = "ghostty|xterm-ghostty|Ghostty,\n\tam,\n\tccc@,\n\tcolors#256,\n\tbel=^G,\n";
        assert_eq!(String::from_utf8(out).unwrap(), expected);
    }

    #[test]
    fn xtgettcap_map_matches_oracle() {
        let src = Source {
            names: &["ghostty", "xterm-ghostty", "Ghostty"],
            capabilities: &[
                Capability {
                    name: "am",
                    value: Value::Boolean,
                },
                Capability {
                    name: "colors",
                    value: Value::Numeric(256),
                },
                Capability {
                    name: "kx",
                    value: Value::String("^?"),
                },
                Capability {
                    name: "kbs",
                    value: Value::String("^H"),
                },
                Capability {
                    name: "kf1",
                    value: Value::String("\\EOP"),
                },
                Capability {
                    name: "Smulx",
                    value: Value::String("\\E[4:%p1%dm"),
                },
            ],
        };
        let map = src.xtgettcap_map();
        assert_eq!(map.get("616D").unwrap(), b"\x1bP1+r616D\x1b\\");
        assert_eq!(map.get("6B78").unwrap(), b"\x1bP1+r6B78=7F\x1b\\");
        assert_eq!(map.get("6B6273").unwrap(), b"\x1bP1+r6B6273=08\x1b\\");
        assert_eq!(map.get("6B6631").unwrap(), b"\x1bP1+r6B6631=1B4F50\x1b\\");
        assert_eq!(
            map.get("536D756C78").unwrap(),
            b"\x1bP1+r536D756C78=5C455B343A25703125646D\x1b\\"
        );
        // TN / Co / RGB synthetic entries.
        assert_eq!(
            map.get("544E").unwrap(),
            b"\x1bP1+r544E=67686F73747479\x1b\\"
        );
        assert_eq!(map.get("436F").unwrap(), b"\x1bP1+r436F=323536\x1b\\");
        assert_eq!(map.get("524742").unwrap(), b"\x1bP1+r524742=38\x1b\\");
    }

    #[test]
    fn ghostty_entry_encodes() {
        let mut out = Vec::new();
        GHOSTTY.encode(&mut out);
        assert!(!out.is_empty());
        assert!(String::from_utf8_lossy(&out).starts_with("xterm-ghostty|ghostty|Ghostty,\n"));
        // Spot-check a few source lines.
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("\tcolors#256,\n"));
        assert!(text.contains("\tam,\n"));
        assert!(text.contains("\tSmulx=\\E[4:%p1%dm,\n"));
        assert!(text.contains("\tkf63=\\E[1;4R,\n"));
        assert!(text.contains("\tsc=\\E7,\n"));
    }

    #[test]
    fn install_writes_source_only() {
        let dir = std::env::temp_dir().join(format!("mr-crabs-ti-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = install_source_to(&dir).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"xterm-ghostty|ghostty|Ghostty,\n"));
        let cmd = compile_command(&dir);
        assert!(cmd.starts_with("tic -o "));
        assert!(cmd.contains("ghostty.src"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_paths_are_absolute() {
        let paths = install_paths();
        assert!(!paths.is_empty());
        for p in &paths {
            assert!(p.is_absolute(), "{p:?} must be absolute");
        }
    }
}
