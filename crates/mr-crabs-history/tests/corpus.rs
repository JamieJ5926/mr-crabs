//! S8 behavior corpus runner: `verification/history-corpus/s8-history.json`.
//!
//! Every case kind maps to one production API (search, viewport, selection,
//! persist, restore, replay, alt-screen, resize/reflow); the corpus pins the
//! deterministic behavior across compressed and uncompressed page
//! boundaries. Skipped gracefully when the corpus file is absent (CI).

use mr_crabs_history::{
    ExtractOptions, HistoryFile, PersistConfig, PersistError, ReplayLog, SearchDirection,
    SearchRequest, SearchStart, Selection, SelectionGesture, SelectionPoint, TerminalSnapshot,
    row_text, search_sync, selection_text, viewport_row, visible_rows,
};
use mr_crabs_terminal::{GridSize, ScrollbackConfig, Terminal};
use serde_json::{Value, json};

fn corpus_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../verification/history-corpus/s8-history.json")
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    let mut out = Vec::with_capacity(hex.len() / 2);
    let mut chars = hex.chars();
    while let (Some(hi), Some(lo)) = (chars.next(), chars.next()) {
        let byte = u8::from_str_radix(&format!("{hi}{lo}"), 16).expect("hex");
        out.push(byte);
    }
    out
}

fn build_term(size: &Value) -> Terminal {
    Terminal::new_with_config(
        GridSize::new(
            size["cols"].as_u64().expect("cols") as u16,
            size["rows"].as_u64().expect("rows") as u16,
        ),
        ScrollbackConfig {
            max_lines: 1000,
            hot_page_lines: 2,
            max_queued_jobs: 8,
            max_pending_completions: 8,
        },
    )
    .expect("terminal")
}

fn run_search_case(case: &Value) {
    let mut term = build_term(&case["size"]);
    term.feed(&hex_to_bytes(
        case["input_hex"].as_str().expect("input_hex"),
    ));
    if case["compress"].as_bool().unwrap_or(false) {
        term.force_compress_all();
    }
    let req_value = &case["request"];
    let needle = req_value["needle"]
        .as_str()
        .expect("needle")
        .as_bytes()
        .to_vec();
    let direction = match req_value["direction"].as_str().expect("direction") {
        "forward" => SearchDirection::Forward,
        "reverse" => SearchDirection::Reverse,
        other => panic!("bad direction {other}"),
    };
    let start = match req_value["start"].as_str().expect("start") {
        "top" => SearchStart::Top,
        "bottom" => SearchStart::Bottom,
        other => {
            let line = other
                .strip_prefix("line:")
                .expect("line:N")
                .parse::<usize>()
                .expect("line index");
            SearchStart::Line(line)
        }
    };
    let mut request = SearchRequest {
        needle,
        direction,
        start,
        limit: req_value["limit"].as_u64().expect("limit") as usize,
        case_sensitive: req_value["case_sensitive"].as_bool().unwrap_or(false),
        visible_rows: Vec::new(),
    };
    if req_value["include_visible"].as_bool().unwrap_or(true) {
        request.visible_rows = visible_rows(&term.snapshot());
    }
    let outcome = search_sync(&mut term, &request, 1);
    let actual = json!({
        "matches": outcome.matches,
        "truncated": outcome.truncated,
        "completed": outcome.completed,
    });
    assert_eq!(
        actual,
        case["expect"],
        "search case {} mismatch",
        case["name"].as_str().unwrap()
    );
}

fn run_viewport_case(case: &Value) {
    let mut term = build_term(&case["size"]);
    term.feed(&hex_to_bytes(
        case["input_hex"].as_str().expect("input_hex"),
    ));
    if case["compress"].as_bool().unwrap_or(false) {
        term.force_compress_all();
    }
    let snap = term.snapshot();
    let mut viewport = mr_crabs_history::Viewport::new();
    viewport.scroll_up(
        case["viewport"]["offset"].as_u64().expect("offset") as usize,
        term.history_len(),
    );
    let row = case["viewport"]["row"].as_u64().expect("row") as u16;
    let row_value = viewport_row(&mut term, &snap, &viewport, row).expect("viewport row");
    let text = String::from_utf8(row_text(&row_value.cells)).expect("utf8");
    let actual = json!({
        "absolute": row_value.absolute,
        "cols": row_value.cols,
        "text": text,
    });
    assert_eq!(
        actual,
        case["expect"],
        "viewport case {} mismatch",
        case["name"].as_str().unwrap()
    );
}

fn run_selection_case(case: &Value) {
    let mut term = build_term(&case["size"]);
    term.feed(&hex_to_bytes(
        case["input_hex"].as_str().expect("input_hex"),
    ));
    let snap = term.snapshot();
    let visible = visible_rows(&snap);
    let sel_value = &case["selection"];
    let gesture = match sel_value["gesture"].as_str().expect("gesture") {
        "cell" => SelectionGesture::Cell,
        "word" => SelectionGesture::Word,
        "line" => SelectionGesture::Line,
        "block" => SelectionGesture::Block,
        other => panic!("bad gesture {other}"),
    };
    let anchor = &sel_value["anchor"];
    let active = &sel_value["active"];
    let selection = Selection::new(
        gesture,
        SelectionPoint {
            line: anchor["line"].as_u64().expect("line") as usize,
            col: anchor["col"].as_u64().expect("col") as u16,
        },
        SelectionPoint {
            line: active["line"].as_u64().expect("line") as usize,
            col: active["col"].as_u64().expect("col") as u16,
        },
    );
    let visible_owned = visible;
    let text = selection_text(
        |line| {
            let history_len = term.history_len();
            if line < history_len {
                let mut cells = Vec::new();
                term.read_history_line(line, &mut cells).then_some(cells)
            } else {
                visible_owned.get(line - history_len).cloned()
            }
        },
        &selection,
        ExtractOptions::default(),
    );
    let actual = json!({ "text": text });
    assert_eq!(
        actual,
        case["expect"],
        "selection case {} mismatch",
        case["name"].as_str().unwrap()
    );
}

fn run_persist_case(case: &Value) {
    let max_bytes = case["max_bytes"].as_u64().unwrap_or(64 * 1024 * 1024) as usize;
    let config = PersistConfig {
        max_bytes,
        ..PersistConfig::default()
    };
    if let Some(payload_hex) = case["payload_hex"].as_str() {
        let payload = hex_to_bytes(payload_hex);
        let result = HistoryFile::decode(&payload, &config).map(|_| ());
        let expected_error = case["expect"]["decode_error"]
            .as_str()
            .expect("decode_error");
        let actual_error = match result {
            Ok(()) => panic!(
                "persist case {}: expected rejection, decoded successfully",
                case["name"].as_str().unwrap()
            ),
            Err(PersistError::TooLarge) => "too_large",
            Err(PersistError::TooManyLines) => "too_many_lines",
            Err(PersistError::VersionMismatch(_)) => "version_mismatch",
            Err(PersistError::BadMagic) => "bad_magic",
            Err(PersistError::Corrupt) => "corrupt",
            Err(PersistError::Truncated) => "truncated",
        };
        assert_eq!(
            actual_error,
            expected_error,
            "persist case {} error mismatch",
            case["name"].as_str().unwrap()
        );
        return;
    }
    let mut term = build_term(&case["size"]);
    term.feed(&hex_to_bytes(
        case["input_hex"].as_str().expect("input_hex"),
    ));
    let file = HistoryFile::capture(&mut term, &config).expect("capture");
    let encoded = file.encode(&config).expect("encode");
    let decoded = HistoryFile::decode(&encoded, &config).expect("decode");
    assert_eq!(
        decoded,
        file,
        "persist case {} roundtrip mismatch",
        case["name"].as_str().unwrap()
    );
    assert_eq!(
        decoded.lines.len(),
        case["expect"]["lines"].as_u64().expect("lines") as usize
    );
}

fn run_restore_case(case: &Value) {
    let mut term = build_term(&case["size"]);
    term.feed(&hex_to_bytes(
        case["input_hex"].as_str().expect("input_hex"),
    ));
    let before = term.snapshot();
    let snapshot = TerminalSnapshot::capture(&mut term, 1000).expect("capture");
    let mut restored = Terminal::new(snapshot.size).expect("fresh terminal");
    snapshot.restore(&mut restored).expect("restore");
    let after = restored.snapshot();
    assert_eq!(
        after,
        before,
        "restore case {} mismatch",
        case["name"].as_str().unwrap()
    );
    assert_eq!(restored.history_len(), term.history_len());
}

fn run_replay_case(case: &Value) {
    let mut term = build_term(&case["size"]);
    term.feed(&hex_to_bytes(
        case["input_hex"].as_str().expect("input_hex"),
    ));
    let snapshot = TerminalSnapshot::capture(&mut term, 1000).expect("capture");
    let mut log = ReplayLog::new(snapshot);
    for event in case["events"].as_array().expect("events") {
        if let Some(feed) = event["feed"].as_str() {
            log.record_feed(&hex_to_bytes(feed), 64 * 1024 * 1024)
                .expect("record feed");
        } else if let Some(resize) = event.get("resize") {
            log.record_resize(
                GridSize::new(
                    resize["cols"].as_u64().expect("cols") as u16,
                    resize["rows"].as_u64().expect("rows") as u16,
                ),
                64 * 1024 * 1024,
            )
            .expect("record resize");
        }
    }
    let mut fresh = Terminal::new(log.start.size).expect("fresh");
    assert!(
        log.verify(&mut fresh, 1000).expect("verify"),
        "replay case {} final state diverged",
        case["name"].as_str().unwrap()
    );
}

fn run_altscreen_case(case: &Value) {
    let mut term = build_term(&case["size"]);
    term.feed(&hex_to_bytes(
        case["input_hex"].as_str().expect("input_hex"),
    ));
    let snap = term.snapshot();
    assert_eq!(
        term.history_len(),
        case["expect"]["history_len"].as_u64().expect("history_len") as usize,
        "altscreen case {} history length",
        case["name"].as_str().unwrap()
    );
    let row0 = row_text(&snap.cells[..usize::from(snap.size.cols)]);
    assert_eq!(
        String::from_utf8(row0).expect("utf8"),
        case["expect"]["visible_text"]
            .as_str()
            .expect("visible_text"),
        "altscreen case {} visible row 0",
        case["name"].as_str().unwrap()
    );
}

fn run_resize_case(case: &Value) {
    let mut term = build_term(&case["size"]);
    term.feed(&hex_to_bytes(
        case["input_hex"].as_str().expect("input_hex"),
    ));
    for resize in case["resizes"].as_array().expect("resizes") {
        term.resize(GridSize::new(
            resize["cols"].as_u64().expect("cols") as u16,
            resize["rows"].as_u64().expect("rows") as u16,
        ))
        .expect("resize");
    }
    if let Some(after_resize) = case["after_resize_input_hex"].as_str() {
        term.feed(&hex_to_bytes(after_resize));
    }
    let cols: Vec<u16> = (0..term.history_len())
        .map(|i| term.history_line_cols(i).expect("cols") as u16)
        .collect();
    let actual = json!({ "history_cols": cols, "history_lines": term.history_len() });
    assert_eq!(
        actual,
        case["expect"],
        "resize case {} mismatch",
        case["name"].as_str().unwrap()
    );
}

#[test]
fn s8_history_corpus_passes() {
    let path = corpus_path();
    if !path.exists() {
        eprintln!("skipping corpus test: {:?} not found", path);
        return;
    }
    let data = std::fs::read_to_string(&path).expect("read corpus");
    let corpus: Value = serde_json::from_str(&data).expect("parse corpus");
    assert_eq!(corpus["slice"], "S8");
    let cases = corpus["cases"].as_array().expect("cases");
    for case in cases {
        let name = case["name"].as_str().expect("name").to_owned();
        match case["kind"].as_str().expect("kind") {
            "search" => run_search_case(case),
            "viewport" => run_viewport_case(case),
            "selection" => run_selection_case(case),
            "persist" => run_persist_case(case),
            "restore" => run_restore_case(case),
            "replay" => run_replay_case(case),
            "altscreen" => run_altscreen_case(case),
            "resize" => run_resize_case(case),
            other => panic!("case {name}: unknown kind {other}"),
        }
    }
}
