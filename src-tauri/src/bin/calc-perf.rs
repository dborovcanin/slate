use editor_core::calc_plan::{
    build_table_formula_dependency_index, build_variable_dependency_graph,
    decide_eval_window_with_cached_dependency_indexes_and_flags, decide_eval_window_with_flags,
    sync_table_formula_dependency_index, sync_variable_dependency_graph, CalcFeatureMask,
};
use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchProfile {
    Default,
    Ci,
}

#[derive(Debug, Clone, Serialize)]
struct BenchStat {
    metric: String,
    scenario: String,
    n: usize,
    avg_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    avg_window_span: f64,
    p95_window_span: usize,
}

#[derive(Debug, Clone, Serialize)]
struct PerfReport {
    profile: String,
    stats: Vec<BenchStat>,
}

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = ((pct / 100.0) * (sorted.len() as f64 - 1.0)).clamp(0.0, sorted.len() as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        let w = rank - lo as f64;
        sorted[lo] * (1.0 - w) + sorted[hi] * w
    }
}

fn percentile_usize(sorted: &[usize], pct: f64) -> usize {
    if sorted.is_empty() {
        return 0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = ((pct / 100.0) * (sorted.len() as f64 - 1.0)).clamp(0.0, sorted.len() as f64 - 1.0);
    let idx = rank.round() as usize;
    sorted[idx.min(sorted.len().saturating_sub(1))]
}

fn summarize(
    metric: &str,
    scenario: &str,
    mut samples_ms: Vec<f64>,
    mut spans: Vec<usize>,
) -> BenchStat {
    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    spans.sort_unstable();
    let n = samples_ms.len();
    let avg_ms = if n == 0 {
        0.0
    } else {
        samples_ms.iter().sum::<f64>() / n as f64
    };
    let avg_window_span = if spans.is_empty() {
        0.0
    } else {
        spans.iter().sum::<usize>() as f64 / spans.len() as f64
    };
    BenchStat {
        metric: metric.to_string(),
        scenario: scenario.to_string(),
        n,
        avg_ms,
        p50_ms: percentile(&samples_ms, 50.0),
        p95_ms: percentile(&samples_ms, 95.0),
        p99_ms: percentile(&samples_ms, 99.0),
        max_ms: samples_ms.last().copied().unwrap_or(0.0),
        avg_window_span,
        p95_window_span: percentile_usize(&spans, 95.0),
    }
}

fn print_stat(stat: &BenchStat) {
    println!(
        "{} [{}]: n={} avg={:.3}ms p50={:.3}ms p95={:.3}ms p99={:.3}ms max={:.3}ms window(avg={:.1},p95={})",
        stat.metric,
        stat.scenario,
        stat.n,
        stat.avg_ms,
        stat.p50_ms,
        stat.p95_ms,
        stat.p99_ms,
        stat.max_ms,
        stat.avg_window_span,
        stat.p95_window_span
    );
}

fn make_variable_chain_doc(chain_len: usize, sink_count: usize) -> Vec<String> {
    let mut lines = Vec::with_capacity(chain_len + sink_count * 2 + 8);
    lines.push("seed := 1".to_string());
    for idx in 0..chain_len {
        if idx == 0 {
            lines.push("v0 := seed + 1".to_string());
        } else {
            lines.push(format!("v{} := v{} + 1", idx, idx - 1));
        }
    }
    for idx in 0..sink_count {
        let a = chain_len.saturating_sub(1 + (idx % 32));
        let b = chain_len.saturating_sub(1 + ((idx + 7) % 32));
        lines.push(format!("sink{} := v{} + v{}", idx, a, b));
        lines.push(format!("sink{}", idx));
    }
    lines
}

fn make_large_coord_table(rows: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(rows + 2);
    out.push("| item | value | calc |".to_string());
    out.push("| --- | --- | --- |".to_string());
    for row in 1..=rows {
        if row >= 3 && row % 7 == 0 {
            out.push(format!(
                "| f{} | {} | :=({},2) + ({},2) |",
                row,
                row,
                row - 1,
                row - 2
            ));
        } else {
            out.push(format!("| r{} | {} | |", row, row));
        }
    }
    out
}

fn bench_variable_chain(profile: BenchProfile) -> Vec<BenchStat> {
    let (chain_len, sink_count, iterations) = match profile {
        BenchProfile::Default => (18_000usize, 2_000usize, 140usize),
        BenchProfile::Ci => (5_500usize, 650usize, 48usize),
    };
    let mask = CalcFeatureMask::default();
    let mut lines = make_variable_chain_doc(chain_len, sink_count);
    let mut variable_graph = build_variable_dependency_graph(&lines, mask);
    let mut table_index = build_table_formula_dependency_index(&lines, mask);

    let mut sync_ms = Vec::with_capacity(iterations);
    let mut cached_ms = Vec::with_capacity(iterations);
    let mut uncached_ms = Vec::with_capacity(iterations);
    let mut spans = Vec::with_capacity(iterations);

    for iter in 0..iterations {
        let edit_idx = 2 + (iter % chain_len.saturating_sub(2));
        let prev_names = vec![format!("v{}", edit_idx - 1)];
        lines[edit_idx] = format!(
            "v{} := v{} + {}",
            edit_idx - 1,
            edit_idx.saturating_sub(2),
            (iter % 9) + 1
        );

        let sync_start = Instant::now();
        sync_variable_dependency_graph(&mut variable_graph, &lines, edit_idx, edit_idx + 1, mask);
        sync_table_formula_dependency_index(&mut table_index, &lines, edit_idx, edit_idx + 1, mask);
        sync_ms.push(sync_start.elapsed().as_secs_f64() * 1000.0);

        let cached_start = Instant::now();
        let cached = decide_eval_window_with_cached_dependency_indexes_and_flags(
            variable_graph.as_ref(),
            table_index.as_ref(),
            &lines,
            edit_idx,
            edit_idx + 1,
            &prev_names,
            true,
            false,
            true,
            true,
            true,
        );
        cached_ms.push(cached_start.elapsed().as_secs_f64() * 1000.0);

        let uncached_start = Instant::now();
        let uncached = decide_eval_window_with_flags(
            &lines,
            edit_idx,
            edit_idx + 1,
            &prev_names,
            true,
            false,
            true,
            true,
            true,
        );
        uncached_ms.push(uncached_start.elapsed().as_secs_f64() * 1000.0);

        assert_eq!(cached.eval_from, uncached.eval_from);
        assert_eq!(cached.eval_to, uncached.eval_to);
        assert_eq!(cached.can_use_partial, uncached.can_use_partial);
        spans.push(cached.eval_to.saturating_sub(cached.eval_from));
    }

    vec![
        summarize(
            "sync_variable_and_table_indexes",
            "variable_chain",
            sync_ms,
            spans.clone(),
        ),
        summarize(
            "decide_eval_window_cached",
            "variable_chain",
            cached_ms,
            spans.clone(),
        ),
        summarize(
            "decide_eval_window_uncached",
            "variable_chain",
            uncached_ms,
            spans,
        ),
    ]
}

fn bench_table_coord(profile: BenchProfile) -> Vec<BenchStat> {
    let (rows, iterations) = match profile {
        BenchProfile::Default => (9_500usize, 120usize),
        BenchProfile::Ci => (2_800usize, 40usize),
    };
    let mask = CalcFeatureMask {
        math_enabled: true,
        table_enabled: true,
        variables_enabled: false,
    };
    let mut lines = make_large_coord_table(rows);
    let mut variable_graph = build_variable_dependency_graph(&lines, mask);
    let mut table_index = build_table_formula_dependency_index(&lines, mask);

    let mut sync_ms = Vec::with_capacity(iterations);
    let mut cached_ms = Vec::with_capacity(iterations);
    let mut uncached_ms = Vec::with_capacity(iterations);
    let mut spans = Vec::with_capacity(iterations);

    for iter in 0..iterations {
        let data_row = 2 + (iter * 11 % rows.saturating_sub(4)).max(1);
        lines[data_row] = format!("| r{} | {} | |", data_row - 1, 1000 + iter);
        let changed_from = data_row;
        let changed_to = changed_from + 1;

        let sync_start = Instant::now();
        sync_variable_dependency_graph(&mut variable_graph, &lines, changed_from, changed_to, mask);
        sync_table_formula_dependency_index(
            &mut table_index,
            &lines,
            changed_from,
            changed_to,
            mask,
        );
        sync_ms.push(sync_start.elapsed().as_secs_f64() * 1000.0);

        let cached_start = Instant::now();
        let cached = decide_eval_window_with_cached_dependency_indexes_and_flags(
            variable_graph.as_ref(),
            table_index.as_ref(),
            &lines,
            changed_from,
            changed_to,
            &[],
            false,
            false,
            true,
            false,
            true,
        );
        cached_ms.push(cached_start.elapsed().as_secs_f64() * 1000.0);

        let uncached_start = Instant::now();
        let uncached = decide_eval_window_with_flags(
            &lines,
            changed_from,
            changed_to,
            &[],
            false,
            false,
            true,
            false,
            true,
        );
        uncached_ms.push(uncached_start.elapsed().as_secs_f64() * 1000.0);

        assert_eq!(cached.eval_from, uncached.eval_from);
        assert_eq!(cached.eval_to, uncached.eval_to);
        assert_eq!(cached.can_use_partial, uncached.can_use_partial);
        spans.push(cached.eval_to.saturating_sub(cached.eval_from));
    }

    vec![
        summarize(
            "sync_variable_and_table_indexes",
            "table_coord",
            sync_ms,
            spans.clone(),
        ),
        summarize(
            "decide_eval_window_cached",
            "table_coord",
            cached_ms,
            spans.clone(),
        ),
        summarize(
            "decide_eval_window_uncached",
            "table_coord",
            uncached_ms,
            spans,
        ),
    ]
}

fn make_mixed_doc(var_count: usize, table_rows: usize, prose_count: usize) -> Vec<String> {
    let mut lines = Vec::new();
    lines.extend((0..prose_count).map(|idx| format!("prose intro {}", idx)));
    lines.push("base := 1".to_string());
    for idx in 0..var_count {
        if idx == 0 {
            lines.push("mv0 := base + 1".to_string());
        } else {
            lines.push(format!("mv{} := mv{} + 1", idx, idx - 1));
        }
    }
    lines.extend(make_large_coord_table(table_rows));
    lines.extend((0..prose_count).map(|idx| format!("prose outro {}", idx)));
    lines
}

fn bench_mixed_structural_and_content_edits(profile: BenchProfile) -> Vec<BenchStat> {
    let (var_count, table_rows, prose_count, iterations) = match profile {
        BenchProfile::Default => (4_500usize, 2_500usize, 1_600usize, 120usize),
        BenchProfile::Ci => (1_400usize, 900usize, 500usize, 36usize),
    };
    let mask = CalcFeatureMask::default();
    let mut lines = make_mixed_doc(var_count, table_rows, prose_count);
    let mut variable_graph = build_variable_dependency_graph(&lines, mask);
    let mut table_index = build_table_formula_dependency_index(&lines, mask);

    let mut sync_ms = Vec::with_capacity(iterations);
    let mut cached_ms = Vec::with_capacity(iterations);
    let mut uncached_ms = Vec::with_capacity(iterations);
    let mut spans = Vec::with_capacity(iterations);

    for iter in 0..iterations {
        let op = iter % 3;
        let (changed_from, changed_to, prev_names, prev_had_assignment, prev_had_builtin) = if op
            == 0
        {
            let line_idx = prose_count + 2 + (iter % var_count.saturating_sub(2));
            let var_idx = line_idx - (prose_count + 1);
            lines[line_idx] = format!(
                "mv{} := mv{} + {}",
                var_idx,
                var_idx.saturating_sub(1),
                (iter % 11) + 1
            );
            (
                line_idx,
                line_idx + 1,
                vec![format!("mv{}", var_idx)],
                true,
                false,
            )
        } else if op == 1 {
            let table_start = prose_count + var_count + 2;
            let data_line = table_start + 2 + (iter * 9 % table_rows.saturating_sub(4)).max(1);
            lines[data_line] = format!("| r{} | {} | |", data_line - table_start - 1, 5000 + iter);
            (data_line, data_line + 1, Vec::new(), false, false)
        } else if iter % 2 == 0 {
            let insert_at = prose_count / 2 + (iter % prose_count.max(1));
            lines.insert(insert_at, format!("inserted prose {}", iter));
            (
                insert_at,
                (insert_at + 1).min(lines.len()),
                Vec::new(),
                false,
                false,
            )
        } else {
            let remove_at = prose_count / 2 + (iter % prose_count.max(1));
            if remove_at < lines.len() {
                lines.remove(remove_at);
            }
            let to = (remove_at + 1).min(lines.len());
            (remove_at.min(lines.len()), to, Vec::new(), false, false)
        };

        let sync_start = Instant::now();
        sync_variable_dependency_graph(&mut variable_graph, &lines, changed_from, changed_to, mask);
        sync_table_formula_dependency_index(
            &mut table_index,
            &lines,
            changed_from,
            changed_to,
            mask,
        );
        sync_ms.push(sync_start.elapsed().as_secs_f64() * 1000.0);

        let cached_start = Instant::now();
        let cached = decide_eval_window_with_cached_dependency_indexes_and_flags(
            variable_graph.as_ref(),
            table_index.as_ref(),
            &lines,
            changed_from,
            changed_to,
            &prev_names,
            prev_had_assignment,
            prev_had_builtin,
            true,
            true,
            true,
        );
        cached_ms.push(cached_start.elapsed().as_secs_f64() * 1000.0);

        let uncached_start = Instant::now();
        let uncached = decide_eval_window_with_flags(
            &lines,
            changed_from,
            changed_to,
            &prev_names,
            prev_had_assignment,
            prev_had_builtin,
            true,
            true,
            true,
        );
        uncached_ms.push(uncached_start.elapsed().as_secs_f64() * 1000.0);

        assert_eq!(cached.eval_from, uncached.eval_from);
        assert_eq!(cached.eval_to, uncached.eval_to);
        assert_eq!(cached.can_use_partial, uncached.can_use_partial);
        spans.push(cached.eval_to.saturating_sub(cached.eval_from));
    }

    vec![
        summarize(
            "sync_variable_and_table_indexes",
            "mixed_structural_content",
            sync_ms,
            spans.clone(),
        ),
        summarize(
            "decide_eval_window_cached",
            "mixed_structural_content",
            cached_ms,
            spans.clone(),
        ),
        summarize(
            "decide_eval_window_uncached",
            "mixed_structural_content",
            uncached_ms,
            spans,
        ),
    ]
}

fn parse_args() -> (BenchProfile, bool) {
    let mut profile = BenchProfile::Default;
    let mut json = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--profile" => {
                let value = args.next().unwrap_or_else(|| {
                    eprintln!("missing value for --profile (expected `default` or `ci`)");
                    std::process::exit(2);
                });
                profile = match value.as_str() {
                    "default" => BenchProfile::Default,
                    "ci" => BenchProfile::Ci,
                    other => {
                        eprintln!("unknown profile `{other}` (expected `default` or `ci`)");
                        std::process::exit(2);
                    }
                };
            }
            "--help" | "-h" => {
                println!(
                    "Usage: cargo run -p slate --bin calc-perf -- [--profile default|ci] [--json]"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown arg `{other}`");
                std::process::exit(2);
            }
        }
    }
    (profile, json)
}

fn main() {
    let (profile, json) = parse_args();
    if !json {
        println!("calc perf");
        println!(
            "profile={}",
            match profile {
                BenchProfile::Default => "default",
                BenchProfile::Ci => "ci",
            }
        );
    }

    let mut stats = Vec::new();
    stats.extend(bench_variable_chain(profile));
    stats.extend(bench_table_coord(profile));
    stats.extend(bench_mixed_structural_and_content_edits(profile));

    if json {
        let report = PerfReport {
            profile: match profile {
                BenchProfile::Default => "default".to_string(),
                BenchProfile::Ci => "ci".to_string(),
            },
            stats,
        };
        let out = serde_json::to_string_pretty(&report).expect("serialize calc perf report");
        println!("{out}");
    } else {
        for stat in &stats {
            print_stat(stat);
        }
    }
}
