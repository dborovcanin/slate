use app_core::config::load_perf_config;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
struct PerfRuntimeConfig {
    enabled: bool,
    log_path: String,
}

fn resolve_log_path(raw: &str, fallback_name: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        std::env::temp_dir()
            .join(fallback_name)
            .to_string_lossy()
            .to_string()
    } else {
        PathBuf::from(trimmed).to_string_lossy().to_string()
    }
}

fn main() {
    let perf = load_perf_config();
    let out = PerfRuntimeConfig {
        enabled: perf.enabled,
        log_path: resolve_log_path(&perf.log_path, "slate-log.log"),
    };

    let json = serde_json::to_string_pretty(&out).expect("serialize perf config");
    println!("{json}");
}
