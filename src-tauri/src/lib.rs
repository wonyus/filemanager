pub mod app_state;
pub mod commands;
pub mod domain;
pub mod dto;
pub mod infrastructure;

use std::fs;

use app_state::AppState;
use infrastructure::database::Database;
use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            fs::create_dir_all(&app_data_dir)?;
            let database_path = app_data_dir.join("s3-file-manager.sqlite3");
            let database = tauri::async_runtime::block_on(Database::connect(&database_path))
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            app.manage(AppState::new(database));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::get_app_info,
            commands::profiles::list_profiles,
            commands::settings::get_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running S3 File Manager");
}
