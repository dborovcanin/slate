use editor_core::context::ResolvedContext;
use editor_core::table::{
    format_table_lines_with_cache, table_cell_cursor_info_in_document_cached, TableFormatCache,
    TableLogicalRowCache,
};
use editor_core::text_rules::{run_doc_change_rules_with_table_cache, TextRuleOptions};
use editor_core::types::{EditorContextSnapshot, SelectionSnapshot, TextRange};
use std::time::Instant;

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

fn summarize(name: &str, samples_ms: &mut [f64]) {
    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let count = samples_ms.len();
    let p50 = percentile(samples_ms, 50.0);
    let p95 = percentile(samples_ms, 95.0);
    let p99 = percentile(samples_ms, 99.0);
    let max = samples_ms.last().copied().unwrap_or(0.0);
    let avg = if count == 0 {
        0.0
    } else {
        samples_ms.iter().sum::<f64>() / count as f64
    };
    println!(
        "{name}: n={count} avg={avg:.3}ms p50={p50:.3}ms p95={p95:.3}ms p99={p99:.3}ms max={max:.3}ms"
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

fn bench_format(rows: usize, cols: usize, continuation_every: usize, iters: usize) {
    let input = make_table(rows, cols, continuation_every);
    let mut cache = TableFormatCache::default();
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let start = Instant::now();
        let _ = format_table_lines_with_cache(&input, &mut cache);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    summarize(
        &format!("format_table_lines_with_cache rows={rows} cols={cols}"),
        &mut samples,
    );
}

fn bench_cursor_lookup(rows: usize, cols: usize, continuation_every: usize, iters: usize) {
    let lines = make_table(rows, cols, continuation_every);
    let mut cache = TableLogicalRowCache::default();
    let mut samples = Vec::with_capacity(iters);
    for i in 0..iters {
        let line_idx = 2 + (i % rows.max(1));
        let col = 8 + (i % cols.max(1)) * 6;
        let start = Instant::now();
        let _ = table_cell_cursor_info_in_document_cached(&lines, line_idx, col, &mut cache);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    summarize(
        &format!("table_cell_cursor_info_cached rows={rows} cols={cols}"),
        &mut samples,
    );
}

fn bench_doc_change_rule(rows: usize, cols: usize, continuation_every: usize, iters: usize) {
    let mut table_lines = make_table(rows, cols, continuation_every);
    let mut table_cache = TableFormatCache::default();
    let mut samples = Vec::with_capacity(iters);
    for i in 0..iters {
        let target = 2 + (i % rows.max(1));
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
    summarize(
        &format!("run_doc_change_rules_with_cache rows={rows} cols={cols}"),
        &mut samples,
    );
}

fn main() {
    println!("multirow table perf");
    let scenarios = [(200usize, 6usize, 5usize), (500, 6, 4), (1000, 8, 3)];
    for (rows, cols, continuation_every) in scenarios {
        println!("\nscenario rows={rows} cols={cols} continuation_every={continuation_every}");
        bench_format(rows, cols, continuation_every, 60);
        bench_cursor_lookup(rows, cols, continuation_every, 600);
        bench_doc_change_rule(rows, cols, continuation_every, 40);
    }
}
