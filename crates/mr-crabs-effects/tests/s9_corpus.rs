//! S9 corpus gate: replay the frozen deterministic frame sequences from
//! `rust/verification/effects-corpus/s9-effects.json` through the public
//! `mr-crabs-effects` API and compare every observable value.

use mr_crabs_effects::{
    CellMetricsUniform, CellPx, EffectsConfig, EffectsModel, RevealMath, RowOrientation,
    TextAnimation,
};
use mr_crabs_terminal::{
    Cell, CursorShape, CursorState, DamageKind, FrameDelta, GridSize, RowDelta,
};

const CORPUS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../verification/effects-corpus/s9-effects.json"
);

const EPS: f64 = 1e-9;

fn assert_close(a: f64, b: f64, what: &str) {
    assert!(
        (a - b).abs() < EPS,
        "{what}: expected {b}, got {a} (diff {})",
        (a - b).abs()
    );
}

fn parse_mode(s: &str) -> TextAnimation {
    match s {
        "streaming" => TextAnimation::Streaming,
        "typewriter" => TextAnimation::Typewriter,
        "none" => TextAnimation::Disabled,
        other => panic!("unknown text_animation {other}"),
    }
}

fn parse_shape(s: &str) -> CursorShape {
    match s {
        "block" => CursorShape::Block,
        "bar" => CursorShape::Bar,
        "underline" => CursorShape::Underline,
        "hollow" => CursorShape::HollowBlock,
        other => panic!("unknown shape {other}"),
    }
}

fn parse_damage(s: &str) -> DamageKind {
    match s {
        "clean" => DamageKind::Clean,
        "partial" => DamageKind::Partial,
        "full" => DamageKind::Full,
        other => panic!("unknown damage {other}"),
    }
}

fn parse_cell(v: &serde_json::Value) -> Cell {
    Cell {
        content: v[0].as_u64().expect("content") as u32,
        style: v[1].as_u64().expect("style") as u16,
        flags: v[2].as_u64().expect("flags") as u16,
    }
}

fn parse_row(v: &serde_json::Value) -> RowDelta {
    RowDelta {
        row: v["row"].as_u64().expect("row") as u16,
        generation: v["generation"].as_u64().expect("generation"),
        cells: v["cells"]
            .as_array()
            .expect("cells")
            .iter()
            .map(parse_cell)
            .collect(),
        runs: Vec::new(),
    }
}

fn parse_cursor(v: &serde_json::Value) -> CursorState {
    CursorState {
        row: v["row"].as_u64().expect("cursor row") as u16,
        col: v["col"].as_u64().expect("cursor col") as u16,
        shape: parse_shape(v["shape"].as_str().expect("cursor shape")),
        visible: v["visible"].as_bool().expect("cursor visible"),
        ..CursorState::default()
    }
}

fn check_revealing(
    actual: &[mr_crabs_effects::CellReveal],
    expected: &serde_json::Value,
    math: &RevealMath,
    sample_x: f64,
    step: usize,
) {
    let exp = expected.as_array().expect("revealing array");
    assert_eq!(
        actual.len(),
        exp.len(),
        "step {step}: revealing length ({} vs {})",
        actual.len(),
        exp.len()
    );
    for (i, (a, e)) in actual.iter().zip(exp).enumerate() {
        assert_eq!(
            a.pos,
            mr_crabs_effects::CellPos::new(
                e["row"].as_u64().unwrap() as u16,
                e["col"].as_u64().unwrap() as u16
            ),
            "step {step} revealing[{i}] pos"
        );
        assert_close(
            a.change_ms,
            e["change_ms"].as_f64().unwrap(),
            &format!("step {step} revealing[{i}] change_ms"),
        );
        assert_close(
            a.elapsed_ms,
            e["elapsed_ms"].as_f64().unwrap(),
            &format!("step {step} revealing[{i}] elapsed_ms"),
        );
        assert_eq!(
            a.phase(math),
            match e["phase"].as_str().unwrap() {
                "animating" => mr_crabs_effects::RevealPhase::Animating,
                "pending" => mr_crabs_effects::RevealPhase::Pending,
                "revealed" => mr_crabs_effects::RevealPhase::Revealed,
                other => panic!("unknown phase {other}"),
            },
            "step {step} revealing[{i}] phase"
        );
        assert_close(
            a.boundary_fraction(math),
            e["boundary"].as_f64().unwrap(),
            &format!("step {step} revealing[{i}] boundary"),
        );
        assert_close(
            a.hidden_fraction_at(math, sample_x),
            e["hidden_at"].as_f64().unwrap(),
            &format!("step {step} revealing[{i}] hidden_at"),
        );
    }
}

fn check_trail(actual: mr_crabs_effects::TrailFrame, expected: &serde_json::Value, step: usize) {
    let exp_active = expected["active"].as_bool().unwrap();
    assert_eq!(actual.active, exp_active, "step {step} trail.active");
    let expected_echoes = expected["echoes"].as_array().expect("trail.echoes");
    assert_eq!(
        expected_echoes.len(),
        actual.echoes.len(),
        "step {step} trail.echoes.len"
    );
    for (i, (actual, expected)) in actual.echoes.iter().zip(expected_echoes).enumerate() {
        let rect = expected["rect"].as_array().expect("trail echo rect");
        assert_close(
            actual.rect.x,
            rect[0].as_f64().unwrap(),
            &format!("step {step} trail.echoes[{i}].rect.x"),
        );
        assert_close(
            actual.rect.y,
            rect[1].as_f64().unwrap(),
            &format!("step {step} trail.echoes[{i}].rect.y"),
        );
        assert_close(
            actual.rect.w,
            rect[2].as_f64().unwrap(),
            &format!("step {step} trail.echoes[{i}].rect.w"),
        );
        assert_close(
            actual.rect.h,
            rect[3].as_f64().unwrap(),
            &format!("step {step} trail.echoes[{i}].rect.h"),
        );
        assert_close(
            actual.alpha,
            expected["alpha"].as_f64().unwrap(),
            &format!("step {step} trail.echoes[{i}].alpha"),
        );
    }
    if !exp_active {
        assert_eq!(actual.alpha, 0.0, "step {step} trail.alpha");
        return;
    }
    assert_close(
        actual.alpha,
        expected["alpha"].as_f64().unwrap(),
        &format!("step {step} trail.alpha"),
    );
    assert_close(
        actual.elapsed_ms,
        expected["elapsed_ms"].as_f64().unwrap(),
        &format!("step {step} trail.elapsed_ms"),
    );
    assert_close(
        actual.radius_px,
        expected["radius"].as_f64().unwrap(),
        &format!("step {step} trail.radius"),
    );
    let glow = expected["glow"].as_array().expect("glow");
    assert_close(
        actual.glow_rect.x,
        glow[0].as_f64().unwrap(),
        &format!("step {step} trail.glow.x"),
    );
    assert_close(
        actual.glow_rect.y,
        glow[1].as_f64().unwrap(),
        &format!("step {step} trail.glow.y"),
    );
    assert_close(
        actual.glow_rect.w,
        glow[2].as_f64().unwrap(),
        &format!("step {step} trail.glow.w"),
    );
    assert_close(
        actual.glow_rect.h,
        glow[3].as_f64().unwrap(),
        &format!("step {step} trail.glow.h"),
    );
}

fn run_sequence_case(case: &serde_json::Value, id: &str) {
    let cfg_json = &case["config"];
    let config = EffectsConfig::new(
        parse_mode(cfg_json["text_animation"].as_str().unwrap()),
        cfg_json["text_animation_duration_ms"].as_u64().unwrap(),
        cfg_json["text_animation_intensity"].as_f64().unwrap(),
        cfg_json["cursor_trail"].as_bool().unwrap(),
        cfg_json["cursor_trail_opacity"].as_f64().unwrap(),
        cfg_json["cursor_trail_duration_ms"].as_u64().unwrap(),
        cfg_json["max_tracked_cells"].as_u64().unwrap() as usize,
    );
    let size = GridSize::new(
        case["grid"]["cols"].as_u64().unwrap() as u16,
        case["grid"]["rows"].as_u64().unwrap() as u16,
    );
    let cell = CellPx::new(
        case["cell_px"]["width"].as_f64().unwrap(),
        case["cell_px"]["height"].as_f64().unwrap(),
    );
    let sample_x = case["sample_x"].as_f64().unwrap_or(9.0);
    let math = RevealMath::new(
        config.text_animation,
        config.text_animation_duration_ms,
        config.text_animation_intensity,
        cell.width,
    );

    let mut model = EffectsModel::new(config, size, cell);
    let steps = case["steps"].as_array().expect("steps");
    for (step_idx, step) in steps.iter().enumerate() {
        let mut frame = FrameDelta::empty(size);
        frame.sequence = step_idx as u64;
        frame.damage = parse_damage(step["damage"].as_str().unwrap());
        frame.rows = step["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(parse_row)
            .collect();
        frame.cursor = parse_cursor(&step["cursor"]);

        let actual = model.apply_frame(
            &frame,
            step["now_ms"].as_u64().unwrap(),
            step["focus"].as_bool().unwrap(),
        );
        let exp = &step["expected"];
        assert_eq!(
            actual.needs_frame,
            exp["needs_frame"].as_bool().unwrap(),
            "{id} step {step_idx}: needs_frame"
        );
        check_revealing(
            &actual.revealing,
            &exp["revealing"],
            &math,
            sample_x,
            step_idx,
        );
        let exp_pending = exp["pending"].as_array().expect("pending");
        assert_eq!(
            actual.pending.len(),
            exp_pending.len(),
            "{id} step {step_idx}: pending length"
        );
        for (i, (a, e)) in actual.pending.iter().zip(exp_pending).enumerate() {
            assert_eq!(
                a,
                &mr_crabs_effects::CellPos::new(
                    e["row"].as_u64().unwrap() as u16,
                    e["col"].as_u64().unwrap() as u16
                ),
                "{id} step {step_idx}: pending[{i}]"
            );
        }
        check_trail(actual.trail, &exp["trail"], step_idx);
    }

    if let Some(len) = case.get("expected_texture_len").and_then(|v| v.as_u64()) {
        assert_eq!(
            model.change_texture().len(),
            len as usize,
            "{id}: change texture length"
        );
    }
    if let Some(zero) = case
        .get("expected_retained_capacity")
        .and_then(|v| v.as_u64())
    {
        assert_eq!(
            model.retained_capacity(),
            zero as usize,
            "{id}: disabled path retains nothing"
        );
    }
}

fn run_coords_case(case: &serde_json::Value, id: &str) {
    let orientation = match case["orientation"].as_str().unwrap() {
        "top_down" => RowOrientation::TopDown,
        "bottom_up" => RowOrientation::BottomUp,
        other => panic!("unknown orientation {other}"),
    };
    let cell = CellPx::new(
        case["cell_px"]["width"].as_f64().unwrap(),
        case["cell_px"]["height"].as_f64().unwrap(),
    );
    let metrics = CellMetricsUniform::new(
        orientation,
        cell,
        case["padding_top"].as_f64().unwrap(),
        case["padding_left"].as_f64().unwrap(),
        case["screen_height"].as_f64().unwrap(),
    );
    let exp = &case["expected"];
    assert_close(
        metrics.row_step_px,
        exp["row_step"].as_f64().unwrap(),
        &format!("{id} row_step"),
    );
    assert_close(
        metrics.row_zero_origin_x,
        exp["origin_x"].as_f64().unwrap(),
        &format!("{id} origin_x"),
    );
    assert_close(
        metrics.row_zero_origin_y,
        exp["origin_y"].as_f64().unwrap(),
        &format!("{id} origin_y"),
    );
    for (i, m) in exp["mappings"].as_array().unwrap().iter().enumerate() {
        let fx = m[0].as_f64().unwrap();
        let fy = m[1].as_f64().unwrap();
        let want_row = m[2].as_i64().unwrap();
        let want_col = m[3].as_i64().unwrap();
        let got = metrics.cell_coord(fx, fy);
        let got = got.map(|(c, r)| (c as i64, r as i64));
        assert_eq!(
            got,
            Some((want_col, want_row)).filter(|(c, r)| *c >= 0 && *r >= 0),
            "{id} mapping[{i}] ({fx}, {fy})"
        );
    }
}

#[test]
fn s9_effects_corpus_matches_fixtures() {
    let text = std::fs::read_to_string(CORPUS_PATH).expect("s9-effects.json");
    let corpus: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    let cases = corpus["cases"].as_array().expect("cases array");
    assert!(cases.len() >= 10, "corpus must cover at least 10 cases");
    for case in cases {
        let id = case["id"].as_str().expect("case id");
        match case["kind"].as_str().expect("case kind") {
            "sequence" => run_sequence_case(case, id),
            "coords" => run_coords_case(case, id),
            other => panic!("unknown case kind {other}"),
        }
    }
}
