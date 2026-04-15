use app_core::config;
use app_core::AppCore;
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
        self.marks.push(StartupMark {
            name: name.to_string(),
            ms: elapsed_ms,
        });
    }

    fn finish(self, mode: &str) -> StartupReport {
        StartupReport {
            mode: mode.to_string(),
            marks: self.marks,
        }
    }
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

    let _note = core.db().get_most_recent_note()?;
    probe.mark("note_fetch_complete");

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
