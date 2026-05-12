use app_core::calc::{CalcEngine, NoteEvaluationOptions};
use app_core::config;
use app_core::storage::Note;
use app_core::AppCore;
use editor_core::calc_plan::{
    build_calc_dependency_index, decide_eval_window, line_metadata_with_mask,
    sync_calc_dependency_index, CalcFeatureMask, DecideEvalWindowParams,
};
use std::fmt::Write as _;
use std::time::Instant;

#[derive(Debug, Clone)]
struct StartupMark {
    name: String,
    ms: f64,
}

#[derive(Debug, Clone)]
struct StartupReport {
    mode: String,
    marks: Vec<StartupMark>,
}

struct Probe {
    start: Instant,
    marks: Vec<StartupMark>,
}

impl Probe {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            marks: Vec::new(),
        }
    }

    fn mark(&mut self, name: &str) {
        let elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        self.mark_ms(name, elapsed_ms);
    }

    fn mark_ms(&mut self, name: &str, ms: f64) {
        self.marks.push(StartupMark {
            name: name.to_string(),
            ms,
        });
    }

    fn finish(self, mode: &str) -> StartupReport {
        StartupReport {
            mode: mode.to_string(),
            marks: self.marks,
        }
    }
}

fn split_lines(body: &str) -> Vec<String> {
    if body.is_empty() {
        vec![String::new()]
    } else {
        body.split('\n').map(|line| line.to_string()).collect()
    }
}

fn collect_note_calc_marks(probe: &mut Probe, note: &Note) {
    let mask = CalcFeatureMask {
        math_enabled: note.modules.math,
        table_enabled: note.modules.table,
        variables_enabled: note.modules.variables,
    };
    if !mask.math_enabled {
        probe.mark_ms("note_calc_skipped_ms", 0.0);
        return;
    }

    let lines = split_lines(&note.body);
    let changed_from = if lines.is_empty() { 0 } else { lines.len() / 2 };
    let changed_to = (changed_from + 1).min(lines.len());

    let build_started = Instant::now();
    let mut dep_index = build_calc_dependency_index(&lines, mask);
    probe.mark_ms(
        "note_calc_dependency_index_build_ms",
        build_started.elapsed().as_secs_f64() * 1000.0,
    );

    let prev_line_meta = lines
        .get(changed_from.min(lines.len().saturating_sub(1)))
        .map(|line| line_metadata_with_mask(line, mask));
    let prev_assignment_names = prev_line_meta
        .as_ref()
        .and_then(|meta| meta.assignment_name.clone())
        .into_iter()
        .collect::<Vec<_>>();
    let prev_had_assignment = prev_line_meta
        .as_ref()
        .map(|meta| meta.has_assignment)
        .unwrap_or(false);
    let prev_had_builtin_formula = prev_line_meta
        .as_ref()
        .map(|meta| meta.has_builtin_formula)
        .unwrap_or(false);

    let cached_started = Instant::now();
    let _cached = decide_eval_window(
        &DecideEvalWindowParams {
            lines: &lines,
            changed_from,
            changed_to,
            has_prev: true,
            mask,
            prev_changed_assignment_names: &prev_assignment_names,
            prev_changed_had_assignment: prev_had_assignment,
            prev_changed_had_builtin_formula: prev_had_builtin_formula,
            variable_graph: None,
            table_formula_index: None,
        }
        .with_calc_dependency_index(dep_index.as_ref()),
    );
    probe.mark_ms(
        "note_calc_eval_window_cached_ms",
        cached_started.elapsed().as_secs_f64() * 1000.0,
    );

    let uncached_started = Instant::now();
    let _uncached = decide_eval_window(&DecideEvalWindowParams {
        lines: &lines,
        changed_from,
        changed_to,
        has_prev: true,
        mask,
        prev_changed_assignment_names: &prev_assignment_names,
        prev_changed_had_assignment: prev_had_assignment,
        prev_changed_had_builtin_formula: prev_had_builtin_formula,
        variable_graph: None,
        table_formula_index: None,
    });
    probe.mark_ms(
        "note_calc_eval_window_uncached_ms",
        uncached_started.elapsed().as_secs_f64() * 1000.0,
    );

    let sync_started = Instant::now();
    sync_calc_dependency_index(&mut dep_index, &lines, changed_from, changed_to, mask);
    probe.mark_ms(
        "note_calc_dependency_index_sync_ms",
        sync_started.elapsed().as_secs_f64() * 1000.0,
    );

    let eval_started = Instant::now();
    let _ = CalcEngine::new().evaluate_note_context(
        &lines,
        NoteEvaluationOptions {
            variables_enabled: note.modules.variables,
            table_enabled: note.modules.table,
            eval_range: None,
        },
    );
    probe.mark_ms(
        "note_calc_full_eval_ms",
        eval_started.elapsed().as_secs_f64() * 1000.0,
    );
}

fn probe_gui() -> Result<StartupReport, String> {
    let mut probe = Probe::new();
    probe.mark("probe_start");

    config::ensure_config_file()?;
    probe.mark("config_ensured");

    let _cfg = config::load_theme_config();
    probe.mark("config_loaded");

    let core = AppCore::open_default()?;
    probe.mark("app_core_opened");

    let note = core.db().get_most_recent_note()?;
    probe.mark("note_fetch_complete");
    if let Some(note) = note.as_ref() {
        collect_note_calc_marks(&mut probe, note);
    }

    Ok(probe.finish("gui"))
}

fn probe_tui() -> Result<StartupReport, String> {
    let mut probe = Probe::new();
    probe.mark("probe_start");

    config::ensure_config_file()?;
    probe.mark("config_ensured");

    let _cfg = config::load_theme_config();
    probe.mark("config_loaded");

    let core = AppCore::open_default()?;
    probe.mark("app_core_opened");

    let note = core.db().get_most_recent_note()?;
    probe.mark("note_fetch_complete");

    let line_count = note
        .as_ref()
        .map(|n| {
            if n.body.is_empty() {
                1usize
            } else {
                n.body.lines().count().max(1)
            }
        })
        .unwrap_or(1);

    if line_count > 0 {
        probe.mark("line_split_ready");
    }
    if let Some(note) = note.as_ref() {
        collect_note_calc_marks(&mut probe, note);
    }

    Ok(probe.finish("tui"))
}

fn usage() {
    eprintln!("Usage: note-startup <gui|tui>");
}

fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn report_to_json(report: &StartupReport) -> String {
    let mut out = String::new();
    out.push_str("{\"mode\":\"");
    out.push_str(&json_escape(&report.mode));
    out.push_str("\",\"marks\":[");
    for (idx, mark) in report.marks.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":\"");
        out.push_str(&json_escape(&mark.name));
        out.push_str("\",\"ms\":");
        let _ = write!(out, "{}", mark.ms);
        out.push('}');
    }
    out.push_str("]}");
    out
}

fn main() {
    let mode = std::env::args().nth(1);
    let result = match mode.as_deref() {
        Some("gui") => probe_gui(),
        Some("tui") => probe_tui(),
        _ => {
            usage();
            std::process::exit(2);
        }
    };

    match result {
        Ok(report) => {
            println!("{}", report_to_json(&report));
        }
        Err(err) => {
            eprintln!("Startup probe failed: {err}");
            std::process::exit(1);
        }
    }
}
