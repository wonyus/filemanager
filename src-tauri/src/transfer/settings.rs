use std::sync::Arc;

use tokio::sync::RwLock;

use crate::dto::settings::{SettingsPatch, SettingsSnapshot, SettingsValidationIssue};

/// In-memory settings service used by the transfer subsystem.  Persistence is
/// intentionally kept behind this small interface so the database adapter can
/// load/save JSON without allowing secrets or provider credentials into the
/// settings row.  The service snapshots settings for each new job, so active
/// transfers are unaffected by later edits.
#[derive(Clone)]
pub struct SettingsService {
    current: Arc<RwLock<SettingsSnapshot>>,
}

impl Default for SettingsService {
    fn default() -> Self {
        Self::new(SettingsSnapshot::default())
    }
}

impl SettingsService {
    pub fn new(snapshot: SettingsSnapshot) -> Self {
        let snapshot = snapshot.normalized();
        let validated = if snapshot.validate().is_ok() {
            snapshot
        } else {
            SettingsSnapshot::default()
        };
        Self {
            current: Arc::new(RwLock::new(validated)),
        }
    }

    pub async fn get(&self) -> SettingsSnapshot {
        self.current.read().await.clone().normalized()
    }

    pub async fn update(
        &self,
        patch: SettingsPatch,
    ) -> Result<SettingsSnapshot, Vec<SettingsValidationIssue>> {
        let existing = self.current.read().await.clone();
        let next = existing.apply_patch(patch)?;
        *self.current.write().await = next.clone();
        Ok(next)
    }

    pub async fn replace(
        &self,
        snapshot: SettingsSnapshot,
    ) -> Result<SettingsSnapshot, Vec<SettingsValidationIssue>> {
        let normalized = snapshot.normalized();
        normalized.validate()?;
        *self.current.write().await = normalized.clone();
        Ok(normalized)
    }

    pub async fn reset(&self) -> SettingsSnapshot {
        let defaults = SettingsSnapshot::default();
        *self.current.write().await = defaults.clone();
        defaults
    }

    /// Serialize only the redacted settings snapshot for the database row or
    /// diagnostics archive.  This method never contains credentials or URLs.
    pub async fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.get().await)
    }

    pub async fn from_json(&self, value: &str) -> Result<SettingsSnapshot, SettingsLoadError> {
        let snapshot: SettingsSnapshot =
            serde_json::from_str(value).map_err(SettingsLoadError::InvalidJson)?;
        self.replace(snapshot)
            .await
            .map_err(SettingsLoadError::InvalidValues)
    }
}

#[derive(Debug)]
pub enum SettingsLoadError {
    InvalidJson(serde_json::Error),
    InvalidValues(Vec<SettingsValidationIssue>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn updates_are_validated_and_snapshotted() {
        let service = SettingsService::default();
        let next = service
            .update(SettingsPatch {
                concurrent_jobs: Some(8),
                ..SettingsPatch::default()
            })
            .await
            .unwrap();
        assert_eq!(next.concurrent_jobs, 8);
        assert_eq!(service.get().await.concurrent_jobs, 8);
        assert!(service
            .update(SettingsPatch {
                progress_hz: Some(0),
                ..SettingsPatch::default()
            })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn json_round_trip_preserves_redacted_snapshot() {
        let service = SettingsService::default();
        let json = service.to_json().await.unwrap();
        assert!(!json.contains("secret"));
        let loaded = service.from_json(&json).await.unwrap();
        assert_eq!(loaded, SettingsSnapshot::default());
    }
}
