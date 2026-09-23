use tauri::command;

use crate::dto::app::AppInfo;

#[command]
pub fn get_app_info() -> AppInfo {
    AppInfo {
        product_name: "S3 File Manager".to_string(),
        version: option_env!("S3FM_RELEASE_VERSION")
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .to_string(),
        schema_version: 1,
        phase: "packaging".to_string(),
    }
}
