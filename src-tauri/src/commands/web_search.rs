#[cfg(feature = "gui")]
#[tauri::command]
pub async fn search_web(query: String) -> Result<app_core::web_search::WebSearchResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let config = app_core::config::load_web_search_config();
        app_core::web_search::search_web(&query, &config)
    })
    .await
    .map_err(|err| format!("Web search worker failed: {err}"))?
}
