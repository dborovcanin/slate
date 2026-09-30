//! Performance gates against the baselines in `perf/baselines/`: startup
//! marks (`note-startup`), table hot paths (`table-perf`) and large-note
//! latency (`large_note_perf`). Which checks run is set in `perf/config.json`;
//! all of them are skipped while `[perf] enabled = false` in `config.toml`.
//!
//! Run from the repository root:
//!
//! ```sh
//! cargo run --release -p slate --bin perf-check
//! cargo run --release -p slate --bin perf-check -- --record-startup [--runs N]
//! ```
//!
//! `--record-startup` rewrites `perf/baselines/startup.json` from fresh
//! medians instead of checking.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::process::{Command, ExitCode};
use std::time::Instant;

const CONFIG_PATH: &str = "perf/config.json";
const STARTUP_BASELINE_PATH: &str = "perf/baselines/startup.json";
const TABLE_BASELINE_PATH: &str = "perf/baselines/table.json";
const STARTUP_MODES: &[&str] = &["tui"];
const DEFAULT_RECORD_RUNS: usize = 5;

struct CheckResult {
    name: &'static str,
    ok: bool,
    elapsed_ms: u128,
    output: String,
}

fn read_json(path: &str) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("failed to read {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("failed to parse {path}: {e}"))
}

/// Runs `cargo` with `args`; returns stdout, or the error output on failure.
fn cargo(args: &[&str]) -> Result<String, String> {
    let output = Command::new("cargo")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

/// Median of every startup mark over `runs` fresh `note-startup` processes.
fn startup_medians(mode: &str, runs: usize) -> Result<BTreeMap<String, f64>, String> {
    let mut samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for _ in 0..runs {
        let output = cargo(&[
            "run",
            "--quiet",
            "-p",
            "slate",
            "--bin",
            "note-startup",
            "--",
            mode,
        ])
        .map_err(|e| format!("startup probe '{mode}' failed: {e}"))?;
        let probe: Value = serde_json::from_str(&output)
            .map_err(|e| format!("invalid JSON from startup probe '{mode}': {e}"))?;
        for mark in probe["marks"].as_array().into_iter().flatten() {
            if let (Some(name), Some(ms)) = (mark["name"].as_str(), mark["ms"].as_f64()) {
                samples.entry(name.to_string()).or_default().push(ms);
            }
        }
    }
    Ok(samples
        .into_iter()
        .map(|(name, mut values)| (name, median(&mut values)))
        .collect())
}

fn check_startup(runs: usize, threshold_pct: f64) -> Result<String, String> {
    let baseline = read_json(STARTUP_BASELINE_PATH)?;
    let factor = 1.0 + threshold_pct / 100.0;
    let mut report = Vec::new();
    let mut regressions = Vec::new();
    for (mode, expected) in baseline["modes"].as_object().into_iter().flatten() {
        let current = startup_medians(mode, runs)?;
        report.push(format!("{mode} median marks:"));
        for (name, ms) in &current {
            report.push(format!("  {name}: {ms:.3}ms"));
        }
        for (metric, expected_ms) in expected.as_object().into_iter().flatten() {
            let expected_ms = expected_ms.as_f64().unwrap_or(0.0);
            if metric == "probe_start" || expected_ms <= 0.0 {
                continue;
            }
            let Some(current_ms) = current.get(metric) else {
                continue;
            };
            let limit = expected_ms * factor;
            if *current_ms > limit {
                regressions.push(format!(
                    "[{mode}] {metric}: current {current_ms:.3}ms > limit {limit:.3}ms (baseline {expected_ms:.3}ms)"
                ));
            }
        }
    }
    if regressions.is_empty() {
        report.insert(
            0,
            format!("startup passed ({threshold_pct}% threshold, {runs} runs)"),
        );
        Ok(report.join("\n"))
    } else {
        report.push("startup regression:".to_string());
        report.extend(regressions);
        Err(report.join("\n"))
    }
}

fn check_table(profile: &str) -> Result<String, String> {
    let baseline = read_json(TABLE_BASELINE_PATH)?;
    let output = cargo(&[
        "run",
        "--quiet",
        "--release",
        "-p",
        "slate",
        "--bin",
        "table-perf",
        "--",
        "--profile",
        profile,
        "--json",
    ])
    .map_err(|e| format!("table-perf probe failed: {e}"))?;
    let report: Value =
        serde_json::from_str(&output).map_err(|e| format!("invalid table-perf JSON: {e}"))?;
    let mut worst: BTreeMap<String, f64> = BTreeMap::new();
    for stat in report["stats"].as_array().into_iter().flatten() {
        if let (Some(metric), Some(p95)) = (stat["metric"].as_str(), stat["p95_ms"].as_f64()) {
            let entry = worst.entry(metric.to_string()).or_insert(p95);
            *entry = entry.max(p95);
        }
    }
    let mut failures = Vec::new();
    for (metric, limit) in baseline["limits_p95_ms"].as_object().into_iter().flatten() {
        let limit = limit.as_f64().unwrap_or(f64::INFINITY);
        match worst.get(metric) {
            None => failures.push(format!("missing metric '{metric}' in report")),
            Some(observed) if *observed > limit => failures.push(format!(
                "{metric}: p95 {observed:.3}ms exceeds limit {limit:.3}ms"
            )),
            Some(_) => {}
        }
    }
    let lines: Vec<String> = worst
        .iter()
        .map(|(metric, p95)| format!("{metric}: worst p95 {p95:.3}ms"))
        .collect();
    if failures.is_empty() {
        Ok(format!(
            "table passed for profile '{profile}'\n{}",
            lines.join("\n")
        ))
    } else {
        Err(format!(
            "table regression for profile '{profile}':\n{}",
            failures.join("\n")
        ))
    }
}

fn check_large_note() -> Result<String, String> {
    // Limits live in perf/baselines/large_note.json; the test enforces them.
    cargo(&[
        "test",
        "--quiet",
        "--release",
        "-p",
        "slate",
        "--lib",
        "large_note_perf",
        "--",
        "--ignored",
        "--nocapture",
    ])
}

fn run_check(name: &'static str, check: impl FnOnce() -> Result<String, String>) -> CheckResult {
    let started = Instant::now();
    let (ok, output) = match check() {
        Ok(output) => (true, output),
        Err(output) => (false, output),
    };
    CheckResult {
        name,
        ok,
        elapsed_ms: started.elapsed().as_millis(),
        output,
    }
}

fn record_startup(runs: usize) -> Result<(), String> {
    let mut modes = serde_json::Map::new();
    for mode in STARTUP_MODES {
        let medians = startup_medians(mode, runs)?;
        let rounded: serde_json::Map<String, Value> = medians
            .into_iter()
            .map(|(name, ms)| (name, json!((ms * 1000.0).round() / 1000.0)))
            .collect();
        modes.insert((*mode).to_string(), Value::Object(rounded));
    }
    let recorded_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| e.to_string())?;
    let baseline = json!({
        "version": 1,
        "recorded_at": recorded_at,
        "threshold_pct": 15,
        "runs": runs,
        "modes": modes,
    });
    let text = serde_json::to_string_pretty(&baseline).map_err(|e| e.to_string())?;
    fs::write(STARTUP_BASELINE_PATH, format!("{text}\n"))
        .map_err(|e| format!("failed to write {STARTUP_BASELINE_PATH}: {e}"))?;
    println!("Wrote startup baseline: {STARTUP_BASELINE_PATH}\n{text}");
    Ok(())
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|idx| args.get(idx + 1).cloned())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--record-startup") {
        let runs = flag_value(&args, "--runs")
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_RECORD_RUNS);
        return match record_startup(runs) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }

    println!("Slate perf check");
    if !app_core::config::load_perf_config().enabled {
        println!("Perf checks are disabled by config.toml [perf] enabled = false.");
        return ExitCode::SUCCESS;
    }
    let config = match read_json(CONFIG_PATH) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let checks = &config["checks"];
    let enabled = |name: &str| checks[name]["enabled"].as_bool().unwrap_or(true);

    let mut results = Vec::new();
    if enabled("startup") {
        let runs = checks["startup"]["runs"].as_u64().unwrap_or(3) as usize;
        let threshold = checks["startup"]["threshold_pct"].as_f64().unwrap_or(15.0);
        results.push(run_check("startup", || check_startup(runs, threshold)));
    }
    if enabled("table") {
        let profile = checks["table"]["profile"]
            .as_str()
            .unwrap_or("ci")
            .to_string();
        results.push(run_check("table", || check_table(&profile)));
    }
    if enabled("large_note") {
        results.push(run_check("large_note", check_large_note));
    }

    println!("\n== Checks ==");
    for result in &results {
        let status = if result.ok { "PASS" } else { "FAIL" };
        println!("{status} {} ({}ms)", result.name, result.elapsed_ms);
        for line in result.output.lines() {
            println!("  {line}");
        }
    }
    let failed = results.iter().filter(|result| !result.ok).count();
    println!(
        "\n== Summary ==\ntotal: {}\npassed: {}\nfailed: {failed}",
        results.len(),
        results.len() - failed
    );
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
