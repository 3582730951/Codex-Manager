use crate::{
    app_shell::set_unsaved_settings_draft_sections,
    commands::shared::{open_in_browser_blocking, open_in_file_manager_blocking},
};

#[tauri::command]
pub async fn open_in_browser(url: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open_in_browser_blocking(&url))
        .await
        .map_err(|err| format!("open_in_browser task failed: {err}"))?
}

#[tauri::command]
pub async fn open_in_file_manager(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open_in_file_manager_blocking(&path))
        .await
        .map_err(|err| format!("open_in_file_manager task failed: {err}"))?
}

#[tauri::command]
pub fn app_window_unsaved_draft_sections_set(sections: Vec<String>) -> Result<(), String> {
    set_unsaved_settings_draft_sections(sections);
    Ok(())
}
