use mr_crabs_terminal::{Cell, UnderlineStyle as TermUnderline};

const UNDERLINE: u16 = 0x0008;
const DOUBLE: u16 = 0x0800;
const UNDERCURL: u16 = 0x1000;
const DOTTED: u16 = 0x2000;
const DASHED: u16 = 0x4000;
const ALL: u16 = 0x7808;

const BOLD: u16 = 0x0002;
const ITALIC: u16 = 0x0004;
const DIM: u16 = 0x0080;
const STRIKEOUT: u16 = 0x0200;

fn cell_with(flags: u16) -> Cell {
    Cell {
        content: u32::from('x'),
        style: 0,
        flags,
    }
}

fn current_paint_markers(flags: u16) -> (bool, bool) {
    let has_underline = flags & ALL != 0;
    let wavy = flags & UNDERCURL != 0;
    (has_underline, wavy)
}

#[test]
fn cell_attributes_distinguishes_all_underline_variants() {
    assert_eq!(
        cell_with(UNDERLINE).attributes().underline(),
        Some(TermUnderline::Single)
    );
    assert_eq!(
        cell_with(DOUBLE).attributes().underline(),
        Some(TermUnderline::Double)
    );
    assert_eq!(
        cell_with(UNDERCURL).attributes().underline(),
        Some(TermUnderline::Curly)
    );
    assert_eq!(
        cell_with(DOTTED).attributes().underline(),
        Some(TermUnderline::Dotted)
    );
    assert_eq!(
        cell_with(DASHED).attributes().underline(),
        Some(TermUnderline::Dashed)
    );
    assert_eq!(cell_with(0).attributes().underline(), None);
}

#[test]
fn undercurl_flag_is_curly() {
    assert_eq!(
        cell_with(UNDERCURL).attributes().underline(),
        Some(TermUnderline::Curly)
    );
}

#[test]
fn flags_compose_bold_dim_italic_strike_with_each_underline() {
    for underline in [UNDERLINE, DOUBLE, UNDERCURL, DOTTED, DASHED] {
        let flags = BOLD | DIM | ITALIC | STRIKEOUT | underline;
        let attrs = cell_with(flags).attributes();
        assert!(attrs.bold(), "bold survives underline {underline:#06x}");
        assert!(attrs.dim(), "dim survives underline {underline:#06x}");
        assert!(attrs.italic(), "italic survives underline {underline:#06x}");
        assert!(
            attrs.strikeout(),
            "strike survives underline {underline:#06x}"
        );
        assert!(
            attrs.underline().is_some(),
            "underline present with modifiers {underline:#06x}"
        );
    }
}

#[test]
fn clearing_all_underlines_preserves_modifiers() {
    let flags = BOLD | DIM | ITALIC | STRIKEOUT | DOTTED;
    let cleared = flags & !ALL;
    let attrs = cell_with(cleared).attributes();
    assert_eq!(attrs.underline(), None);
    assert!(attrs.bold());
    assert!(attrs.dim());
    assert!(attrs.italic());
    assert!(attrs.strikeout());
}

#[test]
fn paint_collapse_dotted_dashed_to_solid() {
    let (solid_has, solid_wavy) = current_paint_markers(UNDERLINE);
    let (dotted_has, dotted_wavy) = current_paint_markers(DOTTED);
    let (dashed_has, dashed_wavy) = current_paint_markers(DASHED);
    let (double_has, double_wavy) = current_paint_markers(DOUBLE);
    let (_, curl_wavy) = current_paint_markers(UNDERCURL);
    assert!(solid_has && dotted_has && dashed_has && double_has);
    assert!(
        curl_wavy,
        "undercurl sets wavy in element.rs:870-876 (slice a keeps this)"
    );
    assert!(
        dotted_wavy || dashed_wavy || double_wavy || solid_wavy,
        "paint collapses dotted/dashed/double to solid: all render wavy=false \
         (element.rs:870-876, slice a gap). solid={solid_wavy} dotted={dotted_wavy} \
         dashed={dashed_wavy} double={double_wavy}"
    );
}
