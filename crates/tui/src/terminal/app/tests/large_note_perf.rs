//! Large-note latency check. Ignored by default because it needs a release
//! build to be meaningful:
//!
//! ```sh
//! cargo test --release -p slate --lib large_note_perf -- --ignored --nocapture
//! ```
//!
//! Builds a generated note (prose, images, tables, table formulas, variables,
//! wiki links and cross-note references to two linked notes), drives it with
//! real key presses, and compares each scenario's p95 (one sample = the
//! action's key handling plus the frame painted after it) against
//! `perf/baselines/large_note.json`. `SLATE_LARGE_NOTE_SIZES=30000,200000`
//! overrides the line counts; sizes without limits are reported only.

use super::*;
use crate::terminal::input::{set_terminal_size, Key};
use serde_json::Value;
use std::collections::BTreeMap;

const ROWS: u16 = 50;
const COLS: u16 = 160;
/// Longer than the autosave and calc debounces so an idle tick does the
/// deferred work a real pause would trigger.
const IDLE_PAUSE: Duration = Duration::from_millis(600);
const IDLE_TICKS: usize = 3;
const OPEN_RUNS: usize = 3;

/// 1x1 transparent PNG.
const PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];
const IMAGE_COUNT: usize = 4;
const REFERENCE_TOPICS: usize = 40;
/// Sections per running-total chain. Real notes chain totals within a
/// chapter, not across the whole document.
const CHAPTER_SECTIONS: usize = 25;

struct Fixture {
    db: Db,
    path: PathBuf,
    main_id: String,
    body: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        cleanup_db_files(&self.path);
    }
}

/// Links address notes by the first 8 ULID chars, which only change every
/// ~17 minutes, so ids made together would share one short id.
fn note_id(days_ago: u64) -> String {
    let now = Ulid::new();
    Ulid::from_parts(now.timestamp_ms() - days_ago * 86_400_000, now.random()).to_string()
}

fn short_id(id: &str) -> &str {
    &id[..8]
}

fn rates_note() -> String {
    [
        "# Rates",
        "",
        "## Budget",
        "budget := 5000",
        "monthly income := 4200",
        "",
        "## Taxes",
        "vat := 0.2",
        "fee rate := 0.035",
    ]
    .join("\n")
}

fn reference_note() -> String {
    let mut lines = vec!["# Reference".to_string()];
    for topic in 0..REFERENCE_TOPICS {
        lines.push(String::new());
        lines.push(format!("## Topic {topic}"));
        lines.push(format!(
            "Notes about topic {topic} with a [source](https://example.com/t/{topic})."
        ));
    }
    lines.join("\n")
}

/// One repeating block of the main note. Every block defines its own
/// variables, reassigns `last cost` (so the note has duplicate assignments,
/// like real notes do), and carries a running total over the previous blocks
/// of its chapter.
fn push_section(out: &mut Vec<String>, i: usize, rates: &str, reference: &str, images: &[String]) {
    let price = 10 + i % 90;
    let qty = 1 + i % 7;
    let topic = i % REFERENCE_TOPICS;
    let image = &images[i % images.len()];
    let running_total = if i % CHAPTER_SECTIONS == 1 {
        String::new()
    } else {
        format!(" + total {}", i - 1)
    };
    out.extend([
        format!("## Section {i}"),
        String::new(),
        format!(
            "Paragraph {i} has **bold**, *italic* and `code` text, a [link](https://example.com/{i}) \
             and a wiki link to [[{reference}#Topic {topic}]] so link hiding has work to do."
        ),
        format!("See [[{rates}#Budget|the budget]] and [[{reference}]] for context."),
        String::new(),
        format!("![diagram {i}](slate-image://{image})"),
        String::new(),
        format!("- item {i} one"),
        format!("- [ ] task {i} pending"),
        format!("- [x] task {i} done"),
        String::new(),
        format!("price {i} := {price}"),
        format!("qty {i} := {qty}"),
        format!("cost {i} := price {i} * qty {i}"),
        format!("total {i} := cost {i} * (1 + tax rate){running_total}"),
        format!("last cost := cost {i}"),
        format!("cost {i} * [[{rates}]].vat"),
        format!("[[{rates}]].budget - total {i}"),
        format!("{price} * {qty} + {topic} / 4"),
        String::new(),
        "| item | qty | price | line total |".to_string(),
        "| --- | --- | --- | --- |".to_string(),
        format!("| a{i} | {qty} | {price} | :=(1,2) * (1,3) |"),
        format!("| b{i} | {} | 2.5 | :=(2,2) * (2,3) |", qty + 1),
        format!("| c{i} | 3 | {} | :=(3,2) * (3,3) |", price / 2),
        "| sum | :=sum_col() |  | :=sum_col() |".to_string(),
        String::new(),
        "| key | value |".to_string(),
        "| --- | --- |".to_string(),
        format!("| id | {i} |"),
        format!("| topic | {topic} |"),
        String::new(),
        "```rust".to_string(),
        format!("fn section_{i}() -> u32 {{ {i} }}"),
        "```".to_string(),
        String::new(),
        format!("> quote {i}: remember total {i}"),
        String::new(),
    ]);
}

fn main_note(lines: usize, rates: &str, reference: &str, images: &[String]) -> String {
    let mut out = vec![
        "# Large note".to_string(),
        String::new(),
        "tax rate := 0.25".to_string(),
        String::new(),
    ];
    let mut i = 1;
    while out.len() < lines {
        push_section(&mut out, i, rates, reference, images);
        i += 1;
    }
    out.truncate(lines);
    out.join("\n")
}

fn build_fixture(lines: usize) -> Fixture {
    let path = temp_db_path();
    let db = Db::open(path.clone()).expect("db opens");
    let rates_id = note_id(2);
    let reference_id = note_id(1);
    let main_id = note_id(0);
    db.save_note(&rates_id, &rates_note()).expect("rates note");
    db.save_note(&reference_id, &reference_note())
        .expect("reference note");
    db.save_note(&main_id, "# Large note").expect("main note");
    let images = (0..IMAGE_COUNT)
        .map(|n| {
            let name = format!("diagram-{n}.png");
            let image_id = db
                .reserve_note_image(&main_id, Some(&name), Some("image/png"))
                .expect("image reserved");
            db.write_note_image_bytes(
                &main_id,
                &image_id,
                Some(&name),
                Some("image/png"),
                PIXEL_PNG,
            )
            .expect("image written");
            image_id
        })
        .collect::<Vec<_>>();
    let body = main_note(lines, short_id(&rates_id), short_id(&reference_id), &images);
    db.save_note(&main_id, &body).expect("main note body");
    Fixture {
        db,
        path,
        main_id,
        body,
    }
}

fn open_app(fixture: &Fixture) -> TerminalApp {
    let opts = TerminalOptions {
        create_new: false,
        note_id: Some(fixture.main_id.clone()),
        list_only: false,
        open_switcher: false,
        open_at_end: false,
    };
    let (mut app, _) = TerminalApp::new_with_startup_metrics(
        &fixture.db,
        &opts,
        crate::config::ThemeConfig::default(),
        true,
        true,
        false,
        true,
        true,
        3,
        super::super::render::RenderPalette::default(),
        "%Y-%m-%d".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
        std::sync::Arc::new(std::sync::Mutex::new(
            app_core::cross_note::CrossNoteVarIndex::default(),
        )),
    )
    .expect("terminal app");
    app.mode = UiMode::Normal;
    app.perf_trace.enabled = false;
    app
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

#[derive(Default)]
struct Samples(BTreeMap<&'static str, Vec<f64>>);

impl Samples {
    fn add(&mut self, metric: &'static str, value_ms: f64) {
        self.0.entry(metric).or_default().push(value_ms);
    }
}

/// Runs each action (a key sequence) and records its keys plus one repaint
/// as a single sample.
fn measure(
    app: &mut TerminalApp,
    db: &Db,
    samples: &mut Samples,
    metric: &'static str,
    actions: &[Vec<Key>],
) {
    for keys in actions {
        let started = Instant::now();
        for key in keys {
            app.handle_key(db, key.clone()).expect("key handled");
        }
        render_screen(app);
        samples.add(metric, ms(started.elapsed()));
    }
}

/// Waits past the debounces, then times the idle ticks that run the deferred
/// calc, folding and autosave work (each blocks input while it runs).
fn measure_idle(app: &mut TerminalApp, db: &Db, samples: &mut Samples) {
    std::thread::sleep(IDLE_PAUSE);
    for _ in 0..IDLE_TICKS {
        let started = Instant::now();
        app.maybe_autosave(db).expect("idle tick");
        samples.add("idle_tick", ms(started.elapsed()));
    }
    render_screen(app);
}

fn keys(text: &str) -> Vec<Key> {
    text.chars().map(Key::Char).collect()
}

/// One action per typed character.
fn each_key(text: &str) -> Vec<Vec<Key>> {
    text.chars().map(|ch| vec![Key::Char(ch)]).collect()
}

fn repeat(action: Vec<Key>, times: usize) -> Vec<Vec<Key>> {
    vec![action; times]
}

/// Moves the cursor to `line` (0-based) without recording a sample.
fn goto_line(app: &mut TerminalApp, db: &Db, line: usize) {
    for key in keys(&format!("{}G", line + 1)) {
        app.handle_key(db, key).expect("goto");
    }
    render_screen(app);
}

/// First line at or after the middle of the note that starts with `prefix`.
fn line_near_middle(body: &str, prefix: &str) -> usize {
    let lines: Vec<&str> = body.lines().collect();
    let mid = lines.len() / 2;
    (mid..lines.len())
        .chain(0..mid)
        .find(|&idx| lines[idx].starts_with(prefix))
        .unwrap_or_else(|| panic!("fixture has no line starting with {prefix:?}"))
}

fn run_size(lines: usize) -> Samples {
    set_terminal_size(ROWS, COLS);
    let fixture = build_fixture(lines);
    let db = fixture.db.clone();
    let mut samples = Samples::default();

    for _ in 0..OPEN_RUNS {
        let started = Instant::now();
        let mut app = open_app(&fixture);
        render_screen(&mut app);
        samples.add("open", ms(started.elapsed()));
        // Large notes paint first and fill in calc values when the
        // background preparation lands.
        wait_for_viewport_calc(&mut app);
        render_screen(&mut app);
        samples.add("open_calc", ms(started.elapsed()));
    }

    let mut app = open_app(&fixture);
    render_screen(&mut app);
    wait_for_viewport_calc(&mut app);
    measure_idle(&mut app, &db, &mut samples);

    // Navigation. Each scenario must actually move, or it measures nothing.
    let before = app.editor.cursor_line;
    measure(
        &mut app,
        &db,
        &mut samples,
        "scroll_line",
        &repeat(keys("j"), 200),
    );
    assert_eq!(app.editor.cursor_line, before + 200, "j did not scroll");
    // Normal mode has no page motions, so a counted motion stands in.
    let before = app.editor.cursor_line;
    measure(
        &mut app,
        &db,
        &mut samples,
        "scroll_jump",
        &repeat(keys("40j"), 20),
    );
    assert_eq!(app.editor.cursor_line, before + 800, "40j did not scroll");
    measure(
        &mut app,
        &db,
        &mut samples,
        "jump_top_bottom",
        &[keys("G"), keys("gg")]
            .into_iter()
            .cycle()
            .take(10)
            .collect::<Vec<_>>(),
    );
    measure(
        &mut app,
        &db,
        &mut samples,
        "search",
        &[
            [vec![Key::Char('/')], keys("Section 1234"), vec![Key::Enter]].concat(),
            keys("n"),
            keys("n"),
            keys("N"),
        ],
    );

    // Typing into prose, a variable assignment and a table cell.
    goto_line(&mut app, &db, line_near_middle(&fixture.body, "Paragraph "));
    app.handle_key(&db, Key::Char('A')).expect("append");
    measure(
        &mut app,
        &db,
        &mut samples,
        "type_prose",
        &each_key(" more words"),
    );
    app.handle_key(&db, Key::Esc).expect("esc");

    goto_line(&mut app, &db, line_near_middle(&fixture.body, "price "));
    app.handle_key(&db, Key::Char('A')).expect("append");
    measure(&mut app, &db, &mut samples, "type_calc", &each_key("5 + 1"));
    app.handle_key(&db, Key::Esc).expect("esc");

    goto_line(&mut app, &db, line_near_middle(&fixture.body, "| a"));
    for key in keys("0f|;l") {
        app.handle_key(&db, key).expect("move into qty cell");
    }
    app.handle_key(&db, Key::Char('a')).expect("append in cell");
    measure(&mut app, &db, &mut samples, "type_table", &each_key("12"));
    app.handle_key(&db, Key::Esc).expect("esc");
    measure_idle(&mut app, &db, &mut samples);

    // Structural edits: these change the line count.
    goto_line(&mut app, &db, line_near_middle(&fixture.body, "- item "));
    app.handle_key(&db, Key::Char('A')).expect("append");
    measure(
        &mut app,
        &db,
        &mut samples,
        "enter",
        &repeat(vec![Key::Enter], 10),
    );
    app.handle_key(&db, Key::Esc).expect("esc");
    measure(
        &mut app,
        &db,
        &mut samples,
        "open_line",
        &repeat([keys("o"), keys("new line"), vec![Key::Esc]].concat(), 5),
    );
    measure(
        &mut app,
        &db,
        &mut samples,
        "delete_line",
        &repeat(keys("dd"), 10),
    );
    measure(
        &mut app,
        &db,
        &mut samples,
        "paste_line",
        &repeat(keys("p"), 5),
    );
    measure(
        &mut app,
        &db,
        &mut samples,
        "delete_char",
        &repeat(keys("x"), 20),
    );
    measure(&mut app, &db, &mut samples, "undo", &repeat(keys("u"), 10));
    measure(
        &mut app,
        &db,
        &mut samples,
        "redo",
        &repeat(vec![Key::Ctrl('r')], 10),
    );
    measure_idle(&mut app, &db, &mut samples);

    samples
}

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = pct / 100.0 * (sorted.len() - 1) as f64;
    let (lo, hi) = (rank.floor() as usize, rank.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
}

fn load_limits() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../perf/baselines/large_note.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn sizes(limits: &Value) -> Vec<usize> {
    if let Ok(raw) = std::env::var("SLATE_LARGE_NOTE_SIZES") {
        return raw
            .split(',')
            .map(|size| {
                size.trim()
                    .parse()
                    .expect("SLATE_LARGE_NOTE_SIZES is a list of line counts")
            })
            .collect();
    }
    limits["sizes"]
        .as_array()
        .expect("baseline lists sizes")
        .iter()
        .map(|size| size.as_u64().expect("size is a line count") as usize)
        .collect()
}

#[test]
#[ignore = "perf check; run in release with --ignored"]
fn large_note_perf() {
    let limits = load_limits();
    let mut failures = Vec::new();
    for lines in sizes(&limits) {
        let samples = run_size(lines);
        let size_limits = limits["limits_p95_ms"][lines.to_string()].as_object();
        eprintln!("\nlarge note: {lines} lines");
        eprintln!(
            "  {:<18} {:>5} {:>9} {:>9} {:>9} {:>9}",
            "metric", "n", "p50 ms", "p95 ms", "max ms", "limit"
        );
        for (metric, values) in &samples.0 {
            let mut sorted = values.clone();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let p95 = percentile(&sorted, 95.0);
            let limit = size_limits
                .and_then(|map| map.get(*metric))
                .and_then(Value::as_f64);
            let verdict = match limit {
                Some(limit) if p95 > limit => {
                    failures.push(format!(
                        "{lines} lines {metric}: p95 {p95:.2}ms > {limit}ms"
                    ));
                    "FAIL"
                }
                Some(_) => "ok",
                None => "-",
            };
            eprintln!(
                "  {:<18} {:>5} {:>9.2} {:>9.2} {:>9.2} {:>9} {verdict}",
                metric,
                sorted.len(),
                percentile(&sorted, 50.0),
                p95,
                sorted.last().copied().unwrap_or(0.0),
                limit
                    .map(|l| format!("{l}"))
                    .unwrap_or_else(|| "-".to_string()),
            );
        }
        for metric in size_limits.into_iter().flat_map(|map| map.keys()) {
            if !samples.0.contains_key(metric.as_str()) {
                failures.push(format!(
                    "{lines} lines: limit set for unmeasured metric {metric}"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "large-note perf regressions:\n{}",
        failures.join("\n")
    );
}

#[test]
fn large_note_fixture_covers_structured_content() {
    let images = vec!["IMG0".to_string()];
    let body = main_note(1_200, "RATES000", "REFER000", &images);
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 1_200);
    let has = |needle: &str| lines.iter().any(|line| line.contains(needle));
    assert!(has("![diagram 1](slate-image://IMG0)"));
    assert!(has("[[REFER000#Topic 1]]"));
    assert!(has("[[RATES000]].vat"));
    assert!(has(":=(1,2) * (1,3)"));
    assert!(has(":=sum_col()"));
    assert!(lines.contains(&"total 1 := cost 1 * (1 + tax rate)"));
    assert!(has("total 2 := cost 2 * (1 + tax rate) + total 1"));
    assert!(lines.contains(&"total 26 := cost 26 * (1 + tax rate)"));
    assert!(
        lines
            .iter()
            .filter(|line| line.starts_with("last cost :="))
            .count()
            > 1
    );
}

#[test]
fn large_note_fixture_evaluates_in_the_app() {
    // Keeps the default 24x80 terminal: the size is process-wide and other
    // tests running in parallel depend on it.
    let fixture = build_fixture(2_500);
    let db = fixture.db.clone();
    let mut app = open_app(&fixture);
    // Link titles and the deferred calc pass load on the idle ticks after open.
    measure_idle(&mut app, &db, &mut Samples::default());
    let line = |prefix: &str| {
        app.editor
            .lines
            .iter()
            .position(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("no line starting with {prefix:?}"))
    };
    let (vat, budget, table) = (line("cost 1 * [["), line("[["), line("| a1 |"));
    assert!(app.calc.results[vat].is_some(), "cross-note vat unresolved");
    assert!(
        app.calc.results[budget].is_some(),
        "cross-note budget unresolved"
    );
    assert!(
        !app.calc.cell_results[table].is_empty(),
        "table formula not evaluated"
    );
    let screen = screen_text(&mut app);
    assert!(
        screen.contains("[image: diagram 1]"),
        "image not collapsed:\n{screen}"
    );
    assert!(
        screen.contains("See the budget and Reference for context."),
        "wiki link unresolved:\n{screen}"
    );
    assert!(
        !screen.contains("?.vat"),
        "cross-note ref unresolved:\n{screen}"
    );
}
