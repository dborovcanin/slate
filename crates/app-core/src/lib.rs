pub mod calc;
pub mod config;
pub mod storage;

use calc::CalcEngine;
use directories::ProjectDirs;
use std::fs;
use std::path::PathBuf;
use storage::Db;

pub struct AppCore {
    db: Db,
    calc_engine: CalcEngine,
}

impl AppCore {
    pub fn open_default() -> Result<Self, String> {
        let dir = data_dir()?;
        let db = Db::open(dir.join("notes.db"))?;
        Ok(Self {
            db,
            calc_engine: CalcEngine::new(),
        })
    }

    pub fn db(&self) -> &Db {
        &self.db
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
