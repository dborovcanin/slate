use app_core::config;
use app_core::AppCore;
use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Clone, Serialize)]
struct StartupMark {
    name: String,
    ms: f64,
}

#[derive(Debug, Clone, Serialize)]
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
            println!(
                "{}",
                serde_json::to_string(&report).unwrap_or_else(|_| "{}".to_string())
            );
        }
        Err(err) => {
            eprintln!("Startup probe failed: {err}");
            std::process::exit(1);
        }
    }
}
