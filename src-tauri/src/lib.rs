pub mod app_state;
pub mod application;
pub mod commands;
pub mod domain;
pub mod dto;
pub mod infrastructure;
pub mod transfer;

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
            let settings = tauri::async_runtime::block_on(database.load_settings())
                .map_err(|error| std::io::Error::other(error.to_string()))?
                .unwrap_or_default();
            app.manage(AppState::new_with_settings_and_data_dir(
                database,
                settings,
                app_data_dir,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::get_app_info,
            commands::profiles::list_profiles,
            commands::profiles::get_profile,
            commands::profiles::create_profile,
            commands::profiles::update_profile,
            commands::profiles::duplicate_profile,
            commands::profiles::delete_profile,
            commands::profiles::test_profile,
            commands::explorer::list_buckets,
            commands::explorer::list_entries,
            commands::metadata::head_object,
            commands::metadata::preview_object,
            commands::metadata::create_share_link,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::reset_settings,
            commands::diagnostics::open_log_directory,
            commands::diagnostics::export_diagnostics,
            commands::diagnostics::clear_logs,
            commands::diagnostics::check_for_updates,
            commands::transfers::start_transfer,
            commands::transfers::list_transfers,
            commands::transfers::get_transfer_details,
            commands::transfers::pause_transfer,
            commands::transfers::resume_transfer,
            commands::transfers::cancel_transfer,
            commands::transfers::retry_transfer,
            commands::transfers::clear_transfer_history
        ])
        .run(tauri::generate_context!())
        .expect("error while running S3 File Manager");
}
