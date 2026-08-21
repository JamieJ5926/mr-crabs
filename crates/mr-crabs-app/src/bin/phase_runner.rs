use std::collections::HashMap;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mr_crabs_app::model::pane::{PaneModel, PaneSession};
use mr_crabs_app::model::split::PaneId;
use mr_crabs_element::RenderCache;
use mr_crabs_pty::{CommandBuilder, PtyConfig, PtySize};
use mr_crabs_terminal::GridSize;

const PHASE_READY: &[u8] = b"mr-crabs-phase-ready";

fn sparse_payload(total: usize) -> Vec<u8> {
    let line = b"hello sparse world 0123456789\n";
    let mut out = Vec::with_capacity(total);
    while out.len() < total {
        let n = (total - out.len()).min(line.len());
        out.extend_from_slice(&line[..n]);
    }
    out
}

fn fullscreen_payload(total: usize, cols: usize, rows: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(total);
    let fill = vec![b'X'; cols];
    while out.len() < total {
        for _ in 0..rows {
            if out.len() >= total {
                break;
            }
            let n = (total - out.len()).min(cols);
            out.extend_from_slice(&fill[..n]);
            if out.len() < total {
                out.push(b'\n');
            }
        }
    }
    out.truncate(total);
    out
}

fn payload_chunks_with(bytes: &[u8], chunk: usize) -> Vec<Vec<u8>> {
    let c = if chunk == 0 { 4096 } else { chunk };
    bytes.chunks(c).map(|v| v.to_vec()).collect()
}

fn top_level_sum(limit_to: &[&'static str], deltas: &[(&'static str, u64, u64)]) -> u64 {
    if limit_to.is_empty() {
        return deltas.iter().map(|(_, _, n)| *n).sum();
    }
    let set: std::collections::HashSet<&'static str> = limit_to.iter().copied().collect();
    deltas
        .iter()
        .filter(|(k, _, _)| set.contains(k))
        .map(|(_, _, n)| *n)
        .sum()
}

/// Budget check: remainder <= (budget_pct / 100) * wall, fail-closed on overflow.
/// budget_pct=30 is the headless 30% remainder budget.
fn remainder_within_budget(remainder: u64, wall: u64, budget_pct: u64) -> bool {
    if budget_pct == 0 || budget_pct > 100 {
        return false;
    }
    match remainder.checked_mul(100) {
        Some(v) => v <= wall.checked_mul(budget_pct).unwrap_or(u64::MAX),
        None => false,
    }
}

/// Pure helper for sidecar summary serialization (testable without I/O).
fn format_summary_json(r: &WorkloadResult) -> String {
    let error_json = match &r.error {
        Some(e) => format!("\"error\":{}", serde_json::to_string(e).unwrap()),
        None => "\"error\":null".to_string(),
    };
    let mut phases_json = String::from("[");
    for (i, (phase, count, nanos)) in r.deltas.iter().enumerate() {
        if i > 0 {
            phases_json.push(',');
        }
        phases_json.push_str(&format!(
            "{{\"phase\":\"{phase}\",\"count\":{count},\"nanos\":{nanos}}}"
        ));
    }
    phases_json.push(']');
    format!(
        "{{\"ts_ms\":{},\"workload\":\"{}\",\"path\":\"{}\",\"expected_bytes\":{},\"drained_bytes\":{},\"chunks\":{},\"frames\":{},\"wall_nanos\":{},\"top_sum_nanos\":{},\"remainder_nanos\":{},\"success\":{},{},\"phases\":{}}}",
        r.ts_ms,
        r.name,
        r.path,
        r.expected_bytes,
        r.drained_bytes,
        r.chunks,
        r.frames,
        r.wall,
        r.top_sum,
        r.remainder,
        r.success,
        error_json,
        phases_json
    )
}

#[cfg_attr(not(feature = "phase-timing"), allow(dead_code))]
#[derive(Default, Clone)]
struct PhaseMaps {
    pty: HashMap<&'static str, (u64, u64)>,
    term: HashMap<&'static str, (u64, u64)>,
    app: HashMap<&'static str, (u64, u64)>,
    element: HashMap<&'static str, (u64, u64)>,
}

fn capture_maps() -> PhaseMaps {
    #[allow(unused_mut)]
    let mut m = PhaseMaps::default();
    #[cfg(feature = "phase-timing")]
    {
        m.pty = mr_crabs_pty::phase::snapshot_map();
        m.term = mr_crabs_terminal::phase::snapshot_map();
        m.app = mr_crabs_app::phase::snapshot_map();
        m.element = mr_crabs_element::phase::snapshot_map();
    }
    m
}

fn deltas_since(
    prev: &PhaseMaps,
) -> (
    Vec<(&'static str, u64, u64)>,
    Vec<(&'static str, u64, u64)>,
    Vec<(&'static str, u64, u64)>,
    Vec<(&'static str, u64, u64)>,
) {
    #[cfg(feature = "phase-timing")]
    {
        return (
            mr_crabs_pty::phase::delta_since(&prev.pty),
            mr_crabs_terminal::phase::delta_since(&prev.term),
            mr_crabs_app::phase::delta_since(&prev.app),
            mr_crabs_element::phase::delta_since(&prev.element),
        );
    }
    #[cfg(not(feature = "phase-timing"))]
    {
        let _ = prev;
        (Vec::new(), Vec::new(), Vec::new(), Vec::new())
    }
}

struct WorkloadResult {
    name: String,
    ts_ms: u128,
    path: String,
    expected_bytes: usize,
    drained_bytes: usize,
    chunks: usize,
    frames: usize,
    wall: u64,
    top_sum: u64,
    remainder: u64,
    success: bool,
    error: Option<String>,
    deltas: Vec<(&'static str, u64, u64)>,
}

fn run_workload(
    name: &str,
    bytes: Vec<u8>,
    grid: GridSize,
    prev: &PhaseMaps,
    chunk_size: usize,
    timeout: Duration,
) -> (WorkloadResult, PhaseMaps) {
    let expected_bytes = bytes.len();
    let chunks = payload_chunks_with(&bytes, chunk_size);
    let mut wall_start = Instant::now();
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path_identity = "pty:PaneModel::pump+RenderCache::apply_frame".to_string();

    let finish = |wall_nanos: u64,
                  after: PhaseMaps,
                  all_deltas: Vec<(&'static str, u64, u64)>,
                  drained_bytes: usize,
                  drained_chunks: usize,
                  frames: usize,
                  writer_error: Option<String>| {
        // Only non-overlapping main-thread phases may explain measured wall.
        // Nested pane/terminal phases and reader-thread PTY waits are detail-only.
        let top_phases: &[&'static str] = &["pane_pump", "render_cache_apply"];
        let top_sum = top_level_sum(top_phases, &all_deltas);
        let top_over_wall = top_sum > wall_nanos;
        let remainder = wall_nanos.saturating_sub(top_sum);
        let bytes_ok = drained_bytes == expected_bytes;
        let budget_ok = !top_over_wall && remainder_within_budget(remainder, wall_nanos, 30);
        let success =
            bytes_ok && budget_ok && writer_error.is_none() && drained_chunks > 0 && frames > 0;
        let error = if !success {
            if let Some(e) = writer_error {
                Some(e)
            } else if top_over_wall {
                Some(format!("top_sum {top_sum} > wall {wall_nanos}"))
            } else if !budget_ok {
                Some(format!(
                    "scheduling gap exceeded 30%: remainder {remainder} > 30% of wall {wall_nanos} (remainder*100={} > wall*30)",
                    match remainder.checked_mul(100) {
                        Some(v) => v.to_string(),
                        None => "overflow".to_string(),
                    }
                ))
            } else if !bytes_ok {
                Some(format!(
                    "byte mismatch: expected {expected_bytes} drained {drained_bytes}"
                ))
            } else if drained_chunks == 0 {
                Some("no chunks drained".to_string())
            } else if frames == 0 {
                Some(
                    "no frames produced via PaneModel::pump + RenderCache::apply_frame".to_string(),
                )
            } else {
                Some("unknown workload failure".to_string())
            }
        } else {
            None
        };
        let result = WorkloadResult {
            name: name.to_string(),
            ts_ms,
            path: path_identity.clone(),
            expected_bytes,
            drained_bytes,
            chunks: drained_chunks,
            frames,
            wall: wall_nanos,
            top_sum,
            remainder,
            success,
            error,
            deltas: all_deltas,
        };
        (result, after)
    };

    let pty_size = match PtySize::new(grid.cols, grid.rows, 0, 0) {
        Ok(s) => s,
        Err(e) => {
            let after = capture_maps();
            let (pty_d, term_d, app_d, elem_d) = deltas_since(prev);
            let mut all: Vec<(&'static str, u64, u64)> = Vec::new();
            all.extend(pty_d.iter().cloned());
            all.extend(term_d.iter().cloned());
            all.extend(app_d.iter().cloned());
            all.extend(elem_d.iter().cloned());
            let wall_nanos = wall_start.elapsed().as_nanos() as u64;
            let (r, after) = finish(
                wall_nanos,
                after,
                all,
                0,
                0,
                0,
                Some(format!("PtySize::new failed: {e:?}")),
            );
            return (r, after);
        }
    };

    // Proven pty_echo sequence: raw mode, echo disabled, unique readiness marker,
    // then exec cat. Bytes must pass unchanged and not be doubled by line discipline.
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.args([
        "-c",
        "/bin/stty raw -echo; printf mr-crabs-phase-ready; exec /bin/cat",
    ]);
    let config = PtyConfig::new(cmd, pty_size)
        .with_writer_capacity(64)
        .with_reader_capacity(64);
    let spawn_res = mr_crabs_pty::PtySession::spawn(config);
    let (mut sess, rx, _exit) = match spawn_res {
        Ok(v) => v,
        Err(e) => {
            let after = capture_maps();
            let (pty_d, term_d, app_d, elem_d) = deltas_since(prev);
            let mut all: Vec<(&'static str, u64, u64)> = Vec::new();
            all.extend(pty_d.iter().cloned());
            all.extend(term_d.iter().cloned());
            all.extend(app_d.iter().cloned());
            all.extend(elem_d.iter().cloned());
            let wall_nanos = wall_start.elapsed().as_nanos() as u64;
            let (r, after) = finish(
                wall_nanos,
                after,
                all,
                0,
                0,
                0,
                Some(format!("PtySession::spawn failed: {e:?}")),
            );
            return (r, after);
        }
    };

    // Explicit readiness handshake: consume and validate only the readiness marker
    // before starting measured payload. Fail closed on any unexpected prefix/trailing
    // bytes rather than contaminating payload counts.
    let ready_deadline = Instant::now() + Duration::from_secs(5);
    let mut ready: Vec<u8> = Vec::with_capacity(PHASE_READY.len());
    while ready.len() < PHASE_READY.len() && Instant::now() < ready_deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                ready.extend_from_slice(&chunk);
                // If we already exceeded marker length, it's contamination.
                if ready.len() > PHASE_READY.len() {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if ready != PHASE_READY {
        // Drain pending to avoid leaking but surface as failure; do not count readiness bytes.
        let after = capture_maps();
        let (pty_d, term_d, app_d, elem_d) = deltas_since(prev);
        let mut all: Vec<(&'static str, u64, u64)> = Vec::new();
        all.extend(pty_d.iter().cloned());
        all.extend(term_d.iter().cloned());
        all.extend(app_d.iter().cloned());
        all.extend(elem_d.iter().cloned());
        let wall_nanos = wall_start.elapsed().as_nanos() as u64;
        let err = if ready.is_empty() {
            format!(
                "readiness marker not observed within 5s: expected {:?}",
                String::from_utf8_lossy(PHASE_READY)
            )
        } else {
            format!(
                "readiness marker mismatch: expected {:?} got {:?} (len {} vs {})",
                String::from_utf8_lossy(PHASE_READY),
                String::from_utf8_lossy(&ready),
                PHASE_READY.len(),
                ready.len()
            )
        };
        // Ensure session is reaped post-failure without contaminating counts.
        let _ = sess.shutdown_and_reap(Duration::from_secs(1));
        let (r, after) = finish(wall_nanos, after, all, 0, 0, 0, Some(err));
        return (r, after);
    }

    // Construct the detached pane and cache before measuring payload work.
    // Spawn/readiness and teardown are setup, not payload attribution.
    let mut pane = match PaneModel::detached(PaneId::new(1), grid) {
        Ok(p) => p,
        Err(e) => {
            let _ = sess.shutdown_and_reap(Duration::from_secs(1));
            let after = capture_maps();
            let (pty_d, term_d, app_d, elem_d) = deltas_since(prev);
            let mut all: Vec<(&'static str, u64, u64)> = Vec::new();
            all.extend(pty_d.iter().cloned());
            all.extend(term_d.iter().cloned());
            all.extend(app_d.iter().cloned());
            all.extend(elem_d.iter().cloned());
            let wall_nanos = wall_start.elapsed().as_nanos() as u64;
            let (r, after) = finish(
                wall_nanos,
                after,
                all,
                0,
                0,
                0,
                Some(format!("PaneModel::detached failed: {e:?}")),
            );
            return (r, after);
        }
    };
    pane.session = PaneSession::from_receivers(grid, Some(rx), None);

    let mut cache = RenderCache::new();
    let mut drained_bytes: usize = 0;
    let mut drained_chunks: usize = 0;
    let mut frames: usize = 0;

    // Interleaved bounded writes and pane.pump/cache application so neither
    // bounded queue deadlocks. The owning PtySession remains live until
    // measured drain completes.
    let deadline = Instant::now() + timeout;
    let mut saw_frame_error: Option<String> = None;
    let mut writer_error: Option<String> = None;
    let mut chunk_idx: usize = 0;

    // Payload wall: from first write through final drain, before shutdown_and_reap.
    // Phase deltas are sampled against the snapshot taken here.
    let payload_prev = capture_maps();
    wall_start = Instant::now();

    while Instant::now() < deadline {
        // Bounded write: one chunk per iteration to keep pipeline flowing.
        if chunk_idx < chunks.len() && writer_error.is_none() {
            // Use blocking write which applies backpressure via bounded writer queue.
            // The concurrent pump below drains the reader queue, so progress is guaranteed.
            if let Err(e) = sess.write(chunks[chunk_idx].clone()) {
                writer_error = Some(format!("pty write failed: {e:?}"));
                break;
            }
            chunk_idx += 1;
        }

        let stats = pane.pump(8);
        drained_chunks += stats.chunks;
        drained_bytes += stats.bytes;
        if stats.frames > 0 {
            if let Some(frame) = pane.latest_frame.clone() {
                let _ = cache.apply_frame(&frame);
                frames += stats.frames as usize;
            } else {
                saw_frame_error =
                    Some("pane.pump reported frames but latest_frame is None".to_string());
                break;
            }
        }
        if drained_bytes >= expected_bytes
            && !pane.session.has_pending()
            && chunk_idx >= chunks.len()
        {
            break;
        }
        if writer_error.is_some() {
            break;
        }
        // Yield without adding a fixed delay to the measured scheduling remainder.
        if stats.chunks == 0 && stats.bytes == 0 && stats.frames == 0 {
            if chunk_idx >= chunks.len()
                && !pane.session.has_pending()
                && drained_bytes >= expected_bytes
            {
                break;
            }
            std::thread::yield_now();
        }
    }

    if let Some(err) = saw_frame_error {
        let wall_nanos = wall_start.elapsed().as_nanos() as u64;
        let after = capture_maps();
        let (pty_d, term_d, app_d, elem_d) = deltas_since(&payload_prev);
        let mut all: Vec<(&'static str, u64, u64)> = Vec::new();
        all.extend(pty_d.iter().cloned());
        all.extend(term_d.iter().cloned());
        all.extend(app_d.iter().cloned());
        all.extend(elem_d.iter().cloned());
        let _ = sess.shutdown_and_reap(Duration::from_secs(2));
        let (r, _) = finish(
            wall_nanos,
            after,
            all,
            drained_bytes,
            drained_chunks,
            frames,
            Some(err),
        );
        return (r, capture_maps());
    }

    if let Some(e) = writer_error.take() {
        let wall_nanos = wall_start.elapsed().as_nanos() as u64;
        let after = capture_maps();
        let (pty_d, term_d, app_d, elem_d) = deltas_since(&payload_prev);
        let mut all: Vec<(&'static str, u64, u64)> = Vec::new();
        all.extend(pty_d.iter().cloned());
        all.extend(term_d.iter().cloned());
        all.extend(app_d.iter().cloned());
        all.extend(elem_d.iter().cloned());
        let _ = sess.shutdown_and_reap(Duration::from_secs(2));
        let (r, _) = finish(
            wall_nanos,
            after,
            all,
            drained_bytes,
            drained_chunks,
            frames,
            Some(e),
        );
        return (r, capture_maps());
    }

    // Final bounded drain after all writes accepted to flush any remaining queued chunks.
    let final_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < final_deadline
        && (pane.session.has_pending() || drained_bytes < expected_bytes)
    {
        let stats = pane.pump(64);
        drained_chunks += stats.chunks;
        drained_bytes += stats.bytes;
        if stats.frames > 0 {
            if let Some(frame) = pane.latest_frame.clone() {
                let _ = cache.apply_frame(&frame);
                frames += stats.frames as usize;
            }
        }
        if !stats.pending && !pane.session.has_pending() && drained_bytes >= expected_bytes {
            break;
        }
        std::thread::yield_now();
    }

    // Payload wall ends here; deltas are sampled before shutdown_and_reap.
    let wall_nanos = wall_start.elapsed().as_nanos() as u64;
    let measured_after = capture_maps();
    let (pty_d, term_d, app_d, elem_d) = deltas_since(&payload_prev);
    let mut all: Vec<(&'static str, u64, u64)> = Vec::new();
    all.extend(pty_d.iter().cloned());
    all.extend(term_d.iter().cloned());
    all.extend(app_d.iter().cloned());
    all.extend(elem_d.iter().cloned());

    // Shutdown strictly post-measurement: only after exact expected bytes are drained and
    // no pending output remains, or a failure bound trips. Surface reap errors.
    let shutdown_err = match sess.shutdown_and_reap(Duration::from_secs(2)) {
        Ok(_) => None,
        Err(e) => Some(format!("shutdown_and_reap failed: {e:?}")),
    };

    let (result, _) = finish(
        wall_nanos,
        measured_after,
        all,
        drained_bytes,
        drained_chunks,
        frames,
        shutdown_err,
    );
    (result, capture_maps())
}

fn write_sidecar(results: &[WorkloadResult]) -> Result<String, String> {
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let out_path = std::env::var("MR_CRABS_PHASE_OUT")
        .unwrap_or_else(|_| format!("/tmp/mr-crabs-phase-runner-{ts_ms}.jsonl"));
    let mut out = String::new();
    for r in results {
        out.push_str(&format_summary_json(r));
        out.push('\n');
        for (phase, count, nanos) in &r.deltas {
            out.push_str(&format!(
                "{{\"ts_ms\":{},\"workload\":\"{}\",\"phase\":\"{phase}\",\"count\":{count},\"nanos\":{nanos}}}\n",
                r.ts_ms, r.name
            ));
        }
    }
    if let Err(e) = std::fs::write(&out_path, &out) {
        return Err(format!("phase sidecar write failed for {out_path}: {e}"));
    }
    eprintln!("phase sidecar: {out_path}");
    Ok(out_path)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut variant_filter: Option<String> = None;
    let mut bytes_arg: Option<usize> = None;
    let mut chunk_arg: Option<usize> = None;
    let mut timeout_ms: Option<u64> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--variant" => {
                if i + 1 >= args.len() {
                    eprintln!("[phase-runner] --variant requires a value");
                    eprintln!(
                        "usage: phase-runner [--variant sparse|sparse_scrolling|fullscreen|fullscreen_scrolling] [--bytes N] [--chunk N] [--timeout-ms N]"
                    );
                    std::process::exit(2);
                }
                variant_filter = Some(args[i + 1].clone());
                i += 2;
            }
            "--bytes" => {
                if i + 1 >= args.len() {
                    eprintln!("[phase-runner] --bytes requires a value");
                    eprintln!(
                        "usage: phase-runner [--variant sparse|sparse_scrolling|fullscreen|fullscreen_scrolling] [--bytes N] [--chunk N] [--timeout-ms N]"
                    );
                    std::process::exit(2);
                }
                match args[i + 1].parse::<usize>() {
                    Ok(v) if v > 0 => bytes_arg = Some(v),
                    Ok(_) => {
                        eprintln!("[phase-runner] --bytes must be > 0, got {}", args[i + 1]);
                        std::process::exit(2);
                    }
                    Err(_) => {
                        eprintln!("[phase-runner] invalid --bytes value: {}", args[i + 1]);
                        std::process::exit(2);
                    }
                }
                i += 2;
            }
            "--chunk" => {
                if i + 1 >= args.len() {
                    eprintln!("[phase-runner] --chunk requires a value");
                    eprintln!(
                        "usage: phase-runner [--variant sparse|sparse_scrolling|fullscreen|fullscreen_scrolling] [--bytes N] [--chunk N] [--timeout-ms N]"
                    );
                    std::process::exit(2);
                }
                match args[i + 1].parse::<usize>() {
                    Ok(v) if v > 0 => chunk_arg = Some(v),
                    Ok(_) => {
                        eprintln!("[phase-runner] --chunk must be > 0, got {}", args[i + 1]);
                        std::process::exit(2);
                    }
                    Err(_) => {
                        eprintln!("[phase-runner] invalid --chunk value: {}", args[i + 1]);
                        std::process::exit(2);
                    }
                }
                i += 2;
            }
            "--timeout-ms" => {
                if i + 1 >= args.len() {
                    eprintln!("[phase-runner] --timeout-ms requires a value");
                    eprintln!(
                        "usage: phase-runner [--variant sparse|sparse_scrolling|fullscreen|fullscreen_scrolling] [--bytes N] [--chunk N] [--timeout-ms N]"
                    );
                    std::process::exit(2);
                }
                match args[i + 1].parse::<u64>() {
                    Ok(v) if v > 0 => timeout_ms = Some(v),
                    Ok(_) => {
                        eprintln!(
                            "[phase-runner] --timeout-ms must be > 0, got {}",
                            args[i + 1]
                        );
                        std::process::exit(2);
                    }
                    Err(_) => {
                        eprintln!("[phase-runner] invalid --timeout-ms value: {}", args[i + 1]);
                        std::process::exit(2);
                    }
                }
                i += 2;
            }
            other => {
                eprintln!("[phase-runner] unknown flag: {other}");
                eprintln!(
                    "usage: phase-runner [--variant sparse|sparse_scrolling|fullscreen|fullscreen_scrolling] [--bytes N] [--chunk N] [--timeout-ms N]"
                );
                std::process::exit(2);
            }
        }
    }
    if let Some(v) = &variant_filter {
        match v.as_str() {
            "sparse" | "sparse_scrolling" | "fullscreen" | "fullscreen_scrolling" => {}
            _ => {
                eprintln!("[phase-runner] unknown --variant {v}");
                eprintln!(
                    "valid variants: sparse, sparse_scrolling, fullscreen, fullscreen_scrolling"
                );
                std::process::exit(2);
            }
        }
    }

    let grid = GridSize::new(80, 24);
    let total_bytes = bytes_arg.unwrap_or(256 * 1024);
    let chunk_size = chunk_arg.unwrap_or(4096);
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(8000));

    let sparse = sparse_payload(total_bytes);
    let fullscreen = fullscreen_payload(total_bytes, 80 as usize, 24 as usize);
    let mut workloads: Vec<(String, Vec<u8>)> = Vec::new();
    match variant_filter.as_deref() {
        Some("sparse") | Some("sparse_scrolling") => {
            workloads.push(("sparse_scrolling".to_string(), sparse))
        }
        Some("fullscreen") | Some("fullscreen_scrolling") => {
            workloads.push(("fullscreen_scrolling".to_string(), fullscreen))
        }
        None => {
            workloads.push(("sparse_scrolling".to_string(), sparse));
            workloads.push(("fullscreen_scrolling".to_string(), fullscreen));
        }
        Some(_) => unreachable!("variant validated above"),
    }
    let mut results: Vec<WorkloadResult> = Vec::new();
    let mut prev = capture_maps();
    let mut any_failed = false;
    for (name, bytes) in workloads {
        let (res, after) = run_workload(&name, bytes, grid, &prev, chunk_size, timeout);
        if !res.success {
            any_failed = true;
        }
        println!(
            "{{\"ts_ms\":{},\"workload\":\"{}\",\"path\":\"{}\",\"expected_bytes\":{},\"drained_bytes\":{},\"chunks\":{},\"frames\":{},\"wall_nanos\":{},\"top_sum_nanos\":{},\"remainder_nanos\":{},\"success\":{}}}",
            res.ts_ms,
            res.name,
            res.path,
            res.expected_bytes,
            res.drained_bytes,
            res.chunks,
            res.frames,
            res.wall,
            res.top_sum,
            res.remainder,
            res.success
        );
        eprintln!(
            "[phase-runner] {}: expected={} drained={} chunks={} frames={} wall={}ms top_sum={}ms remainder={}ms success={} {}",
            res.name,
            res.expected_bytes,
            res.drained_bytes,
            res.chunks,
            res.frames,
            res.wall / 1_000_000,
            res.top_sum / 1_000_000,
            res.remainder / 1_000_000,
            res.success,
            res.error.clone().unwrap_or_default()
        );
        for (ph, c, n) in &res.deltas {
            eprintln!(
                "  phase {ph}: count={c} nanos={n} ({:.2}ms)",
                *n as f64 / 1e6
            );
        }
        results.push(res);
        prev = after;
    }
    if let Err(e) = write_sidecar(&results) {
        eprintln!("[phase-runner] {e}");
        std::process::exit(1);
    }
    if any_failed {
        eprintln!("[phase-runner] one or more workloads failed (fail-closed)");
        std::process::exit(1);
    }
}
