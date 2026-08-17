//! Word/line/block selection gestures and text extraction (S8:
//! `selection-gestures`).
//!
//! Mirrors Ghostty's selection model: a selection has an anchor (press pin)
//! and an active point (current drag position); the gesture determines how
//! the extent expands:
//! - `Cell` — exact cell range (single-click drag);
//! - `Word` — expand to non-boundary characters (double-click;
//!   `SelectionGesture.zig:151-152` default behaviors, `Screen.zig:3217`
//!   `selectWord`);
//! - `Line` — the whole soft-wrapped logical line, trimmed of leading and
//!   trailing whitespace (`Screen.zig:2960` `selectLine`);
//! - `Block` — a rectangle (`Selection.zig` `rectangle` selections).
//!
//! Word boundaries are configurable codepoints plus U+0000 (always a
//! boundary); the default matches Ghostty's `selection-word-chars`
//! (`src/terminal/selection_codepoints.zig:6-27`
//! `default_word_boundaries`, exposed via
//! `src/config/Config.zig:767`).
//!
//! Extraction mirrors the plain formatter with `unwrap=true, trim=true`
//! (`Screen.zig:2891` `selectionString`): wrapped rows join without
//! separators, other rows join with `\n`, trailing blanks are trimmed,
//! wide-character spacers are skipped.

use mr_crabs_terminal::{Cell, HistoryRead};

/// Default word-boundary characters (Ghostty `selection-word-chars` default,
/// `src/config/Config.zig:756`): tab, space, quote, box-drawing/pipe
/// variants, and common punctuation. U+0000 is always a boundary and is not
/// listed here.
pub const DEFAULT_WORD_BOUNDARIES: &str = "\t '\"│`|:;,()[]{}<>$";

/// Word-boundary set used by word selection.
#[derive(Clone, Debug)]
pub struct WordBoundaries {
    characters: Vec<char>,
}

impl WordBoundaries {
    pub fn new(characters: &str) -> Self {
        Self {
            characters: characters.chars().collect(),
        }
    }

    pub fn is_boundary(&self, c: char) -> bool {
        c == '\0' || self.characters.contains(&c)
    }
}

impl Default for WordBoundaries {
    fn default() -> Self {
        Self::new(DEFAULT_WORD_BOUNDARIES)
    }
}

/// The gesture that shapes a selection extent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionGesture {
    /// Exact cell range.
    Cell,
    /// Double-click word range.
    Word,
    /// Triple-click logical line range.
    Line,
    /// Rectangle (block) range.
    Block,
}

/// A point in the line space (history line or visible row).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionPoint {
    pub line: usize,
    pub col: u16,
}

/// An active selection: anchor plus current extent and gesture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selection {
    pub gesture: SelectionGesture,
    pub anchor: SelectionPoint,
    pub active: SelectionPoint,
}

impl Selection {
    pub fn new(gesture: SelectionGesture, anchor: SelectionPoint, active: SelectionPoint) -> Self {
        Self {
            gesture,
            anchor,
            active,
        }
    }

    /// Normalized (top-left, bottom-right) extent in line space.
    pub fn normalized(&self) -> (SelectionPoint, SelectionPoint) {
        let start = if self.anchor.line < self.active.line
            || (self.anchor.line == self.active.line && self.anchor.col <= self.active.col)
        {
            self.anchor
        } else {
            self.active
        };
        let end = if start == self.anchor {
            self.active
        } else {
            self.anchor
        };
        (start, end)
    }
}

/// Expand a word selection around `col` within one row. Spacer cells never
/// delimit words (their codepoint is a placeholder); a boundary cell yields
/// a single-cell word. Returns inclusive `(start_col, end_col)`.
pub fn expand_word(cells: &[Cell], col: u16, boundaries: &WordBoundaries) -> (u16, u16) {
    let cols = cells.len();
    if cols == 0 {
        return (0, 0);
    }
    let col = usize::from(col).min(cols - 1);
    if boundary_at(cells, col, boundaries) {
        return (
            u16::try_from(col).unwrap_or(u16::MAX),
            u16::try_from(col).unwrap_or(u16::MAX),
        );
    }
    let mut start = col;
    while start > 0 && !boundary_at(cells, start - 1, boundaries) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < cols && !boundary_at(cells, end + 1, boundaries) {
        end += 1;
    }
    (
        u16::try_from(start).unwrap_or(u16::MAX),
        u16::try_from(end).unwrap_or(u16::MAX),
    )
}

fn boundary_at(cells: &[Cell], col: usize, boundaries: &WordBoundaries) -> bool {
    let cell = &cells[col];
    if cell.flags & Cell::WIDE_SPACER != 0 {
        return false;
    }
    char::from_u32(cell.content).is_some_and(|c| boundaries.is_boundary(c))
}

/// Expand a line selection around `line` to the whole soft-wrapped logical
/// line. `row` returns `(cols, wrapped)` for an absolute line, or `None`
/// when out of range. Returns inclusive `(start_line, end_line)`.
pub fn expand_line<F>(mut row: F, line: usize) -> (usize, usize)
where
    F: FnMut(usize) -> Option<(u16, bool)>,
{
    let mut start = line;
    while start > 0 && row(start - 1).is_some_and(|(_, wrapped)| wrapped) {
        start -= 1;
    }
    let mut end = line;
    while row(end).is_some_and(|(_, wrapped)| wrapped) {
        end += 1;
    }
    (start, end)
}

/// Options for [`selection_text`].
#[derive(Clone, Copy, Debug)]
pub struct ExtractOptions {
    /// Trim leading/trailing whitespace of each selected line (line
    /// selection does this per Ghostty `selectLine`).
    pub trim_lines: bool,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self { trim_lines: true }
    }
}

/// Extract the selected text. `row` returns the cells of an absolute line
/// (history or visible). Wide spacers are skipped; blank cells become spaces
/// only when followed by content; blank rows emit one newline; wrapped rows
/// join without a separator; other row boundaries emit `\n`; a trailing
/// newline is trimmed.
///
/// A `Line` gesture expands to the whole soft-wrapped logical line
/// ([`expand_line`], Ghostty `selectLine`) and its leading/trailing
/// whitespace is trimmed; for other gestures only the trailing edge of each
/// logical line is trimmed (the formatter's `trim` option), so wrapped
/// continuation rows keep their mid-line blanks.
pub fn selection_text<F>(mut row: F, selection: &Selection, options: ExtractOptions) -> String
where
    F: FnMut(usize) -> Option<Vec<Cell>>,
{
    let (start_point, end_point) = selection.normalized();
    let (start_line, end_line) = if selection.gesture == SelectionGesture::Line {
        // Line selection covers the whole soft-wrapped run around the
        // anchor (Ghostty `selectLine`).
        let mut bounds = |line: usize| {
            row(line).map(|cells| {
                let cols = cells.len();
                let wrapped = cells.last().is_some_and(|cell| cell.flags & 0x0010 != 0);
                (u16::try_from(cols).unwrap_or(u16::MAX), wrapped)
            })
        };
        expand_line(&mut bounds, start_point.line)
    } else {
        (start_point.line, end_point.line)
    };
    let boundaries = WordBoundaries::default();
    let mut out = String::new();
    let mut pending_blank = 0usize;
    let mut pending_newlines = 0usize;
    // The first row of the current logical line: only its leading edge is
    // trimmed (line selection), never the mid-line blanks of a wrapped
    // continuation row.
    let mut logical_first = true;
    let mut line = start_line;
    while let Some(cells) = row(line) {
        let cols = cells.len();
        let (col_start, col_end) = match selection.gesture {
            SelectionGesture::Block => {
                let a = usize::from(start_point.col).min(cols.saturating_sub(1));
                let b = usize::from(end_point.col).min(cols.saturating_sub(1));
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                (lo, hi + 1)
            }
            SelectionGesture::Line => (0, cols),
            SelectionGesture::Word if line == start_line => {
                let (lo, hi) = expand_word(&cells, start_point.col, &boundaries);
                (usize::from(lo), usize::from(hi) + 1)
            }
            _ => {
                if line == start_line && line == end_line {
                    let lo = usize::from(start_point.col).min(cols);
                    let hi = usize::from(end_point.col).min(cols);
                    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
                    (lo, hi + 1)
                } else if line == start_line {
                    (usize::from(start_point.col).min(cols), cols)
                } else if line == end_line {
                    (0, usize::from(end_point.col).min(cols).saturating_add(1))
                } else {
                    (0, cols)
                }
            }
        };
        let mut text = String::new();
        for cell in cells.iter().take(col_end).skip(col_start) {
            if cell.flags & Cell::WIDE_SPACER != 0 {
                continue;
            }
            let content = cell.content;
            if content == 0 || content == u32::from(' ') {
                pending_blank += 1;
                continue;
            }
            if pending_blank > 0 {
                text.push_str(&" ".repeat(pending_blank));
                pending_blank = 0;
            }
            if let Some(ch) = char::from_u32(content) {
                text.push(ch);
            }
        }
        // Trailing blanks are trimmed; they survive only into a wrapped
        // continuation row.
        let wrapped = cells.last().is_some_and(|cell| cell.flags & 0x0010 != 0);
        if !wrapped {
            pending_blank = 0;
        }
        if options.trim_lines && logical_first && selection.gesture == SelectionGesture::Line {
            text = text.trim_start().to_owned();
        }
        if !text.is_empty() {
            for _ in 0..pending_newlines {
                out.push('\n');
            }
            pending_newlines = 0;
            out.push_str(&text);
        }
        if line == end_line {
            break;
        }
        // A non-wrapped row contributes one newline (blank rows included);
        // the final row's newline is trimmed.
        if !wrapped {
            pending_newlines += 1;
        }
        logical_first = !wrapped;
        line += 1;
    }
    out
}

/// Convenience: read one absolute line through a `HistoryRead` plus visible
/// rows, for use as the `row` closure of [`selection_text`] and
/// [`expand_line`].
pub fn read_line_at<R: HistoryRead + ?Sized>(
    reader: &mut R,
    visible: &[Vec<Cell>],
    line: usize,
) -> Option<Vec<Cell>> {
    let history_len = reader.history_len();
    if line < history_len {
        let mut cells = Vec::new();
        if reader.read_history_line(line, &mut cells) {
            Some(cells)
        } else {
            None
        }
    } else {
        visible.get(line - history_len).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(text: &str) -> Vec<Cell> {
        text.chars()
            .map(|ch| Cell {
                content: u32::from(ch),
                style: 0,
                flags: 0,
            })
            .collect()
    }

    #[test]
    fn word_expansion_respects_boundaries() {
        let boundaries = WordBoundaries::default();
        let line = cells("hello, world! foo_bar");
        // 'hello' at col 0..5, comma is a boundary.
        assert_eq!(expand_word(&line, 0, &boundaries), (0, 4));
        assert_eq!(expand_word(&line, 4, &boundaries), (0, 4));
        // ',' is a single-cell word.
        assert_eq!(expand_word(&line, 5, &boundaries), (5, 5));
        // 'world!' at 7..13: '!' is NOT in the Ghostty default boundary set
        // (selection_codepoints.zig default_word_boundaries), so the word
        // extends through it to the space at 13.
        assert_eq!(expand_word(&line, 8, &boundaries), (7, 12));
        // '_' is NOT in the default boundary set: 'foo_bar' is one word.
        assert_eq!(expand_word(&line, 16, &boundaries), (14, 20));
        // Space is a boundary.
        assert_eq!(expand_word(&line, 6, &boundaries), (6, 6));
    }

    #[test]
    fn word_expansion_skips_wide_spacers() {
        let boundaries = WordBoundaries::default();
        let mut line = cells("ab界cd");
        // 界 is wide: insert a spacer after it.
        line.insert(
            3,
            Cell {
                content: u32::from(' '),
                style: 0,
                flags: Cell::WIDE_SPACER,
            },
        );
        // Word at the wide char spans through the spacer: spacer cells never
        // delimit words, so the whole run "ab界cd" is one word.
        assert_eq!(expand_word(&line, 3, &boundaries), (0, 5));
        // Null content is always a boundary.
        let mut with_null = cells("ab");
        with_null.insert(
            1,
            Cell {
                content: 0,
                style: 0,
                flags: 0,
            },
        );
        assert_eq!(expand_word(&with_null, 0, &boundaries), (0, 0));
        assert_eq!(expand_word(&with_null, 2, &boundaries), (2, 2));
    }

    #[test]
    fn line_expansion_covers_soft_wrapped_runs() {
        let rows = [(6u16, false), (6, true), (6, false), (6, false)];
        let mut i = 0usize;
        let (start, end) = expand_line(
            |line| {
                let (cols, wrapped) = rows.get(line).copied()?;
                i += 1;
                Some((cols, wrapped))
            },
            1,
        );
        assert_eq!((start, end), (1, 2), "row 1 is the end of the wrapped run");
        let _ = i;

        let rows = [(6u16, true), (6, true), (6, false)];
        let (start, end) = expand_line(|line| rows.get(line).copied(), 1);
        assert_eq!((start, end), (0, 2));
    }

    #[test]
    fn cell_selection_text_trims_trailing_blanks_and_joins() {
        let mut rows = std::collections::HashMap::new();
        rows.insert(0usize, cells("alpha  "));
        rows.insert(1usize, cells("beta"));
        let selection = Selection::new(
            SelectionGesture::Cell,
            SelectionPoint { line: 0, col: 0 },
            SelectionPoint { line: 1, col: 3 },
        );
        let text = selection_text(
            |line| rows.get(&line).cloned(),
            &selection,
            ExtractOptions::default(),
        );
        assert_eq!(text, "alpha\nbeta", "trailing blanks trimmed, rows joined");
    }

    #[test]
    fn block_selection_is_rectangular() {
        let rows = [cells("abcdef"), cells("ghijkl"), cells("mnopqr")];
        let selection = Selection::new(
            SelectionGesture::Block,
            SelectionPoint { line: 0, col: 1 },
            SelectionPoint { line: 2, col: 3 },
        );
        let text = selection_text(
            |line| rows.get(line).cloned(),
            &selection,
            ExtractOptions::default(),
        );
        assert_eq!(text, "bcd\nhij\nnop", "rectangle columns 1..=3 per row");
    }

    #[test]
    fn line_selection_trims_whitespace() {
        let rows = [cells("   padded line   ")];
        let selection = Selection::new(
            SelectionGesture::Line,
            SelectionPoint { line: 0, col: 3 },
            SelectionPoint { line: 0, col: 4 },
        );
        let text = selection_text(
            |line| rows.get(line).cloned(),
            &selection,
            ExtractOptions::default(),
        );
        assert_eq!(text, "padded line");
    }

    #[test]
    fn word_selection_extracts_the_word() {
        let rows = [cells("say hello now")];
        let selection = Selection::new(
            SelectionGesture::Word,
            SelectionPoint { line: 0, col: 5 },
            SelectionPoint { line: 0, col: 5 },
        );
        let text = selection_text(
            |line| rows.get(line).cloned(),
            &selection,
            ExtractOptions::default(),
        );
        assert_eq!(text, "hello");
    }

    #[test]
    fn blank_rows_and_wrapped_rows_join_like_the_formatter() {
        // Content, blank, content: the blank row emits one newline.
        let rows = [cells("aa"), cells(""), cells("bb")];
        let selection = Selection::new(
            SelectionGesture::Cell,
            SelectionPoint { line: 0, col: 0 },
            SelectionPoint { line: 2, col: 1 },
        );
        let text = selection_text(
            |line| rows.get(line).cloned(),
            &selection,
            ExtractOptions::default(),
        );
        assert_eq!(text, "aa\n\nbb");

        // Wrapped rows join without a separator.
        let mut wrapped = cells("ab  ");
        if let Some(last) = wrapped.last_mut() {
            last.flags |= 0x0010;
        }
        let rows = [wrapped, cells("cd")];
        let selection = Selection::new(
            SelectionGesture::Cell,
            SelectionPoint { line: 0, col: 0 },
            SelectionPoint { line: 1, col: 1 },
        );
        let text = selection_text(
            |line| rows.get(line).cloned(),
            &selection,
            ExtractOptions::default(),
        );
        assert_eq!(
            text, "ab  cd",
            "wrapped-row trailing blanks preserved mid-line"
        );
    }
}
