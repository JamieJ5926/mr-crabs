//! Ghostty Glyph Protocol APC support: request parsing, response encoding,
//! the bounded per-session glossary, and simple-glyf payload validation.
//!
//! Provenance: `src/terminal/apc/glyph/` (request.zig, response.zig,
//! Glossary.zig, execute.zig, and `src/font/opentype/glyf.zig`) at Ghostty
//! commit `d2c70a8c7b9b6893c13640c02d7b6f9a1624f3f0`.
//!
//! Wire framing: `ESC _ 25a1 ; <verb> [ ; key=value ]* [ ; <payload> ] ESC \`
//! with verbs `s` (support), `q` (codepoint query), `r` (register),
//! `c` (clear). Registrations are restricted to the Unicode Private Use
//! Areas; the glossary holds at most 1024 entries per session with FIFO
//! eviction; decoded payloads are bounded at 64 KiB.

pub mod glossary;
pub mod glyf;
pub mod request;
pub mod response;

pub use glossary::{EntryError, Glossary, GlyphEntry};
pub use request::{
    Align, Clear, ClearOption, Format, Pad, Query, QueryOption, Register, RegisterOption,
    RegisterValue, Reply, Request, RequestError, Size, Width,
};
pub use response::{
    ClearResponse, Coverage, Formats, QueryResponse, Reason, RegisterResponse, Response, Support,
};

/// APC identifier for the glyph protocol (`apc/glyph.zig` `identifier`).
pub const IDENTIFIER: &[u8] = b"25a1";

/// Maximum decoded glyph payload size accepted by the protocol (64 KiB).
pub const MAX_PAYLOAD_SIZE: usize = 64 * 1024;

/// Maximum entries allowed in the glossary before eviction (spec-defined).
pub const MAX_GLOSSARY_ENTRIES: usize = 1024;

/// Default maximum bytes an APC command can buffer (1 MiB, the oracle's
/// `Protocol.defaultMaxBytes(.glyph)`).
pub const DEFAULT_MAX_COMMAND_BYTES: usize = 1024 * 1024;

/// Execute a glyph protocol request against the given glossary.
///
/// Never fails; errors are encoded in the response. `system_coverage`
/// reports whether a system font covers a codepoint (the oracle's callers
/// fill this in from their font stack; tests and headless callers pass a
/// closure returning false).
pub fn execute(
    glossary: &mut Glossary,
    req: &Request,
    system_coverage: &dyn Fn(u32) -> bool,
) -> Option<Response> {
    match req {
        Request::Support => Some(Response::Support(Support {
            fmt: Formats {
                glyf: true,
                ..Formats::default()
            },
        })),
        Request::Query(q) => {
            let cp = q.get(QueryOption::Cp)?;
            Some(Response::Query(QueryResponse {
                cp,
                status: Coverage {
                    system: system_coverage(cp),
                    glossary: glossary.contains(cp),
                },
            }))
        }
        Request::Register(reg) => register(glossary, reg),
        Request::Clear(clr) => clear(glossary, clr),
    }
}

fn register(glossary: &mut Glossary, reg: &Register) -> Option<Response> {
    let reply = match reg.get(RegisterOption::Reply) {
        Some(RegisterValue::Reply(reply)) => reply,
        _ => Reply::All,
    };
    match register_fallible(glossary, reg) {
        Ok(cp) => match reply {
            Reply::None | Reply::Failures => None,
            Reply::All => Some(Response::Register(RegisterResponse {
                cp,
                ..Default::default()
            })),
        },
        Err(err) => {
            let reason = match err {
                EntryError::OutOfMemory => Reason::Other("out_of_memory"),
                EntryError::OutOfNamespace => Reason::OutOfNamespace,
                EntryError::PayloadTooLarge => Reason::PayloadTooLarge,
                EntryError::MalformedPayload
                | EntryError::InvalidOptions
                | EntryError::UnsupportedFormat => Reason::MalformedPayload,
                EntryError::CompositeUnsupported => Reason::CompositeUnsupported,
                EntryError::HintingUnsupported => Reason::HintingUnsupported,
            };
            match reply {
                Reply::None => None,
                Reply::All | Reply::Failures => Some(Response::Register(RegisterResponse {
                    cp: match reg.get(RegisterOption::Cp) {
                        Some(RegisterValue::Cp(cp)) => cp,
                        _ => 0,
                    },
                    status: 1,
                    reason: Some(reason),
                })),
            }
        }
    }
}

fn register_fallible(glossary: &mut Glossary, reg: &Register) -> Result<u32, EntryError> {
    let cp = match reg.get(RegisterOption::Cp) {
        Some(RegisterValue::Cp(cp)) => cp,
        _ => return Err(EntryError::MalformedPayload),
    };
    let entry = GlyphEntry::from_register(reg)?;
    glossary.register(cp, entry)?;
    Ok(cp)
}

fn clear(glossary: &mut Glossary, clr: &Clear) -> Option<Response> {
    if let Some(cp) = clr.get(ClearOption::Cp) {
        match glossary.delete(cp) {
            Ok(()) => {}
            Err(EntryError::OutOfNamespace) => {
                return Some(Response::Clear(ClearResponse {
                    status: 1,
                    reason: Some("out_of_namespace"),
                }));
            }
            Err(_) => unreachable!("delete only fails with OutOfNamespace"),
        }
    } else if clr.has(ClearOption::Cp) {
        return Some(Response::Clear(ClearResponse {
            status: 1,
            reason: Some("malformed_payload"),
        }));
    } else {
        glossary.clear_and_free();
    }
    Some(Response::Clear(ClearResponse::default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::request::RequestParser;

    fn parse(data: &str) -> Request {
        let mut p = RequestParser::new(DEFAULT_MAX_COMMAND_BYTES);
        p.feed_slice(data.as_bytes()).unwrap();
        p.complete().unwrap()
    }

    fn execute_str(glossary: &mut Glossary, data: &str) -> Option<Response> {
        let req = parse(data);
        execute(glossary, &req, &|_| false)
    }

    fn encode(resp: &Response) -> Vec<u8> {
        let mut out = Vec::new();
        resp.encode(&mut out);
        out
    }

    #[test]
    fn support_reply_advertises_glyf() {
        let mut g = Glossary::default();
        let resp = execute_str(&mut g, "s").unwrap();
        assert_eq!(encode(&resp), b"\x1b_25a1;s;fmt=glyf\x1b\\");
    }

    #[test]
    fn query_reports_glossary_coverage() {
        let mut g = Glossary::default();
        // Register e0a0 first.
        let req = parse("r;cp=e0a0;AAAAAAAAAAAAAA==");
        execute(&mut g, &req, &|_| false);

        let resp = execute_str(&mut g, "q;cp=e0a0").unwrap();
        assert_eq!(encode(&resp), b"\x1b_25a1;q;cp=e0a0;status=glossary\x1b\\");
        let resp = execute_str(&mut g, "q;cp=e0a1").unwrap();
        assert_eq!(encode(&resp), b"\x1b_25a1;q;cp=e0a1;status=\x1b\\");
        // Missing cp -> no response.
        assert!(execute_str(&mut g, "q").is_none());
    }

    #[test]
    fn query_includes_system_coverage() {
        let mut g = Glossary::default();
        let resp = execute_str(&mut g, "q;cp=41").unwrap();
        assert_eq!(encode(&resp), b"\x1b_25a1;q;cp=41;status=\x1b\\");
        // With a system font covering 'A'.
        let req = parse("q;cp=41");
        let resp = execute(&mut g, &req, &|cp| cp == 0x41).unwrap();
        assert_eq!(encode(&resp), b"\x1b_25a1;q;cp=41;status=system\x1b\\");
    }

    #[test]
    fn register_success_reply_and_fifo_eviction() {
        let mut g = Glossary::default();
        let resp = execute_str(&mut g, "r;cp=e0a0;AAAAAAAAAAAAAA==").unwrap();
        assert_eq!(encode(&resp), b"\x1b_25a1;r;cp=e0a0;status=0\x1b\\");
        assert!(g.contains(0xE0A0));

        // Fill to capacity with entries that do not collide with e0a0:
        // the 1024th new entry pushes the count past the bound and evicts
        // the oldest entry (e0a0) per FIFO.
        for i in 0..MAX_GLOSSARY_ENTRIES {
            let cp = 0xE100 + i as u32;
            let data = format!("r;cp={cp:x};AAAAAAAAAAAAAA==");
            execute_str(&mut g, &data);
        }
        assert_eq!(g.len(), MAX_GLOSSARY_ENTRIES);
        assert!(!g.contains(0xE0A0), "oldest entry evicted");
        assert!(g.contains(0xE100 + (MAX_GLOSSARY_ENTRIES - 1) as u32));
    }

    #[test]
    fn register_rejects_non_pua() {
        let mut g = Glossary::default();
        let resp = execute_str(&mut g, "r;cp=41;AAAAAAAAAAAAAA==").unwrap();
        assert_eq!(
            encode(&resp),
            b"\x1b_25a1;r;cp=41;status=1;reason=out_of_namespace\x1b\\"
        );
        assert!(!g.contains(0x41));
    }

    #[test]
    fn register_rejects_malformed_payload() {
        let mut g = Glossary::default();
        let resp = execute_str(&mut g, "r;cp=e0a0;%%%not-base64%%%").unwrap();
        assert_eq!(
            encode(&resp),
            b"\x1b_25a1;r;cp=e0a0;status=1;reason=malformed_payload\x1b\\"
        );
        assert!(!g.contains(0xE0A0));
    }

    #[test]
    fn register_payload_too_large() {
        let mut g = Glossary::default();
        // 64 KiB + 1 of payload, base64 encoded.
        let big = "A".repeat((MAX_PAYLOAD_SIZE + 1) * 4 / 3 + 8);
        let data = format!("r;cp=e0a0;{big}");
        let resp = execute_str(&mut g, &data).unwrap();
        assert_eq!(
            encode(&resp),
            b"\x1b_25a1;r;cp=e0a0;status=1;reason=payload_too_large\x1b\\"
        );
    }

    #[test]
    fn register_reply_verbosity() {
        let mut g = Glossary::default();
        // reply=2: silent success.
        assert!(execute_str(&mut g, "r;cp=e0a0;reply=2;AAAAAAAAAAAAAA==").is_none());
        assert!(g.contains(0xE0A0));
        // reply=0: silent failure too.
        assert!(execute_str(&mut g, "r;cp=41;reply=0;%%%").is_none());
        assert!(!g.contains(0x41));
        // reply=2: failure still replies.
        let resp = execute_str(&mut g, "r;cp=41;reply=2;%%%").unwrap();
        assert_eq!(
            encode(&resp),
            b"\x1b_25a1;r;cp=41;status=1;reason=malformed_payload\x1b\\"
        );
    }

    #[test]
    fn clear_all_and_single() {
        let mut g = Glossary::default();
        execute_str(&mut g, "r;cp=e0a0;AAAAAAAAAAAAAA==");
        execute_str(&mut g, "r;cp=e0a1;AAAAAAAAAAAAAA==");
        let resp = execute_str(&mut g, "c").unwrap();
        assert_eq!(encode(&resp), b"\x1b_25a1;c;status=0\x1b\\");
        assert!(!g.contains(0xE0A0));
        assert!(!g.contains(0xE0A1));

        execute_str(&mut g, "r;cp=e0a0;AAAAAAAAAAAAAA==");
        execute_str(&mut g, "r;cp=e0a1;AAAAAAAAAAAAAA==");
        let resp = execute_str(&mut g, "c;cp=e0a0").unwrap();
        assert_eq!(encode(&resp), b"\x1b_25a1;c;status=0\x1b\\");
        assert!(!g.contains(0xE0A0));
        assert!(g.contains(0xE0A1));

        // Non-PUA clear is rejected.
        let resp = execute_str(&mut g, "c;cp=41").unwrap();
        assert_eq!(
            encode(&resp),
            b"\x1b_25a1;c;status=1;reason=out_of_namespace\x1b\\"
        );
        // Malformed cp is rejected without clearing.
        let resp = execute_str(&mut g, "c;cp=zz").unwrap();
        assert_eq!(
            encode(&resp),
            b"\x1b_25a1;c;status=1;reason=malformed_payload\x1b\\"
        );
        assert!(g.contains(0xE0A1));
    }

    #[test]
    fn re_registration_moves_entry_to_end() {
        let mut g = Glossary::default();
        execute_str(&mut g, "r;cp=e0a1;AAAAAAAAAAAAAA==");
        execute_str(&mut g, "r;cp=e0a2;AAAAAAAAAAAAAA==");
        // Re-register e0a1: it becomes newest, e0a2 becomes oldest.
        execute_str(&mut g, "r;cp=e0a1;AAAAAAAAAAAAAA==");
        // Fill to capacity with entries that do not collide with e0a1/
        // e0a2: the 1023rd new entry pushes the count past the bound and
        // evicts the oldest entry (e0a2).
        for i in 0..MAX_GLOSSARY_ENTRIES - 1 {
            let cp = 0xE100 + i as u32;
            let data = format!("r;cp={cp:x};AAAAAAAAAAAAAA==");
            execute_str(&mut g, &data);
        }
        assert!(g.contains(0xE0A1), "re-registered entry survives");
        assert!(!g.contains(0xE0A2), "oldest original entry evicted");
    }
}
