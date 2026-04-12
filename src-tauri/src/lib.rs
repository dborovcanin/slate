mod calc;
mod commands;
mod storage;

use calc::engine::CalcEngine;
use directories::ProjectDirs;
use storage::Db;
use std::fs;

pub fn run() {
    let project_dirs = ProjectDirs::from("io", "github", "note")
        .expect("Failed to determine app data directory");
    let data_dir = project_dirs.data_dir();
    fs::create_dir_all(data_dir).expect("Failed to create data directory");

    let db_path = data_dir.join("notes.db");
    let db = Db::open(db_path).expect("Failed to open database");
    let calc_engine = CalcEngine::new();

    tauri::Builder::default()
        .manage(db)
        .manage(calc_engine)
        .invoke_handler(tauri::generate_handler![
            commands::notes::get_or_create_note,
            commands::notes::save_note,
            commands::notes::create_note,
            commands::notes::list_notes,
            commands::notes::delete_note,
            commands::calc::evaluate_lines,
        ])
        .run(tauri::generate_context!())
        .expect("Error while running tauri application");
}
