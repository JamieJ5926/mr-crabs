use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use mr_crabs_bench::{
    RELEASE_WORKLOADS, run_release_aggregate, run_release_workload, run_s3_suite,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("mr-crabs-bench: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args_os();
    let _program = args.next();
    let mut suite: Option<String> = None;
    let mut workload: Option<String> = None;
    let mut json_path: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        let s = arg
            .into_string()
            .map_err(|v| format!("invalid utf8 argument {v:?}\n{}", usage()))?;
        match s.as_str() {
            "--suite" => {
                let val = args
                    .next()
                    .ok_or_else(|| format!("missing value for --suite\n{}", usage()))?
                    .into_string()
                    .map_err(|v| format!("invalid utf8 {v:?}"))?;
                suite = Some(val);
            }
            "--workload" => {
                let val = args
                    .next()
                    .ok_or_else(|| format!("missing value for --workload\n{}", usage()))?
                    .into_string()
                    .map_err(|v| format!("invalid utf8 {v:?}"))?;
                workload = Some(val);
            }
            "--json" => {
                let val = args
                    .next()
                    .ok_or_else(|| format!("missing value for --json\n{}", usage()))?
                    .into_string()
                    .map_err(|v| format!("invalid utf8 {v:?}"))?;
                json_path = Some(PathBuf::from(val));
            }
            other => return Err(format!("unexpected argument {other:?}\n{}", usage())),
        }
    }

    let suite = suite.ok_or_else(|| format!("missing --suite\n{}", usage()))?;
    let json_path = json_path.ok_or_else(|| format!("missing --json <path>\n{}", usage()))?;

    let (summary, bytes) = match suite.as_str() {
        "release" => match workload.as_deref() {
            Some(name) => {
                if !RELEASE_WORKLOADS.contains(&name) {
                    return Err(format!(
                        "unknown release workload {name:?}; valid workloads: {}",
                        RELEASE_WORKLOADS.join(", ")
                    ));
                }
                let output = run_release_workload(name);
                let status = output.status.clone();
                let mut b = serde_json::to_vec_pretty(&output)
                    .map_err(|e| format!("failed to serialize output: {e}"))?;
                b.push(b'\n');
                (
                    format!(
                        "wrote release workload {name} to {} (status={status})",
                        json_path.display()
                    ),
                    b,
                )
            }
            None => {
                let output = run_release_aggregate();
                let mut b = serde_json::to_vec_pretty(&output)
                    .map_err(|e| format!("failed to serialize output: {e}"))?;
                b.push(b'\n');
                (
                    format!(
                        "wrote release aggregate ({} workloads) to {}",
                        output.workloads.len(),
                        json_path.display()
                    ),
                    b,
                )
            }
        },
        "s3" => {
            let output = run_s3_suite();
            let mut b = serde_json::to_vec_pretty(&output)
                .map_err(|e| format!("failed to serialize output: {e}"))?;
            b.push(b'\n');
            (
                format!(
                    "wrote s3 bench to {} (logical_lines={}, hot_resident_bytes={}, compressed_bytes={}, throughput_mbps={:.2})",
                    json_path.display(),
                    output.logical_lines,
                    output.hot_resident_bytes,
                    output.compressed_bytes,
                    output.throughput_mbps
                ),
                b,
            )
        }
        "s4-smoke" => {
            let ok = mr_crabs_bench::headless_cache_smoke();
            let body = format!(
                "{{\n  \"suite\": \"s4-smoke\",\n  \"ok\": {}\n}}\n",
                if ok { "true" } else { "false" }
            )
            .into_bytes();
            (
                format!("wrote s4-smoke to {} (ok={ok})", json_path.display()),
                body,
            )
        }
        "s5-smoke" => {
            let ok = mr_crabs_bench::s5_input_smoke();
            let body = format!(
                "{{\n  \"suite\": \"s5-smoke\",\n  \"ok\": {},\n  \"checked\": {}\n}}\n",
                if ok { "true" } else { "false" },
                if ok { 1 } else { 0 }
            )
            .into_bytes();
            (
                format!("wrote s5-smoke to {} (ok={ok})", json_path.display()),
                body,
            )
        }
        other => {
            return Err(format!(
                "unsupported suite {other:?}; supported suites: release, s3, s4-smoke, s5-smoke\n{}",
                usage()
            ));
        }
    };

    if let Some(parent) = json_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
        }
    }
    // Atomic write via tmp + rename.
    let tmp = json_path.with_extension("json.tmp");
    fs::write(&tmp, &bytes).map_err(|e| format!("failed to write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, &json_path).map_err(|e| {
        format!(
            "failed to rename {} -> {}: {e}",
            tmp.display(),
            json_path.display()
        )
    })?;

    println!("{summary}");
    Ok(())
}

fn usage() -> String {
    format!(
        concat!(
            "usage:\n",
            "  mr-crabs-bench --suite release --workload <name> --json <path>\n",
            "  mr-crabs-bench --suite release --json <path>\n",
            "  mr-crabs-bench --suite s3 --json <path>\n",
            "  mr-crabs-bench --suite s4-smoke --json <path>\n",
            "  mr-crabs-bench --suite s5-smoke --json <path>\n",
            "\nrelease workloads: {}\n"
        ),
        RELEASE_WORKLOADS.join(", ")
    )
}
