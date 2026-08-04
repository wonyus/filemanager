use tauri::command;

use crate::dto::settings::SettingsSnapshot;

#[command]
pub fn get_settings() -> SettingsSnapshot {
    SettingsSnapshot::default()
}
