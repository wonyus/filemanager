use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub product_name: String,
    pub version: String,
    pub schema_version: i32,
    pub phase: String,
}
