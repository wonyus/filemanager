use std::{
    collections::VecDeque,
    fmt,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::Utc;
use tokio::{
    fs,
    sync::{Mutex, RwLock},
};
use tracing::{Event, Subscriber};
use tracing_subscriber::{
    layer::{Context, Layer},
    registry::LookupSpan,
};
use uuid::Uuid;

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
const DEFAULT_LOG_RETENTION_DAYS: u16 = 14;
const DEFAULT_LOG_MAX_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Clone)]
pub struct DiagnosticsService {
    data_dir: Arc<PathBuf>,
    log_file: Arc<PathBuf>,
    entries: Arc<RwLock<VecDeque<DiagnosticLogEntry>>>,
    export_lock: Arc<Mutex<()>>,
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
            export_lock: Arc::new(Mutex::new(())),
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
        if fs::metadata(self.log_file.as_ref())
            .await
            .map(|metadata| metadata.len() > DEFAULT_LOG_MAX_BYTES)
            .unwrap_or(false)
        {
            let _ = self
                .prune_log_file(DEFAULT_LOG_RETENTION_DAYS, DEFAULT_LOG_MAX_BYTES)
                .await;
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
        let _export_guard = self.export_lock.lock().await;
        if request.schema_version != DTO_SCHEMA_VERSION {
            return Err(AppError::Validation(
                "unsupported diagnostics schema version".to_string(),
            ));
        }
        let destination = validate_destination(&request.destination_path)?;
        self.prune_log_file(settings.log_retention_days, settings.log_max_bytes)
            .await?;
        let snapshot = DiagnosticsSnapshot {
            schema_version: DTO_SCHEMA_VERSION,
            app_version: option_env!("S3FM_RELEASE_VERSION")
                .unwrap_or(env!("CARGO_PKG_VERSION"))
                .to_string(),
            platform: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
            database_schema_version: 6,
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
        // Diagnostics are an archive by contract.  Keep the payload as a
        // single redacted JSON member so support tooling can inspect it
        // without ever receiving raw logs, secrets, or presigned URLs.
        let temporary = destination.with_extension(format!("tmp-{}", Uuid::new_v4()));
        let temporary_for_zip = temporary.clone();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let file = std::fs::File::create(&temporary_for_zip).map_err(AppError::Io)?;
            let mut archive = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            archive
                .start_file("diagnostics.json", options)
                .map_err(|error| {
                    AppError::Unknown(format!("diagnostics archive failed: {error}"))
                })?;
            archive.write_all(&bytes).map_err(AppError::Io)?;
            archive.finish().map_err(|error| {
                AppError::Unknown(format!("diagnostics archive failed: {error}"))
            })?;
            Ok(())
        })
        .await
        .map_err(|error| {
            AppError::Unknown(format!("diagnostics archive worker failed: {error}"))
        })??;
        fs::rename(&temporary, &destination).await?;
        let bytes_written = fs::metadata(&destination).await?.len();
        Ok(DiagnosticsExportResult {
            schema_version: DTO_SCHEMA_VERSION,
            path: destination.to_string_lossy().into_owned(),
            bytes_written,
            redacted: true,
        })
    }

    async fn prune_log_file(&self, retention_days: u16, max_bytes: u64) -> Result<(), AppError> {
        let Ok(bytes) = fs::read(self.log_file.as_ref()).await else {
            return Ok(());
        };
        if bytes.is_empty() {
            return Ok(());
        }
        let cutoff = Utc::now() - chrono::Duration::days(i64::from(retention_days));
        let mut kept = VecDeque::new();
        for line in bytes.split(|byte| *byte == b'\n') {
            if line.is_empty() {
                continue;
            }
            let keep = serde_json::from_slice::<DiagnosticLogEntry>(line)
                .ok()
                .and_then(|entry| chrono::DateTime::parse_from_rfc3339(&entry.timestamp).ok())
                .is_some_and(|timestamp| timestamp.with_timezone(&chrono::Utc) >= cutoff);
            if keep {
                kept.push_back(line.to_vec());
            }
        }
        let mut selected = Vec::new();
        let mut selected_bytes = 0usize;
        for line in kept.into_iter().rev() {
            if selected_bytes.saturating_add(line.len() + 1) > max_bytes as usize {
                break;
            }
            selected_bytes = selected_bytes.saturating_add(line.len() + 1);
            selected.push(line);
        }
        selected.reverse();
        let mut output = Vec::with_capacity(selected_bytes);
        for line in selected {
            output.extend_from_slice(&line);
            output.push(b'\n');
        }
        if output.len() < bytes.len() {
            let temporary = self
                .log_file
                .with_extension(format!("tmp-{}", Uuid::new_v4()));
            fs::write(&temporary, output).await?;
            fs::rename(temporary, self.log_file.as_ref()).await?;
        }
        Ok(())
    }
}

/// Bridge application tracing calls into the redacted diagnostics store. The
/// layer schedules asynchronous writes and never blocks provider work on I/O.
#[derive(Clone)]
pub struct DiagnosticsLayer {
    service: Arc<DiagnosticsService>,
}

impl DiagnosticsLayer {
    pub fn new(service: Arc<DiagnosticsService>) -> Self {
        Self { service }
    }
}

impl<S> Layer<S> for DiagnosticsLayer
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);
        let message = if visitor.fields.is_empty() {
            visitor.message
        } else {
            format!("{} {}", visitor.message, visitor.fields.join(" "))
        };
        let service = self.service.clone();
        let level = event.metadata().level().to_string();
        let component = event.metadata().target().to_string();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                service.record(&level, &component, &message).await;
            });
        }
    }
}

#[derive(Default)]
struct EventVisitor {
    message: String,
    fields: Vec<String>,
}

impl tracing::field::Visit for EventVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        let rendered = format!("{}={value:?}", field.name());
        if field.name() == "message" {
            self.message = format!("{value:?}").trim_matches('"').to_string();
        } else {
            self.fields.push(rendered);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push(format!("{}={value}", field.name()));
        }
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
    let mut destination = path.to_path_buf();
    if destination
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("zip")
    {
        destination.set_extension("zip");
    }
    Ok(destination)
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
