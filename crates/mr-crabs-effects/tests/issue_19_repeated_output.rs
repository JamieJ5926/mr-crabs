use mr_crabs_effects::{CellPos, CellPx, EffectsConfig, EffectsModel, TextAnimation};
use mr_crabs_terminal::{Cell, CursorState, DamageKind, FrameDelta, GridSize, RowDelta};

fn row(generation: u64, contents: &[u32]) -> RowDelta {
    RowDelta {
        row: 0,
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

#[test]
fn repeated_identical_partial_frame_starts_a_fresh_typewriter_reveal() {
    let size = GridSize::new(4, 1);
    let config = EffectsConfig::new(TextAnimation::Typewriter, 120, 1.0, false, 0.35, 250, 16);
    let mut model = EffectsModel::new(config, size, CellPx::new(10.0, 20.0));
    let contents = [
        u32::from('A'),
        u32::from('B'),
        u32::from(' '),
        u32::from(' '),
    ];

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
