//! Glyph Protocol response encoding.
//!
//! Faithful port of `src/terminal/apc/glyph/response.zig`. Responses are
//! formatted as `ESC _ 25a1 ; <verb> ; <key=value>* ESC \` and are bounded
//! to 1024 bytes on the wire.

/// Recommended fixed buffer size for a formatted response (oracle
/// `max_wire_bytes`).
pub const MAX_WIRE_BYTES: usize = 1024;

/// Query response coverage state for a codepoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// A system font covers the codepoint.
    pub system: bool,
    /// A session glyph registration covers the codepoint.
    pub glossary: bool,
}

impl Coverage {
    /// Parse a comma-separated coverage list; unknown names are ignored.
    pub fn init(value: &str) -> Coverage {
        let mut result = Coverage::default();
        for name in value.split(',') {
            match name {
                "system" => result.system = true,
                "glossary" => result.glossary = true,
                _ => {}
            }
        }
        result
    }
}

/// Supported payload formats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Formats {
    pub glyf: bool,
    pub colrv0: bool,
    pub colrv1: bool,
}

/// Support query response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Support {
    pub fmt: Formats,
}

/// Codepoint query response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryResponse {
    pub cp: u32,
    pub status: Coverage,
}

/// Register error reason codes (spec §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    OutOfNamespace,
    CompositeUnsupported,
    HintingUnsupported,
    MalformedPayload,
    PayloadTooLarge,
    /// A reason code not known by this version of Ghostty.
    Other(&'static str),
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Reason::OutOfNamespace => "out_of_namespace",
            Reason::CompositeUnsupported => "composite_unsupported",
            Reason::HintingUnsupported => "hinting_unsupported",
            Reason::MalformedPayload => "malformed_payload",
            Reason::PayloadTooLarge => "payload_too_large",
            Reason::Other(value) => value,
        }
    }
}

/// Register response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RegisterResponse {
    pub cp: u32,
    pub status: u8,
    pub reason: Option<Reason>,
}

/// Clear response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ClearResponse {
    pub status: u8,
    pub reason: Option<&'static str>,
}

/// A response to a glyph APC request, formatted for the wire protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    Support(Support),
    Query(QueryResponse),
    Register(RegisterResponse),
    Clear(ClearResponse),
}

impl Response {
    /// Format into the glyph APC wire format.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"\x1b_25a1;");
        match self {
            Response::Support(r) => {
                out.extend_from_slice(b"s;fmt=");
                let mut first = true;
                if r.fmt.glyf {
                    first = false;
                    out.extend_from_slice(b"glyf");
                }
                if r.fmt.colrv0 {
                    if !first {
                        out.push(b',');
                    }
                    first = false;
                    out.extend_from_slice(b"colrv0");
                }
                if r.fmt.colrv1 {
                    if !first {
                        out.push(b',');
                    }
                    out.extend_from_slice(b"colrv1");
                }
            }
            Response::Query(r) => {
                out.extend_from_slice(format!("q;cp={:x};status=", r.cp).as_bytes());
                let mut first = true;
                if r.status.system {
                    first = false;
                    out.extend_from_slice(b"system");
                }
                if r.status.glossary {
                    if !first {
                        out.push(b',');
                    }
                    out.extend_from_slice(b"glossary");
                }
            }
            Response::Register(r) => {
                out.extend_from_slice(format!("r;cp={:x};status={}", r.cp, r.status).as_bytes());
                if let Some(reason) = r.reason {
                    out.extend_from_slice(b";reason=");
                    out.extend_from_slice(reason.name().as_bytes());
                }
            }
            Response::Clear(r) => {
                out.extend_from_slice(format!("c;status={}", r.status).as_bytes());
                if let Some(reason) = r.reason {
                    out.extend_from_slice(b";reason=");
                    out.extend_from_slice(reason.as_bytes());
                }
            }
        }
        out.extend_from_slice(b"\x1b\\");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(resp: &Response) -> Vec<u8> {
        let mut out = Vec::new();
        resp.encode(&mut out);
        out
    }

    #[test]
    fn support_with_formats() {
        let resp = Response::Support(Support {
            fmt: Formats {
                glyf: true,
                colrv0: true,
                ..Formats::default()
            },
        });
        assert_eq!(enc(&resp), b"\x1b_25a1;s;fmt=glyf,colrv0\x1b\\");
        let resp = Response::Support(Support {
            fmt: Formats::default(),
        });
        assert_eq!(enc(&resp), b"\x1b_25a1;s;fmt=\x1b\\");
    }

    #[test]
    fn query_wire_format() {
        let resp = Response::Query(QueryResponse {
            cp: 0xE0A0,
            status: Coverage {
                system: true,
                glossary: true,
            },
        });
        assert_eq!(
            enc(&resp),
            b"\x1b_25a1;q;cp=e0a0;status=system,glossary\x1b\\"
        );
        let resp = Response::Query(QueryResponse {
            cp: 0xE0A0,
            status: Coverage::default(),
        });
        assert_eq!(enc(&resp), b"\x1b_25a1;q;cp=e0a0;status=\x1b\\");
    }

    #[test]
    fn register_wire_format() {
        let resp = Response::Register(RegisterResponse {
            cp: 0xE0A0,
            ..Default::default()
        });
        assert_eq!(enc(&resp), b"\x1b_25a1;r;cp=e0a0;status=0\x1b\\");
        let resp = Response::Register(RegisterResponse {
            cp: 0xE0A0,
            status: 1,
            reason: Some(Reason::OutOfNamespace),
        });
        assert_eq!(
            enc(&resp),
            b"\x1b_25a1;r;cp=e0a0;status=1;reason=out_of_namespace\x1b\\"
        );
    }

    #[test]
    fn clear_wire_format() {
        let resp = Response::Clear(ClearResponse::default());
        assert_eq!(enc(&resp), b"\x1b_25a1;c;status=0\x1b\\");
        let resp = Response::Clear(ClearResponse {
            status: 1,
            reason: Some("out_of_namespace"),
        });
        assert_eq!(
            enc(&resp),
            b"\x1b_25a1;c;status=1;reason=out_of_namespace\x1b\\"
        );
    }

    #[test]
    fn coverage_parses_names() {
        assert_eq!(Coverage::init(""), Coverage::default());
        assert_eq!(
            Coverage::init("system"),
            Coverage {
                system: true,
                glossary: false
            }
        );
        assert_eq!(
            Coverage::init("glossary,system,future"),
            Coverage {
                system: true,
                glossary: true
            }
        );
    }
}
