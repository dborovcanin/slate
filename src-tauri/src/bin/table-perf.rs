use editor_core::context::ResolvedContext;
use editor_core::table::{
    format_table_lines_with_cache, table_cell_cursor_info_in_document_cached, TableFormatCache,
    TableLogicalRowCache,
};
use editor_core::text_rules::{run_doc_change_rules_with_table_cache, TextRuleOptions};
use editor_core::types::{EditorContextSnapshot, SelectionSnapshot, TextRange};
use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchProfile {
    Default,
    Ci,
}

#[derive(Debug, Clone, Copy)]
struct Scenario {
    rows: usize,
    cols: usize,
    continuation_every: usize,
    format_iters: usize,
    cursor_iters: usize,
    doc_change_iters: usize,
}

#[derive(Debug, Clone, Serialize)]
struct BenchStat {
    metric: String,
    rows: usize,
    cols: usize,
    continuation_every: usize,
    n: usize,
    avg_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    cache_hits: Option<usize>,
    cache_misses: Option<usize>,
    cache_entries: Option<usize>,
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

fn summarize(
    metric: &str,
    scenario: Scenario,
    mut samples_ms: Vec<f64>,
    cache_meta: Option<(usize, usize, usize)>,
) -> BenchStat {
    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = samples_ms.len();
    let p50 = percentile(&samples_ms, 50.0);
    let p95 = percentile(&samples_ms, 95.0);
    let p99 = percentile(&samples_ms, 99.0);
    let max = samples_ms.last().copied().unwrap_or(0.0);
    let avg = if n == 0 {
        0.0
    } else {
        samples_ms.iter().sum::<f64>() / n as f64
    };
    let (cache_hits, cache_misses, cache_entries) = cache_meta
        .map(|(hits, misses, entries)| (Some(hits), Some(misses), Some(entries)))
        .unwrap_or((None, None, None));
    BenchStat {
        metric: metric.to_string(),
        rows: scenario.rows,
        cols: scenario.cols,
        continuation_every: scenario.continuation_every,
        n,
        avg_ms: avg,
        p50_ms: p50,
        p95_ms: p95,
        p99_ms: p99,
        max_ms: max,
        cache_hits,
        cache_misses,
        cache_entries,
    }
}

fn print_stat(stat: &BenchStat) {
    let mut suffix = String::new();
    if let (Some(hits), Some(misses), Some(entries)) =
        (stat.cache_hits, stat.cache_misses, stat.cache_entries)
    {
        let total = hits.saturating_add(misses);
        let hit_ratio = if total == 0 {
            0.0
        } else {
            (hits as f64 / total as f64) * 100.0
        };
        suffix =
            format!(" cache(hits={hits} misses={misses} entries={entries} hit={hit_ratio:.1}%)");
    }
    println!(
        "{} rows={} cols={}: n={} avg={:.3}ms p50={:.3}ms p95={:.3}ms p99={:.3}ms max={:.3}ms{}",
        stat.metric,
        stat.rows,
        stat.cols,
        stat.n,
        stat.avg_ms,
        stat.p50_ms,
        stat.p95_ms,
        stat.p99_ms,
        stat.max_ms,
        suffix,
    );
}

fn make_table(rows: usize, cols: usize, continuation_every: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(rows + 2 + rows / continuation_every.max(1));
    let header = (0..cols)
        .map(|idx| format!("c{}", idx + 1))
        .collect::<Vec<_>>();
    out.push(format!("| {} |", header.join(" | ")));
    out.push(format!("| {} |", vec!["---"; cols].join(" | ")));
    for row_idx in 0..rows {
        let mut row = Vec::with_capacity(cols);
        for col_idx in 0..cols {
            if col_idx == cols.saturating_sub(1) && row_idx % 17 == 0 {
                row.push("!ERROR#non_numeric".to_string());
            } else {
                row.push(format!("r{}c{}", row_idx + 1, col_idx + 1));
            }
        }
        out.push(format!("| {} |", row.join(" | ")));
        if continuation_every > 0 && row_idx % continuation_every == 0 {
            let mut cont = vec![String::from("> detail")];
            for col_idx in 1..cols {
                cont.push(format!("tail{}", col_idx));
            }
            out.push(format!("| {} |", cont.join(" | ")));
        }
    }
    out
}

fn build_note_with_table(table_lines: &[String], filler_lines: usize) -> String {
    let mut out = String::new();
    for idx in 0..filler_lines {
        out.push_str(&format!("prefix line {}\n", idx + 1));
    }
    out.push_str(&table_lines.join("\n"));
    out.push('\n');
    for idx in 0..filler_lines {
        out.push_str(&format!("suffix line {}\n", idx + 1));
    }
    out
}

fn bench_format(scenario: Scenario) -> BenchStat {
    let input = make_table(scenario.rows, scenario.cols, scenario.continuation_every);
    let mut cache = TableFormatCache::default();
    cache.reset_parsed_row_cache_stats();
    let mut samples = Vec::with_capacity(scenario.format_iters);
    for _ in 0..scenario.format_iters {
        let start = Instant::now();
        let _ = format_table_lines_with_cache(&input, &mut cache);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let (hits, misses) = cache.parsed_row_cache_stats();
    summarize(
        "format_table_lines_with_cache",
        scenario,
        samples,
        Some((hits, misses, cache.parsed_row_cache_size())),
    )
}

fn bench_cursor_lookup(scenario: Scenario) -> BenchStat {
    let lines = make_table(scenario.rows, scenario.cols, scenario.continuation_every);
    let mut cache = TableLogicalRowCache::default();
    let mut samples = Vec::with_capacity(scenario.cursor_iters);
    for i in 0..scenario.cursor_iters {
        let line_idx = 2 + (i % scenario.rows.max(1));
        let col = 8 + (i % scenario.cols.max(1)) * 6;
        let start = Instant::now();
        let _ = table_cell_cursor_info_in_document_cached(&lines, line_idx, col, &mut cache);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    summarize("table_cell_cursor_info_cached", scenario, samples, None)
}

fn bench_doc_change_rule(scenario: Scenario) -> BenchStat {
    let mut table_lines = make_table(scenario.rows, scenario.cols, scenario.continuation_every);
    let mut table_cache = TableFormatCache::default();
    table_cache.reset_parsed_row_cache_stats();
    let mut samples = Vec::with_capacity(scenario.doc_change_iters);
    for i in 0..scenario.doc_change_iters {
        let target = 2 + (i % scenario.rows.max(1));
        if let Some(line) = table_lines.get_mut(target) {
            let insert_at = line.rfind('|').unwrap_or(line.len().saturating_sub(1));
            line.insert_str(insert_at.saturating_sub(1), " x");
        }
        let note = build_note_with_table(&table_lines, 200);
        let anchor = note.find("x").unwrap_or(note.len().saturating_sub(1));
        let snapshot = EditorContextSnapshot {
            text: note,
            selection: SelectionSnapshot {
                anchor,
                head: anchor,
            },
            changed_range: Some(TextRange {
                from: anchor.saturating_sub(1),
                to: anchor,
            }),
        };
        let ctx = ResolvedContext::new(snapshot);
        let opts = TextRuleOptions {
            markdown_autoformat: true,
            checklist_auto_reorder: true,
            table_enabled: true,
        };
        let start = Instant::now();
        let _ = run_doc_change_rules_with_table_cache(&ctx, opts, &mut table_cache);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let (hits, misses) = table_cache.parsed_row_cache_stats();
    summarize(
        "run_doc_change_rules_with_cache",
        scenario,
        samples,
        Some((hits, misses, table_cache.parsed_row_cache_size())),
    )
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
                    _ => {
                        eprintln!("unsupported profile `{value}` (expected `default` or `ci`)");
                        std::process::exit(2);
                    }
                };
            }
            other => {
                eprintln!("unsupported argument `{other}`");
                std::process::exit(2);
            }
        }
    }
    (profile, json)
}

fn scenarios_for_profile(profile: BenchProfile) -> Vec<Scenario> {
    match profile {
        BenchProfile::Default => vec![
            Scenario {
                rows: 200,
                cols: 6,
                continuation_every: 5,
                format_iters: 60,
                cursor_iters: 600,
                doc_change_iters: 40,
            },
            Scenario {
                rows: 500,
                cols: 6,
                continuation_every: 4,
                format_iters: 60,
                cursor_iters: 600,
                doc_change_iters: 40,
            },
            Scenario {
                rows: 1000,
                cols: 8,
                continuation_every: 3,
                format_iters: 60,
                cursor_iters: 600,
                doc_change_iters: 40,
            },
        ],
        BenchProfile::Ci => vec![
            Scenario {
                rows: 300,
                cols: 6,
                continuation_every: 4,
                format_iters: 20,
                cursor_iters: 200,
                doc_change_iters: 20,
            },
            Scenario {
                rows: 800,
                cols: 8,
                continuation_every: 3,
                format_iters: 20,
                cursor_iters: 200,
                doc_change_iters: 20,
            },
        ],
    }
}

fn main() {
    let (profile, output_json) = parse_args();
    let scenarios = scenarios_for_profile(profile);
    let mut stats = Vec::new();

    if !output_json {
        println!("multirow table perf");
    }

    for scenario in scenarios {
        if !output_json {
            println!(
                "\nscenario rows={} cols={} continuation_every={} profile={}",
                scenario.rows,
                scenario.cols,
                scenario.continuation_every,
                match profile {
                    BenchProfile::Default => "default",
                    BenchProfile::Ci => "ci",
                }
            );
        }

        let format_stat = bench_format(scenario);
        let cursor_stat = bench_cursor_lookup(scenario);
        let doc_change_stat = bench_doc_change_rule(scenario);

        if !output_json {
            print_stat(&format_stat);
            print_stat(&cursor_stat);
            print_stat(&doc_change_stat);
        }

        stats.push(format_stat);
        stats.push(cursor_stat);
        stats.push(doc_change_stat);
    }

    if output_json {
        let report = PerfReport {
            profile: match profile {
                BenchProfile::Default => "default".to_string(),
                BenchProfile::Ci => "ci".to_string(),
            },
            stats,
        };
        let json = serde_json::to_string_pretty(&report).expect("serialize perf report");
        println!("{json}");
    }
}
