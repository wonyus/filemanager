use serde::{Deserialize, Serialize};

use super::transfer::CollisionPolicy;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
const PIB: u64 = 1024 * GIB * 1024;

/// The update channel is deliberately a closed enum.  The updater must never
/// accept an arbitrary URL or channel name from the renderer.
#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

/// Redacted settings returned to the frontend and copied into every newly
/// created transfer job.  The legacy aliases are retained for compatibility
/// with the initial shell; they are kept in sync by `normalized`.
#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub schema_version: u16,
    pub concurrent_jobs: u8,
    pub per_job_part_concurrency: u8,
    pub per_profile_request_limit: u8,
    pub multipart_threshold_bytes: u64,
    pub initial_part_size_bytes: u64,
    pub retry_limit: u8,
    pub retry_base_delay_ms: u64,
    pub retry_max_delay_ms: u64,
    pub progress_hz: u8,
    pub default_collision_policy: CollisionPolicy,
    pub preserve_empty_folders: bool,
    pub keep_partial_downloads: bool,
    pub preview_cache_bytes: u64,
    pub preview_cache_max_age_hours: u16,
    pub transfer_history_days: u16,
    pub transfer_history_max_jobs: u32,
    pub log_retention_days: u16,
    pub log_max_bytes: u64,
    pub typed_confirm_object_threshold: u64,
    pub typed_confirm_bytes_threshold: u64,
    pub update_channel: UpdateChannel,
    pub automatic_update_check: bool,

    // Compatibility aliases used by the Phase 0 frontend.
    pub transfer_concurrency: u32,
    pub part_concurrency: u32,
    pub preview_cache_quota_bytes: u64,
}

impl Default for SettingsSnapshot {
    fn default() -> Self {
        Self {
            schema_version: 1,
            concurrent_jobs: 4,
            per_job_part_concurrency: 4,
            per_profile_request_limit: 8,
            multipart_threshold_bytes: 64 * MIB,
            initial_part_size_bytes: 16 * MIB,
            retry_limit: 5,
            retry_base_delay_ms: 500,
            retry_max_delay_ms: 30_000,
            progress_hz: 5,
            default_collision_policy: CollisionPolicy::Ask,
            preserve_empty_folders: true,
            keep_partial_downloads: false,
            preview_cache_bytes: 512 * MIB,
            preview_cache_max_age_hours: 24,
            transfer_history_days: 30,
            transfer_history_max_jobs: 1_000,
            log_retention_days: 14,
            log_max_bytes: 100 * MIB,
            typed_confirm_object_threshold: 100,
            typed_confirm_bytes_threshold: 10 * GIB,
            update_channel: UpdateChannel::Stable,
            automatic_update_check: true,
            transfer_concurrency: 4,
            part_concurrency: 4,
            preview_cache_quota_bytes: 512 * MIB,
        }
    }
}

impl SettingsSnapshot {
    /// Keep compatibility aliases synchronized before serialization or job
    /// snapshotting.  This is intentionally cheap and deterministic.
    pub fn normalized(mut self) -> Self {
        self.transfer_concurrency = u32::from(self.concurrent_jobs);
        self.part_concurrency = u32::from(self.per_job_part_concurrency);
        self.preview_cache_quota_bytes = self.preview_cache_bytes;
        self
    }

    /// Validate all documented ranges and cross-field constraints.  Returning
    /// field-level issues lets the settings command highlight the offending
    /// controls without exposing internal errors.
    pub fn validate(&self) -> Result<(), Vec<SettingsValidationIssue>> {
        let mut issues = Vec::new();
        check_range(&mut issues, "concurrentJobs", self.concurrent_jobs, 1, 16);
        check_range(
            &mut issues,
            "perJobPartConcurrency",
            self.per_job_part_concurrency,
            1,
            16,
        );
        check_range(
            &mut issues,
            "perProfileRequestLimit",
            self.per_profile_request_limit,
            1,
            32,
        );
        check_range(
            &mut issues,
            "multipartThresholdBytes",
            self.multipart_threshold_bytes,
            16 * MIB,
            5 * GIB,
        );
        check_range(
            &mut issues,
            "initialPartSizeBytes",
            self.initial_part_size_bytes,
            5 * MIB,
            5 * GIB,
        );
        check_range(&mut issues, "retryLimit", self.retry_limit, 0, 10);
        check_range(
            &mut issues,
            "retryBaseDelayMs",
            self.retry_base_delay_ms,
            100,
            5_000,
        );
        check_range(
            &mut issues,
            "retryMaxDelayMs",
            self.retry_max_delay_ms,
            1_000,
            120_000,
        );
        check_range(&mut issues, "progressHz", self.progress_hz, 1, 10);
        check_range(
            &mut issues,
            "previewCacheBytes",
            self.preview_cache_bytes,
            64 * MIB,
            10 * GIB,
        );
        check_range(
            &mut issues,
            "previewCacheMaxAgeHours",
            self.preview_cache_max_age_hours,
            1,
            168,
        );
        check_range(
            &mut issues,
            "transferHistoryDays",
            self.transfer_history_days,
            1,
            365,
        );
        check_range(
            &mut issues,
            "transferHistoryMaxJobs",
            self.transfer_history_max_jobs,
            100,
            100_000,
        );
        check_range(
            &mut issues,
            "logRetentionDays",
            self.log_retention_days,
            1,
            90,
        );
        check_range(
            &mut issues,
            "logMaxBytes",
            self.log_max_bytes,
            10 * MIB,
            2 * GIB,
        );
        check_range(
            &mut issues,
            "typedConfirmObjectThreshold",
            self.typed_confirm_object_threshold,
            1,
            1_000_000,
        );
        check_range(
            &mut issues,
            "typedConfirmBytesThreshold",
            self.typed_confirm_bytes_threshold,
            MIB,
            PIB,
        );
        if self.retry_max_delay_ms < self.retry_base_delay_ms {
            issues.push(SettingsValidationIssue {
                field: "retryMaxDelayMs".to_string(),
                message: "must be greater than or equal to retryBaseDelayMs".to_string(),
            });
        }
        if self.initial_part_size_bytes > self.multipart_threshold_bytes {
            issues.push(SettingsValidationIssue {
                field: "initialPartSizeBytes".to_string(),
                message: "should not exceed multipartThresholdBytes".to_string(),
            });
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }

    pub fn apply_patch(&self, patch: SettingsPatch) -> Result<Self, Vec<SettingsValidationIssue>> {
        if patch.schema_version != 1 {
            return Err(vec![SettingsValidationIssue {
                field: "schemaVersion".to_string(),
                message: format!(
                    "unsupported settings schema version: {}",
                    patch.schema_version
                ),
            }]);
        }
        let mut next = self.clone();
        macro_rules! set {
            ($name:ident) => {
                if let Some(value) = patch.$name {
                    next.$name = value;
                }
            };
        }
        set!(concurrent_jobs);
        set!(per_job_part_concurrency);
        set!(per_profile_request_limit);
        set!(multipart_threshold_bytes);
        set!(initial_part_size_bytes);
        set!(retry_limit);
        set!(retry_base_delay_ms);
        set!(retry_max_delay_ms);
        set!(progress_hz);
        set!(default_collision_policy);
        set!(preserve_empty_folders);
        set!(keep_partial_downloads);
        set!(preview_cache_bytes);
        set!(preview_cache_max_age_hours);
        set!(transfer_history_days);
        set!(transfer_history_max_jobs);
        set!(log_retention_days);
        set!(log_max_bytes);
        set!(typed_confirm_object_threshold);
        set!(typed_confirm_bytes_threshold);
        set!(update_channel);
        set!(automatic_update_check);

        // Accept Phase 0 field names when no canonical field was supplied.
        if patch.concurrent_jobs.is_none() {
            if let Some(value) = patch.transfer_concurrency {
                next.concurrent_jobs = value.min(u32::from(u8::MAX)) as u8;
            }
        }
        if patch.per_job_part_concurrency.is_none() {
            if let Some(value) = patch.part_concurrency {
                next.per_job_part_concurrency = value.min(u32::from(u8::MAX)) as u8;
            }
        }
        if patch.preview_cache_bytes.is_none() {
            if let Some(value) = patch.preview_cache_quota_bytes {
                next.preview_cache_bytes = value;
            }
        }

        next = next.normalized();
        next.validate().map(|()| next)
    }
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub concurrent_jobs: Option<u8>,
    pub per_job_part_concurrency: Option<u8>,
    pub per_profile_request_limit: Option<u8>,
    pub multipart_threshold_bytes: Option<u64>,
    pub initial_part_size_bytes: Option<u64>,
    pub retry_limit: Option<u8>,
    pub retry_base_delay_ms: Option<u64>,
    pub retry_max_delay_ms: Option<u64>,
    pub progress_hz: Option<u8>,
    pub default_collision_policy: Option<CollisionPolicy>,
    pub preserve_empty_folders: Option<bool>,
    pub keep_partial_downloads: Option<bool>,
    pub preview_cache_bytes: Option<u64>,
    pub preview_cache_max_age_hours: Option<u16>,
    pub transfer_history_days: Option<u16>,
    pub transfer_history_max_jobs: Option<u32>,
    pub log_retention_days: Option<u16>,
    pub log_max_bytes: Option<u64>,
    pub typed_confirm_object_threshold: Option<u64>,
    pub typed_confirm_bytes_threshold: Option<u64>,
    pub update_channel: Option<UpdateChannel>,
    pub automatic_update_check: Option<bool>,

    pub transfer_concurrency: Option<u32>,
    pub part_concurrency: Option<u32>,
    pub preview_cache_quota_bytes: Option<u64>,
}

impl Default for SettingsPatch {
    fn default() -> Self {
        Self {
            schema_version: 1,
            concurrent_jobs: None,
            per_job_part_concurrency: None,
            per_profile_request_limit: None,
            multipart_threshold_bytes: None,
            initial_part_size_bytes: None,
            retry_limit: None,
            retry_base_delay_ms: None,
            retry_max_delay_ms: None,
            progress_hz: None,
            default_collision_policy: None,
            preserve_empty_folders: None,
            keep_partial_downloads: None,
            preview_cache_bytes: None,
            preview_cache_max_age_hours: None,
            transfer_history_days: None,
            transfer_history_max_jobs: None,
            log_retention_days: None,
            log_max_bytes: None,
            typed_confirm_object_threshold: None,
            typed_confirm_bytes_threshold: None,
            update_channel: None,
            automatic_update_check: None,
            transfer_concurrency: None,
            part_concurrency: None,
            preview_cache_quota_bytes: None,
        }
    }
}

fn default_schema_version() -> u16 {
    1
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsValidationIssue {
    pub field: String,
    pub message: String,
}

fn check_range<T>(issues: &mut Vec<SettingsValidationIssue>, field: &str, value: T, min: T, max: T)
where
    T: Ord + std::fmt::Display,
{
    if value < min || value > max {
        issues.push(SettingsValidationIssue {
            field: field.to_string(),
            message: format!("must be between {min} and {max}"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_normalized() {
        let defaults = SettingsSnapshot::default();
        assert!(defaults.validate().is_ok());
        let normalized = defaults.clone().normalized();
        assert_eq!(normalized.transfer_concurrency, 4);
        assert_eq!(
            normalized.preview_cache_quota_bytes,
            defaults.preview_cache_bytes
        );
    }

    #[test]
    fn invalid_patch_reports_field_level_errors() {
        let result = SettingsSnapshot::default().apply_patch(SettingsPatch {
            concurrent_jobs: Some(0),
            retry_max_delay_ms: Some(100),
            ..SettingsPatch::default()
        });
        let issues = result.expect_err("invalid values must be rejected");
        assert!(issues.iter().any(|issue| issue.field == "concurrentJobs"));
        assert!(issues.iter().any(|issue| issue.field == "retryMaxDelayMs"));
    }

    #[test]
    fn unsupported_patch_schema_is_rejected() {
        let result = SettingsSnapshot::default().apply_patch(SettingsPatch {
            schema_version: 99,
            ..SettingsPatch::default()
        });
        let issues = result.expect_err("unknown request schemas must be rejected");
        assert_eq!(issues[0].field, "schemaVersion");
    }

    #[test]
    fn legacy_patch_names_are_supported() {
        let next = SettingsSnapshot::default()
            .apply_patch(SettingsPatch {
                transfer_concurrency: Some(8),
                ..SettingsPatch::default()
            })
            .expect("legacy aliases remain supported");
        assert_eq!(next.concurrent_jobs, 8);
        assert_eq!(next.transfer_concurrency, 8);
    }
}
