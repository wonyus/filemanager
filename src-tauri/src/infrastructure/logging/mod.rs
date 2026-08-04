use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::Utc;
use tokio::{fs, sync::RwLock};

use crate::{
    domain::error::AppError,
    dto::{
        diagnostics::{
            DiagnosticLogEntry, DiagnosticsExportRequest, DiagnosticsExportResult,
            DiagnosticsSnapshot, LogDirectoryResult, RedactedProfile, DTO_SCHEMA_VERSION,
        },
        profile::ProfileSummary,
        settings::SettingsSnapshot,
    },
};

const MAX_MEMORY_ENTRIES: usize = 200;

#[derive(Clone)]
pub struct DiagnosticsService {
    data_dir: Arc<PathBuf>,
    log_file: Arc<PathBuf>,
    entries: Arc<RwLock<VecDeque<DiagnosticLogEntry>>>,
}

impl DiagnosticsService {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        let log_dir = data_dir.join("logs");
        let log_file = log_dir.join("s3-file-manager.log");
        let _ = std::fs::create_dir_all(&log_dir);
        Self {
            data_dir: Arc::new(data_dir),
            log_file: Arc::new(log_file),
            entries: Arc::new(RwLock::new(VecDeque::with_capacity(MAX_MEMORY_ENTRIES))),
        }
    }

    pub fn default_for_tests() -> Self {
        Self::new(std::env::temp_dir().join("s3-file-manager"))
    }

    pub async fn record(&self, level: &str, component: &str, message: &str) {
        let entry = DiagnosticLogEntry {
            timestamp: Utc::now().to_rfc3339(),
            level: sanitize_token(level),
            component: sanitize_token(component),
            message: redact_message(message),
        };
        {
            let mut entries = self.entries.write().await;
            entries.push_back(entry.clone());
            while entries.len() > MAX_MEMORY_ENTRIES {
                entries.pop_front();
            }
        }
        if let Some(parent) = self.log_file.parent() {
            let _ = fs::create_dir_all(parent).await;
        }
        let line = match serde_json::to_string(&entry) {
            Ok(line) => line,
            Err(_) => return,
        };
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_file.as_ref())
            .await
        {
            use tokio::io::AsyncWriteExt;
            let _ = file.write_all(format!("{line}\n").as_bytes()).await;
        }
    }

    pub async fn recent_entries(&self) -> Vec<DiagnosticLogEntry> {
        self.entries.read().await.iter().cloned().collect()
    }

    pub fn log_directory(&self) -> LogDirectoryResult {
        LogDirectoryResult {
            schema_version: DTO_SCHEMA_VERSION,
            path: self
                .log_file
                .parent()
                .unwrap_or(self.data_dir.as_path())
                .to_string_lossy()
                .into_owned(),
        }
    }

    pub async fn clear_logs(&self) -> Result<u64, AppError> {
        let mut removed = 0;
        if fs::try_exists(self.log_file.as_ref()).await? {
            let size = fs::metadata(self.log_file.as_ref()).await?.len();
            fs::remove_file(self.log_file.as_ref()).await?;
            removed = size;
        }
        self.entries.write().await.clear();
        Ok(removed)
    }

    pub async fn export(
        &self,
        request: DiagnosticsExportRequest,
        settings: SettingsSnapshot,
        profiles: Vec<ProfileSummary>,
    ) -> Result<DiagnosticsExportResult, AppError> {
        if request.schema_version != DTO_SCHEMA_VERSION {
            return Err(AppError::Validation(
                "unsupported diagnostics schema version".to_string(),
            ));
        }
        let destination = validate_destination(&request.destination_path)?;
        let snapshot = DiagnosticsSnapshot {
            schema_version: DTO_SCHEMA_VERSION,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            platform: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
            database_schema_version: 3,
            providers: vec![
                "awsS3".to_string(),
                "cloudflareR2".to_string(),
                "minio".to_string(),
                "wasabi".to_string(),
                "customS3".to_string(),
            ],
            settings: settings.normalized(),
            profiles: profiles.into_iter().map(RedactedProfile::from).collect(),
            recent_logs: self.recent_entries().await,
        };
        let bytes = serde_json::to_vec_pretty(&snapshot)
            .map_err(|error| AppError::Unknown(format!("diagnostics encoding failed: {error}")))?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        let temporary = destination.with_extension("tmp");
        fs::write(&temporary, &bytes).await?;
        fs::rename(&temporary, &destination).await?;
        Ok(DiagnosticsExportResult {
            schema_version: DTO_SCHEMA_VERSION,
            path: destination.to_string_lossy().into_owned(),
            bytes_written: bytes.len() as u64,
            redacted: true,
        })
    }
}

fn validate_destination(value: &str) -> Result<PathBuf, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains('\0') || trimmed.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "diagnostics destination path is invalid".to_string(),
        ));
    }
    let path = Path::new(trimmed);
    if path.file_name().is_none() {
        return Err(AppError::Validation(
            "diagnostics destination must be a file".to_string(),
        ));
    }
    Ok(path.to_path_buf())
}

fn sanitize_token(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        .take(32)
        .collect()
}

fn redact_message(value: &str) -> String {
    let mut message = value.replace(['\r', '\n'], " ");
    for marker in ["X-Amz-Signature=", "X-Amz-Credential=", "Authorization:"] {
        if let Some(index) = message.find(marker) {
            message.truncate(index);
            message.push_str("[redacted]");
        }
    }
    message.chars().take(1_024).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_redacted_bounded_entries() {
        let dir = std::env::temp_dir().join(format!("s3fm-diagnostics-{}", uuid::Uuid::new_v4()));
        let service = DiagnosticsService::new(&dir);
        service
            .record(
                "INFO",
                "test",
                "https://example.test?a=X-Amz-Signature=secret",
            )
            .await;
        let entries = service.recent_entries().await;
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].message.contains("secret"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
