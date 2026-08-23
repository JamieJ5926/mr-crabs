use mr_crabs_effects::{CellPos, CellPx, EffectsConfig, EffectsModel, TextAnimation};
use mr_crabs_terminal::{Cell, CursorState, DamageKind, FrameDelta, GridSize, RowDelta};

fn row(row: u16, generation: u64, text: &str, cols: u16) -> RowDelta {
    let mut cells = text
        .chars()
        .map(|ch| Cell {
            content: u32::from(ch),
            style: 0,
            flags: 0,
        })
        .collect::<Vec<_>>();
    cells.resize(usize::from(cols), Cell::default());
    RowDelta {
        row,
        generation,
        cells,
        runs: Vec::new(),
    }
}

fn frame(size: GridSize, sequence: u64, rows: Vec<RowDelta>) -> FrameDelta {
    let mut frame = FrameDelta::empty(size);
    frame.sequence = sequence;
    frame.damage = if rows.is_empty() {
        DamageKind::Clean
    } else {
        DamageKind::Partial
    };
    frame.rows = rows;
    frame.cursor = CursorState::default();
    frame
}

fn model(size: GridSize, duration_ms: u64) -> EffectsModel {
    EffectsModel::new(
        EffectsConfig::new(
            TextAnimation::Typewriter,
            duration_ms,
            1.0,
            false,
            0.35,
            250,
            usize::from(size.cols) * usize::from(size.rows),
        ),
        size,
        CellPx::new(10.0, 20.0),
    )
}

#[test]
fn output_row_starts_without_waiting_for_command_row() {
    let size = GridSize::new(8, 2);
    let mut model = model(size, 600);
    let update = frame(
        size,
        1,
        vec![row(0, 1, "command!", size.cols), row(1, 1, "界", size.cols)],
    );
    let effects = model.apply_frame(&update, 1_000, true);

    assert!(
        effects
            .revealing
            .iter()
            .any(|reveal| { reveal.pos == CellPos::new(0, 0) && reveal.change_ms == 1_000.0 })
    );
    assert!(
        effects
            .revealing
            .iter()
            .any(|reveal| { reveal.pos == CellPos::new(1, 0) && reveal.change_ms == 1_000.0 })
    );
    assert!(model.last_change_ms().is_some_and(|last| last <= 1_600.0));
}

#[test]
fn later_output_row_resets_cross_build_backlog() {
    let size = GridSize::new(16, 2);
    let mut model = model(size, 600);
    let command = frame(size, 1, vec![row(0, 1, "long command echo", size.cols)]);
    assert!(model.apply_frame(&command, 1_000, true).needs_frame);

    let output = frame(size, 2, vec![row(1, 1, "result", size.cols)]);
    let effects = model.apply_frame(&output, 1_040, true);
    assert!(
        effects
            .revealing
            .iter()
            .any(|reveal| { reveal.pos == CellPos::new(1, 0) && reveal.change_ms == 1_040.0 })
    );
}

#[test]
fn long_row_never_schedules_beyond_one_duration() {
    let size = GridSize::new(80, 1);
    let mut model = model(size, 120);
    let update = frame(size, 1, vec![row(0, 1, &"x".repeat(80), size.cols)]);
    assert!(model.apply_frame(&update, 5_000, true).needs_frame);
    assert!(model.last_change_ms().is_some_and(|last| last <= 5_120.0));
}

#[test]
fn five_sequential_command_output_cycles_start_output_immediately() {
    let size = GridSize::new(16, 2);
    let mut model = model(size, 600);

    for execution in 0..5_u64 {
        let now = 1_000 + execution * 1_000;
        let generation = execution + 1;
        let update = frame(
            size,
            execution + 1,
            vec![
                row(0, generation, "long command echo", size.cols),
                row(1, generation, "result", size.cols),
            ],
        );
        let effects = model.apply_frame(&update, now, true);
        assert!(
            effects.revealing.iter().any(|reveal| {
                reveal.pos == CellPos::new(1, 0) && reveal.change_ms == now as f64
            })
        );
    }
}

#[test]
fn three_overlapping_distinct_rows_each_start_at_arrival() {
    let size = GridSize::new(16, 3);
    let mut model = model(size, 600);

    for (sequence, row_index, now) in [(1, 0, 1_000), (2, 1, 1_040), (3, 2, 1_080)] {
        let update = frame(
            size,
            sequence,
            vec![row(row_index, sequence, "result", size.cols)],
        );
        let effects = model.apply_frame(&update, now, true);
        assert!(effects.revealing.iter().any(|reveal| {
            reveal.pos == CellPos::new(row_index, 0) && reveal.change_ms == now as f64
        }));
    }
}
