pub mod calc;
pub mod config;
pub mod cross_note;
pub mod daily;
pub mod history;
pub mod note_sources;
pub mod reminders;
pub mod storage;
pub mod web_search;

use calc::CalcEngine;
use cross_note::CrossNoteVarIndex;
use directories::ProjectDirs;
use note_sources::NoteSourceService;
use rustc_hash::FxHashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use storage::{Db, DbOpenMetrics};

#[derive(Debug, Clone, Copy, Default)]
pub struct AppCoreOpenMetrics {
    pub data_dir_ms: f64,
    pub db_open_ms: f64,
    pub note_sources_init_ms: f64,
    pub calc_engine_init_ms: f64,
    pub total_ms: f64,
    pub db: DbOpenMetrics,
}

pub struct AppCore {
    db: Db,
    note_sources: NoteSourceService,
    calc_engine: CalcEngine,
    /// Server-side line cache for the delta calc IPC protocol.
    /// Key: note_id. Value: current lines of that note as known to the server.
    pub note_line_cache: Mutex<FxHashMap<String, Arc<Vec<String>>>>,
    /// Cross-note variable index: tracks exports and inter-note dependencies.
    /// Wrapped in Arc so it can be shared with the TUI session.
    pub cross_note_var_index: Arc<Mutex<CrossNoteVarIndex>>,
}

impl AppCore {
    pub fn open_default() -> Result<Self, String> {
        let (core, _) = Self::open_default_with_metrics()?;
        Ok(core)
    }

    pub fn open_default_with_metrics() -> Result<(Self, AppCoreOpenMetrics), String> {
        let total_started = Instant::now();

        let data_dir_started = Instant::now();
        let dir = data_dir()?;
        let data_dir_ms = data_dir_started.elapsed().as_secs_f64() * 1000.0;

        let db_open_started = Instant::now();
        let (db, db_metrics) = Db::open_with_metrics(dir.join("notes.db"))?;
        let db_open_ms = db_open_started.elapsed().as_secs_f64() * 1000.0;

        let note_sources_started = Instant::now();
        let note_sources = NoteSourceService::new(db.clone());
        let note_sources_init_ms = note_sources_started.elapsed().as_secs_f64() * 1000.0;

        let calc_engine_started = Instant::now();
        let calc_engine = CalcEngine::new();
        let calc_engine_init_ms = calc_engine_started.elapsed().as_secs_f64() * 1000.0;

        Ok((
            Self {
                db,
                note_sources,
                calc_engine,
                note_line_cache: Mutex::new(FxHashMap::default()),
                cross_note_var_index: Arc::new(Mutex::new(CrossNoteVarIndex::default())),
            },
            AppCoreOpenMetrics {
                data_dir_ms,
                db_open_ms,
                note_sources_init_ms,
                calc_engine_init_ms,
                total_ms: total_started.elapsed().as_secs_f64() * 1000.0,
                db: db_metrics,
            },
        ))
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn cross_note_var_index_arc(&self) -> Arc<Mutex<CrossNoteVarIndex>> {
        Arc::clone(&self.cross_note_var_index)
    }

    pub fn note_sources(&self) -> &NoteSourceService {
        &self.note_sources
    }

    pub fn calc_engine(&self) -> &CalcEngine {
        &self.calc_engine
    }

    /// Return exported variable entries for a given note id.
    /// Used to populate cross-note variable autocomplete suggestions.
    pub fn cross_note_exports_for_autocomplete(
        &self,
        note_id: &str,
    ) -> Vec<calc::VariableIndexEntry> {
        self.cross_note_var_index
            .lock()
            .ok()
            .map(|index| index.exports_for_note(note_id).to_vec())
            .unwrap_or_default()
    }
}

pub fn data_dir() -> Result<PathBuf, String> {
    let project_dirs = ProjectDirs::from("io", "github", "slate")
        .ok_or_else(|| "Failed to determine app data directory".to_string())?;
    let data_dir = project_dirs.data_dir().to_path_buf();
    fs::create_dir_all(&data_dir).map_err(|e| format!("Failed to create data directory: {e}"))?;
    Ok(data_dir)
}
