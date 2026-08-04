use tauri::command;

use crate::dto::app::AppInfo;

#[command]
pub fn get_app_info() -> AppInfo {
    AppInfo {
        product_name: "S3 File Manager".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: 1,
        phase: "foundation".to_string(),
    }
}
