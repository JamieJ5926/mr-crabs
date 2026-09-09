use mr_crabs_terminal::{Cell, GridSize, Terminal};
use unicode_width::UnicodeWidthChar;

fn dump(label: &str, s: &str) {
    let mut term = Terminal::new(GridSize::new(40, 2)).expect("term");
    term.feed(s.as_bytes()).expect("feed");
    let snap = term.snapshot();
    println!("=== {label} {s:?}");
    println!(
        "  scalars={} cps={:?}",
        s.chars().count(),
        s.chars().map(|c| format!("U+{:04X}", c as u32)).collect::<Vec<_>>()
    );
    for (i, c) in s.chars().enumerate() {
        println!(
            "  scalar[{i}] U+{:04X} unicode_width={:?}",
            c as u32,
            UnicodeWidthChar::width(c)
        );
    }
    let mut occupied = 0usize;
    for (i, cell) in snap.cells.iter().take(40).enumerate() {
        if cell.is_default() {
            continue;
        }
        occupied += 1;
        let ch = char::from_u32(cell.content).unwrap_or('\u{FFFD}');
        println!(
            "  cell[{i}] content=U+{:04X} {ch:?} flags=0x{:04X} wide={} spacer={} combining={}",
            cell.content,
            cell.flags,
            cell.flags & Cell::WIDE != 0,
            cell.flags & Cell::WIDE_SPACER != 0,
            cell.flags & Cell::COMBINING != 0,
        );
    }
    println!("  occupied_cells={occupied} combining_marks={:?}", snap.combining_marks);
    println!("  cursor.col={}", snap.cursor.col);
}

fn main() {
    dump("party", "🎉");
    dump("flag_gb", "🇬🇧");
    dump("skin", "👋🏻");
    dump("family_zwj", "👨‍👩‍👧‍👦");
    dump("woman_zwj", "👩‍💻");
    dump("keycap", "1️⃣");
    dump("combining_acute", "e\u{0301}");
    dump("zwj_alone", "\u{200d}");
    dump("vs16_alone", "\u{fe0f}");
    dump("fitzpatrick_alone", "\u{1F3FB}");
    dump("regional_g", "\u{1F1EC}");
}
