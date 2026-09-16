use mr_crabs_terminal::{Cell, GridSize, Terminal};

fn new_term() -> Terminal {
    Terminal::new(GridSize::new(10, 4)).unwrap()
}

fn row_contents(snap: &mr_crabs_terminal::NormalizedSnapshot, row: usize) -> Vec<u32> {
    let cols = snap.size.cols as usize;
    snap.cells[row * cols..(row + 1) * cols]
        .iter()
        .map(|c| c.content)
        .collect()
}

fn row0_marks(snap: &mr_crabs_terminal::NormalizedSnapshot) -> Vec<&mr_crabs_terminal::CombiningMarks> {
    let cols = snap.size.cols as usize;
    snap.combining_marks
        .iter()
        .filter(|m| (m.cell_index as usize) < cols)
        .collect()
}

#[test]
fn family_zwj_is_one_wide_cell() {
    let mut term = new_term();
    term.feed("👨\u{200D}👩\u{200D}👧".as_bytes()).unwrap();
    let snap = term.snapshot();
    assert_eq!(snap.cursor.col, 2, "cursor {snap:?}");
    assert_eq!(snap.cells[0].content, u32::from('👨'));
    assert_ne!(snap.cells[0].flags & Cell::WIDE, 0);
    assert_ne!(snap.cells[1].flags & Cell::WIDE_SPACER, 0);
    assert_eq!(snap.cells[2].content, u32::from(' '), "row: {:?}", row_contents(&snap, 0));
    assert_eq!(snap.cells[4].content, u32::from(' '), "second wide cell leaked");
    let marks = row0_marks(&snap);
    assert_eq!(marks.len(), 1, "marks: {:?}", snap.combining_marks);
    assert_eq!(
        marks[0].codepoints,
        vec![0x200D, u32::from('👩'), 0x200D, u32::from('👧')]
    );
}

#[test]
fn wave_skin_tone_is_one_wide_cell() {
    let mut term = new_term();
    term.feed("👋🏽".as_bytes()).unwrap();
    let snap = term.snapshot();
    assert_eq!(snap.cursor.col, 2);
    assert_eq!(snap.cells[0].content, u32::from('👋'));
    assert_ne!(snap.cells[0].flags & Cell::WIDE, 0);
    assert_ne!(snap.cells[1].flags & Cell::WIDE_SPACER, 0);
    assert_eq!(snap.cells[2].content, u32::from(' '));
    let marks = row0_marks(&snap);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].codepoints, vec![0x1F3FD]);
}

#[test]
fn flag_pair_is_one_wide_cell() {
    let mut term = new_term();
    term.feed("🇺🇸".as_bytes()).unwrap();
    let snap = term.snapshot();
    assert_eq!(snap.cursor.col, 2);
    assert_eq!(snap.cells[0].content, u32::from('🇺'));
    assert_ne!(snap.cells[0].flags & Cell::WIDE, 0);
    assert_ne!(snap.cells[1].flags & Cell::WIDE_SPACER, 0);
    assert_eq!(snap.cells[2].content, u32::from(' '));
    let marks = row0_marks(&snap);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].codepoints, vec![0x1F1F8]);
}

#[test]
fn heart_vs16_is_one_cluster() {
    let mut term = new_term();
    term.feed("❤️".as_bytes()).unwrap();
    let snap = term.snapshot();
    assert_eq!(snap.cursor.col, 2, "cursor {snap:?}");
    assert_eq!(snap.cells[0].content, u32::from('❤'));
    assert_ne!(snap.cells[0].flags & Cell::WIDE, 0);
    assert_ne!(snap.cells[1].flags & Cell::WIDE_SPACER, 0);
    let marks = row0_marks(&snap);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].codepoints, vec![0xFE0F]);
}

#[test]
fn ascii_after_cluster_lands_adjacent() {
    let mut term = new_term();
    term.feed("👨\u{200D}👩\u{200D}👧X".as_bytes()).unwrap();
    let snap = term.snapshot();
    assert_eq!(snap.cursor.col, 3);
    assert_eq!(snap.cells[2].content, u32::from('X'));
}

#[test]
fn newline_after_cluster_starts_next_row() {
    let mut term = new_term();
    term.feed("👨\u{200D}👩\u{200D}👧\r\nY".as_bytes()).unwrap();
    let snap = term.snapshot();
    assert_eq!(snap.cursor.row, 1);
    assert_eq!(snap.cursor.col, 1);
    assert_eq!(snap.cells[0].content, u32::from('👨'));
    let cols = snap.size.cols as usize;
    assert_eq!(snap.cells[cols].content, u32::from('Y'));
}

#[test]
fn snapshot_mid_cluster_then_continue() {
    let mut term = new_term();
    term.feed("👨\u{200D}".as_bytes()).unwrap();
    let mid = term.snapshot();
    assert_eq!(mid.cursor.col, 2);
    term.feed("👩\u{200D}👧".as_bytes()).unwrap();
    let snap = term.snapshot();
    assert_eq!(snap.cursor.col, 2);
    assert_eq!(snap.cells[2].content, u32::from(' '), "row: {:?}", row_contents(&snap, 0));
    let marks = row0_marks(&snap);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].codepoints.len(), 4);
}

#[test]
fn controls_unchanged() {
    let mut term = new_term();
    term.feed("A".as_bytes()).unwrap();
    let snap = term.snapshot();
    assert_eq!(snap.cursor.col, 1);
    assert_eq!(snap.cells[0].content, u32::from('A'));
    let mut term = new_term();
    term.feed("中".as_bytes()).unwrap();
    let snap = term.snapshot();
    assert_eq!(snap.cursor.col, 2);
    assert_eq!(snap.cells[0].content, u32::from('中'));
    assert_ne!(snap.cells[0].flags & Cell::WIDE, 0);
    let mut term = new_term();
    term.feed("e\u{0301}".as_bytes()).unwrap();
    let snap = term.snapshot();
    assert_eq!(snap.cursor.col, 1);
    assert_eq!(snap.cells[0].content, u32::from('e'));
    let marks = row0_marks(&snap);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].codepoints, vec![0x0301]);
}
