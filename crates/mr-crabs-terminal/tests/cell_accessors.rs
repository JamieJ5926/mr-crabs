use mr_crabs_terminal::{CellWidth, GridSize, Terminal, UnderlineStyle};

#[test]
fn cell_accessors_report_semantic_width_and_attributes() {
    let mut term = Terminal::new(GridSize::new(8, 2)).unwrap();
    term.feed("界\x1b[1;3;4;7;9mA".as_bytes())
        .expect("terminal feed");
    let snapshot = term.snapshot();

    assert_eq!(snapshot.cells[0].width(), CellWidth::Wide);
    assert_eq!(snapshot.cells[1].width(), CellWidth::WideSpacer);
    assert_eq!(snapshot.cells[2].width(), CellWidth::Single);

    let attributes = snapshot.cells[2].attributes();
    assert!(attributes.bold());
    assert!(attributes.italic());
    assert!(attributes.inverse());
    assert!(attributes.strikeout());
    assert_eq!(attributes.underline(), Some(UnderlineStyle::Single));
    assert!(!attributes.dim());
    assert!(!attributes.hidden());
    assert!(!attributes.wrapped());
    assert!(!attributes.combining());
}
