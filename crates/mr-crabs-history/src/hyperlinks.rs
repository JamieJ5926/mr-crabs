//! OSC 8 hyperlink lookup over the visible grid (S8: `hyperlink-interaction`).
//!
//! The engine stores the hyperlink identity on every cell of the link span
//! (alacritty `Cell::hyperlink`, applied by the OSC 8 handler through the
//! cursor template), so a lookup at any covered cell returns the same
//! identity. [`hyperlink_span`] scans left/right to recover the full span —
//! bounded by the grid width in each direction.

use mr_crabs_terminal::{HyperlinkInfo, Terminal};

/// Alacritty assigns a synthesized `"{n}_alacritty"` id to links whose OSC 8
/// payload carried no explicit id (cell.rs `HyperlinkInner::new`). Those
/// synthetic ids are an engine-internal identity, not part of the wire
/// contract: the id is only exposed when the payload carried one.
fn explicit_id(id: &str) -> Option<String> {
    if let Some(prefix) = id.strip_suffix("_alacritty") {
        if !prefix.is_empty() && prefix.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
    }
    Some(id.to_owned())
}

/// The hyperlink covering the visible-grid cell at `(row, col)`.
pub fn hyperlink_at(term: &Terminal, row: u16, col: u16) -> Option<HyperlinkInfo> {
    term.hyperlink_at(row, col).map(|info| HyperlinkInfo {
        id: info.id.and_then(|id| explicit_id(&id)),
        uri: info.uri,
    })
}

/// The full OSC 8 span covering the cell at `(row, col)`, when it is part of
/// a hyperlink. `end_col` is exclusive; the scan is bounded by the grid
/// width in each direction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperlinkSpan {
    pub row: u16,
    pub start_col: u16,
    pub end_col: u16,
    pub id: Option<String>,
    pub uri: String,
}

pub fn hyperlink_span(term: &Terminal, row: u16, col: u16) -> Option<HyperlinkSpan> {
    let info = hyperlink_at(term, row, col)?;
    let cols = term.size().cols;
    let mut start = col;
    while start > 0 {
        let Some(left) = hyperlink_at(term, row, start - 1) else {
            break;
        };
        if left.uri != info.uri || left.id != info.id {
            break;
        }
        start -= 1;
    }
    let mut end = col + 1;
    while end < cols {
        let Some(right) = hyperlink_at(term, row, end) else {
            break;
        };
        if right.uri != info.uri || right.id != info.id {
            break;
        }
        end += 1;
    }
    Some(HyperlinkSpan {
        row,
        start_col: start,
        end_col: end,
        id: info.id,
        uri: info.uri,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mr_crabs_terminal::{GridSize, Terminal};

    #[test]
    fn hyperlink_lookup_and_span() {
        let size = GridSize::new(20, 3);
        let mut term = Terminal::new(size).unwrap();
        // OSC 8 link over "link text here" then an unlinked tail.
        term.feed(b"\x1b]8;;https://example.com\x07link text here\x1b]8;;\x07 and more");
        let snap = term.snapshot();
        // 14 linked cells + 9 tail cells = 23 chars wrap past the 20-column
        // grid, so the cursor lands on row 1.
        assert_eq!(snap.cursor.row, 1);

        // The link covers "link text here" (14 cells, columns 0..14).
        let hit = hyperlink_at(&term, 0, 0).expect("link at row 0 col 0");
        assert_eq!(hit.uri, "https://example.com");
        assert_eq!(hit.id, None);

        let span = hyperlink_span(&term, 0, 5).expect("span covers col 5");
        assert_eq!(span.start_col, 0);
        assert_eq!(span.end_col, 14);
        assert_eq!(span.uri, "https://example.com");

        // The unlinked tail has no hyperlink.
        assert!(hyperlink_at(&term, 0, 16).is_none());
        assert!(hyperlink_span(&term, 0, 16).is_none());

        // Out-of-range lookups fail closed.
        assert!(hyperlink_at(&term, 99, 0).is_none());
        assert!(hyperlink_at(&term, 0, 99).is_none());
    }

    #[test]
    fn hyperlink_with_explicit_id() {
        let size = GridSize::new(12, 2);
        let mut term = Terminal::new(size).unwrap();
        term.feed(b"\x1b]8;id=42;https://x.example/a\x07abc\x1b]8;;\x07");
        let hit = hyperlink_at(&term, 0, 1).expect("link");
        assert_eq!(hit.id.as_deref(), Some("42"));
        assert_eq!(hit.uri, "https://x.example/a");
        let span = hyperlink_span(&term, 0, 1).expect("span");
        assert_eq!((span.start_col, span.end_col), (0, 3));
    }
}
