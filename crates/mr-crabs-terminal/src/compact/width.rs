//! Display width used by printable input. Control C0/C1 and DEL do not
//! occupy a cell; combining marks occupy zero; East-Asian / emoji wide
//! occupy two. Per-character widths come from `unicode-width` so TUI
//! apps (OMP) match Ghostty/Alacritty cell counts. ZWJ-joined emoji,
//! VS16 presentation, and regional-indicator flags are measured as
//! clusters via `UnicodeWidthStr`, because summing per-character
//! widths overcounts those sequences.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn char_width(c: char) -> Option<usize> {
    match c {
        '\u{00}'..='\u{1F}' | '\u{7F}' | '\u{80}'..='\u{9F}' => None,
        c => UnicodeWidthChar::width(c),
    }
}

/// Display columns occupied by a grapheme cluster.
///
/// ZWJ-joined family emoji, VS16 presentation sequences, skin-tone
/// modifiers, and regional-indicator flags collapse to the cluster
/// width (typically 2). Isolated nerd-font PUA, CJK, and ASCII keep
/// their per-character widths.
#[allow(dead_code)]
pub fn cluster_width(cluster: &str) -> usize {
    UnicodeWidthStr::width(cluster)
}

#[cfg(test)]
mod tests {
    use super::{char_width, cluster_width};

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
        assert_eq!(char_width('\u{1F7E3}'), Some(2));
        assert_eq!(char_width('中'), Some(2));
    }

    #[test]
    fn zwj_and_vs16_are_zero_width_as_chars() {
        assert_eq!(char_width('\u{200D}'), Some(0));
        assert_eq!(char_width('\u{FE0F}'), Some(0));
    }

    #[test]
    fn nerd_pua_is_single_cell() {
        for c in [
            '\u{E000}',
            '\u{E00D}',
            '\u{E0A0}',
            '\u{F015}',
            '\u{E700}',
            '\u{F8FF}',
            '\u{F0000}',
            '\u{FFFFD}',
            '\u{100000}',
            '\u{10FFFD}',
        ] {
            assert_eq!(char_width(c), Some(1), "U+{:04X}", c as u32);
            let mut buf = [0u8; 4];
            assert_eq!(cluster_width(c.encode_utf8(&mut buf)), 1);
        }
    }

    #[test]
    fn cluster_width_table() {
        const CASES: &[(&str, &str, usize)] = &[
            ("ascii", "A", 1),
            ("ascii_run", "hello", 5),
            ("cjk", "中", 2),
            ("cjk_pair", "中文", 4),
            ("single_emoji", "\u{1F7E3}", 2),
            ("party_emoji", "\u{1F389}", 2),
            ("family_zwj", "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", 2),
            (
                "family_zwj_four",
                "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}",
                2,
            ),
            ("woman_technologist", "\u{1F469}\u{200D}\u{1F4BB}", 2),
            (
                "kiss_zwj",
                "\u{1F469}\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F48B}\u{200D}\u{1F468}",
                2,
            ),
            (
                "rainbow_flag",
                "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}",
                2,
            ),
            ("zwj_alone", "\u{200D}", 0),
            ("vs16_alone", "\u{FE0F}", 0),
            ("heart_text", "\u{2764}", 1),
            ("heart_vs16", "\u{2764}\u{FE0F}", 2),
            ("sun_vs16", "\u{2600}\u{FE0F}", 2),
            ("play_vs16", "\u{25B6}\u{FE0F}", 2),
            ("wave_skintone", "\u{1F44B}\u{1F3FD}", 2),
            ("thumb_skintone", "\u{1F44D}\u{1F3FF}", 2),
            ("thumb_light", "\u{1F44D}\u{1F3FB}", 2),
            ("flag_us", "\u{1F1FA}\u{1F1F8}", 2),
            ("pua_bmp_start", "\u{E000}", 1),
            ("pua_nerd_powerline", "\u{E0A0}", 1),
            ("pua_nerd_devicon", "\u{E700}", 1),
            ("pua_bmp_end", "\u{F8FF}", 1),
            ("pua_plane15", "\u{F0000}", 1),
            ("pua_plane16", "\u{10FFFD}", 1),
            ("acute_e", "e\u{0301}", 1),
            ("keycap_one", "1\u{FE0F}\u{20E3}", 2),
        ];
        for (name, cluster, expected) in CASES {
            assert_eq!(
                cluster_width(cluster),
                *expected,
                "{name}: {cluster:?} expected {expected} cols"
            );
        }
    }

    #[test]
    fn family_zwj_is_not_the_per_char_sum() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let naive: usize = family.chars().filter_map(char_width).sum();
        assert_eq!(naive, 6);
        assert_eq!(cluster_width(family), 2);
    }

    #[test]
    fn vs16_adds_no_cell_and_selects_emoji_presentation() {
        assert_eq!(cluster_width("\u{2764}"), 1);
        assert_eq!(cluster_width("\u{2764}\u{FE0F}"), 2);
        assert_eq!(char_width('\u{FE0F}'), Some(0));
    }

    #[test]
    fn skin_tone_joins_to_one_wide_cluster() {
        let naive: usize = "\u{1F44B}\u{1F3FD}"
            .chars()
            .filter_map(char_width)
            .sum();
        assert_eq!(naive, 4);
        assert_eq!(cluster_width("\u{1F44B}\u{1F3FD}"), 2);
    }
}
