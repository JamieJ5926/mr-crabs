//! Display width used by printable input. Control C0/C1 and DEL do not
//! occupy a cell; combining marks occupy zero; East-Asian / emoji wide
//! occupy two. Widths come from `unicode-width` so TUI apps (OMP) match
//! Ghostty/Alacritty cell counts.

use unicode_width::UnicodeWidthChar;

pub fn char_width(c: char) -> Option<usize> {
    match c {
        '\u{00}'..='\u{1F}' | '\u{7F}' | '\u{80}'..='\u{9F}' => None,
        c => UnicodeWidthChar::width(c),
    }
}

#[cfg(test)]
mod tests {
    use super::char_width;

    #[test]
    fn controls_occupy_no_cell() {
        assert_eq!(char_width('\n'), None);
        assert_eq!(char_width('\u{1b}'), None);
        assert_eq!(char_width('\u{7f}'), None);
    }

    #[test]
    fn ascii_and_emoji_match_unicode_width() {
        assert_eq!(char_width('A'), Some(1));
        assert_eq!(char_width('π'), Some(1));
        // U+1F7E3 large purple circle — missing from the old hand table.
        assert_eq!(char_width('\u{1F7E3}'), Some(2));
        assert_eq!(char_width('中'), Some(2));
    }
}
