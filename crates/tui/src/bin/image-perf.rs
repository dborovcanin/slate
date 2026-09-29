use editor_core::markdown_tokens::find_markdown_image_matches;
use std::time::Instant;

fn main() {
    let note_lines = env_usize("SLATE_IMAGE_PERF_LINES", 100_000);
    let viewport_lines = env_usize("SLATE_IMAGE_PERF_VIEWPORT_LINES", 24).min(note_lines);
    let iterations = env_usize("SLATE_IMAGE_PERF_ITERATIONS", 1_000).max(1);
    let mut lines = Vec::with_capacity(note_lines);
    for line in 0..note_lines {
        lines.push(format!("row {line}: ![diagram](./assets/{line}.png)"));
    }

    let mut samples = Vec::with_capacity(iterations);
    let mut matches = 0usize;
    for iteration in 0..iterations {
        let max_start = note_lines.saturating_sub(viewport_lines);
        let start = if max_start == 0 {
            0
        } else {
            iteration.wrapping_mul(7919) % (max_start + 1)
        };
        let began = Instant::now();
        for line in &lines[start..start + viewport_lines] {
            matches += find_markdown_image_matches(line).len();
        }
        samples.push(began.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let p95 = samples[(samples.len() - 1) * 95 / 100];
    println!(
        "image_viewport_parse note_lines={note_lines} refs={note_lines} viewport_lines={viewport_lines} iterations={iterations} p95_ms={p95:.3} matches={matches}"
    );
}

fn env_usize(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}
