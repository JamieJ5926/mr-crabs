//! S7 — Kitty/iTerm2/Ghostty graphics: image protocols, decoded image
//! storage, and a bounded texture-cache interface for GPUI.
//!
//! Provenance: Ghostty source commit `d2c70a8c7b9b6893c13640c02d7b6f9a1624f3f0`,
//! `src/terminal/kitty/**` (graphics_command.zig, graphics_image.zig,
//! graphics_storage.zig, graphics_exec.zig), `src/terminal/apc/glyph/**`
//! (request.zig, response.zig, Glossary.zig, execute.zig),
//! `src/terminal/osc/parsers/iterm2.zig`, `src/font/opentype/glyf.zig`.
//! No Zig runtime is linked; every payload and cache has an explicit
//! size/count bound.
//!
//! Modules:
//! - `kitty::command` — kitty graphics APC payload parser and response
//!   encoder (actions q/t/T/p/d/f/a/c, quiet modes, bounded base64 data).
//! - `kitty::load` — `LoadingImage`: chunk accumulation, direct/file/
//!   temporary-file/shared-memory transports with bounds and temp cleanup,
//!   zlib inflate, PNG decode.
//! - `image` — decoded `Image` representation, `ImageError` with oracle
//!   response messages, bounded PNG decode to RGBA, bounded zlib inflate.
//! - `placement` — placement geometry (pixel/grid sizes, rects), the
//!   terminal context, and placement keys.
//! - `store` — `ImageStore`: byte/count budgets, deterministic eviction,
//!   generation stamps, delete semantics, and command execution with
//!   quiet-mode response filtering.
//! - `iterm` — OSC 1337 `File=` parsing, chunked upload accumulation, and
//!   PNG loading. Sixel is explicitly not implemented.
//! - `glyph` — Ghostty Glyph Protocol: request parsing, response encoding,
//!   the bounded glossary, and simple-glyf payload validation.
//! - `texture` — the bounded deterministic LRU texture cache and opaque
//!   handles consumable by a GPUI element without another renderer.
//! - `host` — the `GraphicsHost` integration seam (PTY responses, cursor
//!   movement, storage-change notification).

pub mod glyph;
pub mod host;
pub mod image;
pub mod iterm;
pub mod kitty;
pub mod placement;
pub mod store;
pub mod texture;

#[cfg(test)]
pub(crate) mod testutil {
    use std::fmt::Write;

    /// SHA-256 hex of `data` (std-only; corpus hash assertions).
    pub fn sha256_hex(data: &[u8]) -> String {
        // FIPS 180-4 SHA-256.
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        const H0: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        let mut h = H0;
        let mut msg = data.to_vec();
        let bit_len = (data.len() as u64).wrapping_mul(8);
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bit_len.to_be_bytes());
        for chunk in msg.chunks_exact(64) {
            let mut w = [0u32; 64];
            for (i, word) in chunk.chunks_exact(4).enumerate() {
                w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
                (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ (!e & g);
                let t1 = hh
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let t2 = s0.wrapping_add(maj);
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }
            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
            h[5] = h[5].wrapping_add(f);
            h[6] = h[6].wrapping_add(g);
            h[7] = h[7].wrapping_add(hh);
        }
        let mut out = String::with_capacity(64);
        for v in h {
            write!(out, "{v:08x}").unwrap();
        }
        out
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
