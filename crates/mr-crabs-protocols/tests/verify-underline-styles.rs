use mr_crabs_protocols::{
    color::Rgb,
    sgr::{ColorSpec, SgrAttr, SgrState, UnderlineStyle},
};

fn printed(state: &SgrState) -> String {
    let mut out = Vec::new();
    state.print_attributes(&mut out);
    String::from_utf8(out).expect("sgr output is ascii")
}

#[test]
fn undercurl_tracks_as_4_3() {
    let mut s = SgrState::new();
    s.apply(SgrAttr::Underline(UnderlineStyle::Curly));
    assert_eq!(printed(&s), "0;4:3");
}

#[test]
fn dotted_tracks_as_4_4() {
    let mut s = SgrState::new();
    s.apply(SgrAttr::Underline(UnderlineStyle::Dotted));
    assert_eq!(printed(&s), "0;4:4");
}

#[test]
fn dashed_tracks_as_4_5() {
    let mut s = SgrState::new();
    s.apply(SgrAttr::Underline(UnderlineStyle::Dashed));
    assert_eq!(printed(&s), "0;4:5");
}

#[test]
fn double_tracks_as_4_2() {
    let mut s = SgrState::new();
    s.apply(SgrAttr::Underline(UnderlineStyle::Double));
    assert_eq!(printed(&s), "0;4:2");
}

#[test]
fn solid_tracks_as_bare_4() {
    let mut s = SgrState::new();
    s.apply(SgrAttr::Underline(UnderlineStyle::Solid));
    assert_eq!(printed(&s), "0;4");
}

#[test]
fn bold_survives_underline_apply() {
    let mut s = SgrState::new();
    s.apply(SgrAttr::Bold);
    s.apply(SgrAttr::Underline(UnderlineStyle::Curly));
    assert_eq!(printed(&s), "0;1;4:3");
    assert!(s.bold);
}

#[test]
fn faint_italic_strike_survive_underline() {
    let mut s = SgrState::new();
    s.apply(SgrAttr::Faint);
    s.apply(SgrAttr::Italic);
    s.apply(SgrAttr::Strikethrough);
    s.apply(SgrAttr::Underline(UnderlineStyle::Dotted));
    assert_eq!(printed(&s), "0;2;3;4:4;9");
    assert!(s.faint && s.italic && s.strikethrough);
}

#[test]
fn underline_apply_preserves_bold_faint_italic_strike() {
    let mut s = SgrState::new();
    for attr in [
        SgrAttr::Bold,
        SgrAttr::Faint,
        SgrAttr::Italic,
        SgrAttr::Strikethrough,
    ] {
        s.apply(attr);
    }
    s.apply(SgrAttr::Underline(UnderlineStyle::Dashed));
    assert!(s.bold && s.faint && s.italic && s.strikethrough);
    assert_eq!(s.underline, UnderlineStyle::Dashed);
}

#[test]
fn cancel_underline_clears_every_variant() {
    for style in [
        UnderlineStyle::Solid,
        UnderlineStyle::Double,
        UnderlineStyle::Curly,
        UnderlineStyle::Dotted,
        UnderlineStyle::Dashed,
    ] {
        let mut s = SgrState::new();
        s.apply(SgrAttr::Bold);
        s.apply(SgrAttr::Underline(style));
        s.apply(SgrAttr::NoUnderline);
        assert_eq!(s.underline, UnderlineStyle::None, "variant {style:?}");
        assert!(s.bold, "cancel underline keeps bold for {style:?}");
        assert_eq!(printed(&s), "0;1");
    }
}

#[test]
fn print_order_is_0_1_2_3_4x_53_5_7_8_9_colors() {
    let mut s = SgrState::new();
    for attr in [
        SgrAttr::Bold,
        SgrAttr::Faint,
        SgrAttr::Italic,
        SgrAttr::Underline(UnderlineStyle::Dashed),
        SgrAttr::Blink,
        SgrAttr::Inverse,
        SgrAttr::Invisible,
        SgrAttr::Strikethrough,
        SgrAttr::Overline,
        SgrAttr::Foreground(Some(ColorSpec::Rgb(Rgb {
            r: 255,
            g: 255,
            b: 255,
        }))),
        SgrAttr::Background(Some(ColorSpec::Rgb(Rgb {
            r: 255,
            g: 255,
            b: 255,
        }))),
    ] {
        s.apply(attr);
    }
    assert_eq!(
        printed(&s),
        "0;1;2;3;4:5;53;5;7;8;9;38:2::255:255:255;48:2::255:255:255"
    );
}

#[test]
fn print_order_holds_for_undercurl_with_bold_faint() {
    let mut s = SgrState::new();
    s.apply(SgrAttr::Bold);
    s.apply(SgrAttr::Faint);
    s.apply(SgrAttr::Italic);
    s.apply(SgrAttr::Underline(UnderlineStyle::Curly));
    assert_eq!(printed(&s), "0;1;2;3;4:3");
}

#[test]
fn underline_color_58_is_emitted() {
    let mut s = SgrState::new();
    s.apply(SgrAttr::Underline(UnderlineStyle::Curly));
    let text = printed(&s);
    assert!(
        text.contains("58"),
        "SgrState must emit SGR 58 for underline color; got {text:?} (slice b gap: no field, protocol drops)"
    );
}
