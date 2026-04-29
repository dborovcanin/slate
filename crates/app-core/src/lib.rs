pub mod calc;
pub mod config;
pub mod note_sources;
pub mod storage;

use calc::CalcEngine;
use directories::ProjectDirs;
use note_sources::NoteSourceService;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use storage::Db;

pub struct AppCore {
    db: Db,
    note_sources: NoteSourceService,
    calc_engine: CalcEngine,
    /// Server-side line cache for the delta calc IPC protocol.
    /// Key: note_id. Value: current lines of that note as known to the server.
    pub note_line_cache: Mutex<HashMap<String, Arc<Vec<String>>>>,
}

impl AppCore {
    pub fn open_default() -> Result<Self, String> {
        let dir = data_dir()?;
        let db = Db::open(dir.join("notes.db"))?;
        let note_sources = NoteSourceService::new(db.clone());
        Ok(Self {
            db,
            note_sources,
            calc_engine: CalcEngine::new(),
            note_line_cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn note_sources(&self) -> &NoteSourceService {
        &self.note_sources
    }

    pub fn calc_engine(&self) -> &CalcEngine {
        &self.calc_engine
    }
}

pub fn data_dir() -> Result<PathBuf, String> {
    let project_dirs = ProjectDirs::from("io", "github", "slate")
        .ok_or_else(|| "Failed to determine app data directory".to_string())?;
    let data_dir = project_dirs.data_dir().to_path_buf();
    fs::create_dir_all(&data_dir).map_err(|e| format!("Failed to create data directory: {e}"))?;
    Ok(data_dir)
}
