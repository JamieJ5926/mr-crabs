use mr_crabs_effects::{CellPos, CellPx, EffectsConfig, EffectsModel, TextAnimation};
use mr_crabs_terminal::{Cell, CursorState, DamageKind, FrameDelta, GridSize, RowDelta};

fn row_at(row: u16, generation: u64, contents: &[u32]) -> RowDelta {
    RowDelta {
        row,
        generation,
        cells: contents
            .iter()
            .copied()
            .map(|content| Cell {
                content,
                style: 0,
                flags: 0,
            })
            .collect(),
        runs: Vec::new(),
    }
}

fn row(generation: u64, contents: &[u32]) -> RowDelta {
    row_at(0, generation, contents)
}
fn wide_pair_row(generation: u64) -> RowDelta {
    RowDelta {
        row: 0,
        generation,
        cells: vec![
            Cell {
                content: u32::from('界'),
                style: 0,
                flags: Cell::WIDE,
            },
            Cell {
                content: u32::from(' '),
                style: 0,
                flags: Cell::WIDE_SPACER,
            },
            Cell::default(),
            Cell::default(),
        ],
        runs: Vec::new(),
    }
}

fn frame_with_damage(
    size: GridSize,
    sequence: u64,
    damage: DamageKind,
    rows: Vec<RowDelta>,
) -> FrameDelta {
    let mut frame = FrameDelta::empty(size);
    frame.sequence = sequence;
    frame.damage = damage;
    frame.rows = rows;
    frame.cursor = CursorState::default();
    frame
}

fn frame(size: GridSize, sequence: u64, rows: Vec<RowDelta>) -> FrameDelta {
    let damage = if rows.is_empty() {
        DamageKind::Clean
    } else {
        DamageKind::Partial
    };
    frame_with_damage(size, sequence, damage, rows)
}

fn typewriter_model_for(size: GridSize, duration_ms: u64) -> EffectsModel {
    let config = EffectsConfig::new(
        TextAnimation::Typewriter,
        duration_ms,
        1.0,
        false,
        0.35,
        250,
        usize::from(size.cols) * usize::from(size.rows),
    );
    EffectsModel::new(config, size, CellPx::new(10.0, 20.0))
}

fn typewriter_model(duration_ms: u64) -> (GridSize, EffectsModel) {
    let size = GridSize::new(4, 1);
    (size, typewriter_model_for(size, duration_ms))
}
fn streaming_model(duration_ms: u64) -> (GridSize, EffectsModel) {
    let size = GridSize::new(4, 1);
    let config = EffectsConfig::new(
        TextAnimation::Streaming,
        duration_ms,
        1.0,
        false,
        0.35,
        250,
        usize::from(size.cols) * usize::from(size.rows),
    );
    (
        size,
        EffectsModel::new(config, size, CellPx::new(10.0, 20.0)),
    )
}

fn contents() -> [u32; 4] {
    [
        u32::from('A'),
        u32::from('B'),
        u32::from(' '),
        u32::from(' '),
    ]
}

fn reveal_positions(effect: &mr_crabs_effects::EffectsFrame) -> Vec<CellPos> {
    effect
        .revealing
        .iter()
        .map(|reveal| reveal.pos)
        .chain(effect.pending.iter().copied())
        .collect()
}

#[test]
fn repeated_identical_partial_frame_starts_a_fresh_typewriter_reveal() {
    let (size, mut model) = typewriter_model(120);
    let contents = contents();

    let first = frame(size, 1, vec![row(1, &contents)]);
    assert!(model.apply_frame(&first, 1_000, true).needs_frame);

    let expired = frame(size, 2, Vec::new());
    assert!(model.apply_frame(&expired, 1_165, true).is_idle());

    let repeated = frame(size, 3, vec![row(2, &contents)]);
    let effect = model.apply_frame(&repeated, 2_000, true);
    assert!(effect.text_reveal_allowed);
    assert!(effect.needs_frame);
    assert_eq!(
        effect
            .revealing
            .iter()
            .map(|cell| cell.pos)
            .collect::<Vec<_>>(),
        vec![CellPos::new(0, 0)]
    );
    assert_eq!(effect.pending, vec![CellPos::new(0, 1)]);
    assert_eq!(model.last_change_ms(), Some(2_015.0));

    let repeated_expired = frame(size, 4, Vec::new());
    assert!(model.apply_frame(&repeated_expired, 2_135, true).is_idle());

    let duplicate_generation = frame(size, 5, vec![row(2, &contents)]);
    assert!(
        model
            .apply_frame(&duplicate_generation, 3_000, true)
            .is_idle()
    );
}

#[test]
fn repeated_identical_wide_pair_restamps_full_span() {
    let (size, mut model) = streaming_model(120);

    let first = frame(size, 1, vec![wide_pair_row(1)]);
    assert!(model.apply_frame(&first, 1_000, true).needs_frame);

    let expired = frame(size, 2, Vec::new());
    assert!(model.apply_frame(&expired, 1_120, true).is_idle());

    let repeated = frame(size, 3, vec![wide_pair_row(2)]);
    let effect = model.apply_frame(&repeated, 2_000, true);
    let reveals = effect
        .revealing
        .iter()
        .map(|reveal| (reveal.pos, reveal.change_ms))
        .collect::<Vec<_>>();

    assert_eq!(
        reveals,
        vec![(CellPos::new(0, 0), 2_000.0), (CellPos::new(0, 1), 2_000.0),]
    );

    let duplicate_generation = frame(size, 4, vec![wide_pair_row(2)]);
    assert!(
        model
            .apply_frame(&duplicate_generation, 3_000, true)
            .is_idle()
    );
}

#[test]
fn five_cold_sequential_identical_executions_each_start_a_reveal() {
    let (size, mut model) = typewriter_model(600);
    let contents = contents();
    let mut sequence = 1;

    for execution in 0..5_u64 {
        let now_ms = 1_000 + execution * 1_000;
        let generation = execution + 1;
        let output = frame(size, sequence, vec![row(generation, &contents)]);
        let effect = model.apply_frame(&output, now_ms, true);

        assert!(effect.text_reveal_allowed, "execution {execution}");
        assert!(effect.needs_frame, "execution {execution}");
        assert!(effect.revealing.iter().any(|reveal| {
            reveal.pos == CellPos::new(0, 0) && reveal.change_ms == now_ms as f64
        }));
        assert!(
            effect.pending.contains(&CellPos::new(0, 1)),
            "execution {execution} must retain a pending second glyph"
        );

        let expiry_ms = model.last_change_ms().unwrap() as u64 + 600;
        let expired = frame(size, sequence + 1, Vec::new());
        assert!(
            model.apply_frame(&expired, expiry_ms, true).is_idle(),
            "execution {execution} must return to zero idle RAF"
        );
        sequence += 2;
    }
}

#[test]
fn third_overlapping_identical_execution_preserves_active_and_pending_reveals() {
    let (size, mut model) = typewriter_model(600);
    let contents = contents();

    let first = frame(size, 1, vec![row(1, &contents)]);
    assert!(model.apply_frame(&first, 1_000, true).needs_frame);

    let second = frame(size, 2, vec![row(2, &contents)]);
    let second_effect = model.apply_frame(&second, 1_040, true);
    assert!(second_effect.text_reveal_allowed);
    assert!(second_effect.needs_frame);

    let third = frame(size, 3, vec![row(3, &contents)]);
    let third_effect = model.apply_frame(&third, 1_080, true);
    assert!(third_effect.text_reveal_allowed);
    assert!(third_effect.needs_frame);
    assert!(third_effect.revealing.is_empty());
    assert_eq!(
        third_effect.pending,
        vec![
            CellPos::new(0, 0),
            CellPos::new(0, 1),
            CellPos::new(0, 2),
            CellPos::new(0, 3),
        ]
    );
    assert_eq!(model.last_change_ms(), Some(1_525.0));

    let cascade = frame(size, 4, Vec::new());
    let cascade_effect = model.apply_frame(&cascade, 1_450, true);
    assert!(cascade_effect.text_reveal_allowed);
    assert!(cascade_effect.needs_frame);
    assert!(
        cascade_effect
            .revealing
            .iter()
            .any(|reveal| reveal.pos == CellPos::new(0, 0))
    );
    assert!(cascade_effect.pending.contains(&CellPos::new(0, 1)));

    let expired = frame(size, 5, Vec::new());
    assert!(model.apply_frame(&expired, 2_125, true).is_idle());
}

#[test]
fn large_partial_bypass_preserves_existing_reveal_and_ignores_new_cells() {
    let size = GridSize::new(4, 17);
    let mut model = typewriter_model_for(size, 600);
    let contents = contents();

    let initial = frame(size, 1, vec![row_at(0, 1, &contents)]);
    assert!(model.apply_frame(&initial, 1_000, true).needs_frame);

    let mut rows = vec![row_at(0, 2, &contents)];
    for row_index in 1..size.rows {
        rows.push(row_at(
            row_index,
            1,
            &[
                u32::from('X'),
                u32::from(' '),
                u32::from(' '),
                u32::from(' '),
            ],
        ));
    }
    let large = frame_with_damage(size, 2, DamageKind::Partial, rows);
    let effect = model.apply_frame(&large, 1_040, true);

    assert!(effect.text_reveal_allowed);
    assert!(effect.needs_frame);
    assert_eq!(
        reveal_positions(effect),
        vec![
            CellPos::new(0, 0),
            CellPos::new(0, 1),
            CellPos::new(0, 2),
            CellPos::new(0, 3),
        ]
    );
    assert_eq!(model.last_change_ms(), Some(1_225.0));
}

#[test]
fn full_bypass_preserves_existing_reveal_for_unchanged_cells() {
    let size = GridSize::new(4, 2);
    let mut model = typewriter_model_for(size, 600);
    let contents = contents();
    let blank = [u32::from(' '); 4];

    let initial = frame(size, 1, vec![row_at(0, 1, &contents)]);
    assert!(model.apply_frame(&initial, 1_000, true).needs_frame);

    let full = frame_with_damage(
        size,
        2,
        DamageKind::Full,
        vec![row_at(0, 2, &contents), row_at(1, 1, &blank)],
    );
    let effect = model.apply_frame(&full, 1_040, true);

    assert!(effect.text_reveal_allowed);
    assert!(effect.needs_frame);
    assert_eq!(
        reveal_positions(effect),
        vec![
            CellPos::new(0, 0),
            CellPos::new(0, 1),
            CellPos::new(0, 2),
            CellPos::new(0, 3),
        ]
    );
    assert_eq!(model.last_change_ms(), Some(1_225.0));
}
