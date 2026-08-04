#[cfg(feature = "updater")]
use serde_json::Value;
use tauri::{command, AppHandle, State};
#[cfg(feature = "updater")]
use url::Url;

use crate::{
    app_state::AppState,
    domain::error::PublicError,
    dto::diagnostics::{
        DiagnosticsExportRequest, DiagnosticsExportResult, InstallUpdateRequest,
        InstallUpdateResult, LogDirectoryResult, UpdateCheckResult,
    },
    dto::settings::UpdateChannel,
};

#[command]
#[allow(clippy::result_large_err)]
pub fn open_log_directory(state: State<'_, AppState>) -> Result<LogDirectoryResult, PublicError> {
    Ok(state.diagnostics.log_directory())
}

#[command]
pub async fn export_diagnostics(
    state: State<'_, AppState>,
    request: DiagnosticsExportRequest,
) -> Result<DiagnosticsExportResult, PublicError> {
    let profiles = state.profiles.list_profiles().await?;
    let settings = state.settings.get().await;
    let result = state
        .diagnostics
        .export(request, settings, profiles)
        .await
        .map_err(PublicError::from)?;
    state
        .diagnostics
        .record("INFO", "diagnostics", "diagnostics export completed")
        .await;
    Ok(result)
}

#[command]
pub async fn clear_logs(state: State<'_, AppState>) -> Result<u64, PublicError> {
    let removed = state
        .diagnostics
        .clear_logs()
        .await
        .map_err(PublicError::from)?;
    state
        .diagnostics
        .record("INFO", "diagnostics", "log files cleared")
        .await;
    Ok(removed)
}

#[command]
pub async fn check_for_updates(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateCheckResult, PublicError> {
    let settings = state.settings.get().await;
    let channel = match settings.update_channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Beta => "beta",
    };
    let active_transfers = state.transfers.active_count().await;
    if active_transfers > 0 {
        return Ok(update_check_result(
            channel,
            false,
            format!("Update checks are deferred while {active_transfers} transfer(s) are active."),
            None,
            None,
            None,
        ));
    }

    #[cfg(feature = "updater")]
    {
        let endpoint = configured_endpoint(&app, channel).map_err(PublicError::from)?;
        let updater = tauri_plugin_updater::UpdaterExt::updater_builder(&app)
            .endpoints(vec![endpoint])
            .map_err(|error| PublicError::from(updater_error(error.to_string())))?
            .build()
            .map_err(|error| PublicError::from(updater_error(error.to_string())))?;
        let result = updater
            .check()
            .await
            .map_err(|error| PublicError::from(updater_error(error.to_string())))?;
        if let Some(update) = result {
            return Ok(update_check_result(
                channel,
                true,
                format!(
                    "A signed {channel} update ({}) is available.",
                    update.version
                ),
                Some(update.version),
                update.body,
                update.date.map(|date| date.to_string()),
            ));
        }
        Ok(update_check_result(
            channel,
            false,
            format!("No {channel} update is available."),
            None,
            None,
            None,
        ))
    }

    #[cfg(not(feature = "updater"))]
    {
        let _ = app;
        // Network update checks are intentionally conservative until the
        // protected release build is compiled with the updater feature and a
        // signed manifest endpoint is configured.  Returning a typed result
        // keeps the UI explicit instead of following arbitrary URLs.
        Ok(update_check_result(
            channel,
            false,
            "No signed update manifest is configured for this build.".to_string(),
            None,
            None,
            None,
        ))
    }
}

#[command]
pub async fn install_update(
    app: AppHandle,
    state: State<'_, AppState>,
    request: InstallUpdateRequest,
) -> Result<InstallUpdateResult, PublicError> {
    if request.schema_version != 1 {
        return Err(PublicError::from(
            crate::domain::error::AppError::Validation(
                "unsupported update request schema version".to_string(),
            ),
        ));
    }
    let expected_version = request.expected_version.trim();
    if expected_version.is_empty()
        || request.confirmation.trim() != format!("INSTALL UPDATE {expected_version}")
    {
        return Err(PublicError::from(
            crate::domain::error::AppError::Validation(
                "explicit update confirmation is required".to_string(),
            ),
        ));
    }
    let active_transfers = state.transfers.active_count().await;
    if active_transfers > 0 {
        return Ok(InstallUpdateResult {
            schema_version: 1,
            installed: false,
            version: expected_version.to_string(),
            message: format!(
                "Update installation is deferred while {active_transfers} transfer(s) are active. Retry after they finish."
            ),
        });
    }

    #[cfg(feature = "updater")]
    {
        let settings = state.settings.get().await;
        let channel = match settings.update_channel {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Beta => "beta",
        };
        let endpoint = configured_endpoint(&app, channel).map_err(PublicError::from)?;
        let updater = tauri_plugin_updater::UpdaterExt::updater_builder(&app)
            .endpoints(vec![endpoint])
            .map_err(|error| PublicError::from(updater_error(error.to_string())))?
            .build()
            .map_err(|error| PublicError::from(updater_error(error.to_string())))?;
        let update = updater
            .check()
            .await
            .map_err(|error| PublicError::from(updater_error(error.to_string())))?
            .ok_or_else(|| {
                PublicError::from(crate::domain::error::AppError::UpdateVerificationFailed(
                    "the signed manifest no longer advertises the requested version".to_string(),
                ))
            })?;
        if update.version != expected_version {
            return Err(PublicError::from(
                crate::domain::error::AppError::UpdateVerificationFailed(
                    "the signed manifest changed after confirmation".to_string(),
                ),
            ));
        }
        // Download first, then re-check the transfer gate immediately before
        // install.  This prevents an update from replacing the app while a
        // transfer started during the potentially long download.
        let bytes = update
            .download(|_, _| {}, || {})
            .await
            .map_err(|error| PublicError::from(updater_error(error.to_string())))?;
        let active_after_download = state.transfers.active_count().await;
        if active_after_download > 0 {
            return Ok(InstallUpdateResult {
                schema_version: 1,
                installed: false,
                version: expected_version.to_string(),
                message: format!(
                    "Update downloaded but installation is deferred while {active_after_download} transfer(s) are active. Retry after they finish."
                ),
            });
        }
        update
            .install(bytes)
            .map_err(|error| PublicError::from(updater_error(error.to_string())))?;
        Ok(InstallUpdateResult {
            schema_version: 1,
            installed: true,
            version: expected_version.to_string(),
            message: "Update verified and handed to the installer; restart when prompted."
                .to_string(),
        })
    }

    #[cfg(not(feature = "updater"))]
    {
        let _ = app;
        let _ = state;
        Err(PublicError::from(
            crate::domain::error::AppError::Validation(
                "Signed updater is not included in this development build.".to_string(),
            ),
        ))
    }
}

fn update_check_result(
    channel: &str,
    available: bool,
    message: String,
    version: Option<String>,
    notes: Option<String>,
    published_at: Option<String>,
) -> UpdateCheckResult {
    UpdateCheckResult {
        schema_version: 1,
        channel: channel.to_string(),
        available,
        message,
        version,
        notes,
        published_at,
        install_requires_confirmation: available,
        can_install: available,
    }
}

#[cfg(feature = "updater")]
fn configured_endpoint(
    app: &AppHandle,
    channel: &str,
) -> Result<Url, crate::domain::error::AppError> {
    let index = channel_index(channel).ok_or_else(|| {
        crate::domain::error::AppError::Validation("unsupported update channel".to_string())
    })?;
    let value = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|plugin| plugin.get("endpoints"))
        .and_then(Value::as_array)
        .and_then(|endpoints| endpoints.get(index))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            crate::domain::error::AppError::Validation(
                "no signed update manifest is configured for this build".to_string(),
            )
        })?;
    let endpoint =
        Url::parse(value).map_err(|_| crate::domain::error::AppError::InvalidEndpoint)?;
    if endpoint.scheme() != "https"
        || endpoint.host_str().is_none()
        || endpoint.username() != ""
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(crate::domain::error::AppError::InsecureEndpointBlocked);
    }
    Ok(endpoint)
}

#[cfg(feature = "updater")]
fn channel_index(channel: &str) -> Option<usize> {
    match channel {
        "stable" => Some(0),
        "beta" => Some(1),
        _ => None,
    }
}

#[cfg(feature = "updater")]
fn updater_error(message: String) -> crate::domain::error::AppError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("minisign")
        || lower.contains("signature")
        || lower.contains("base64")
        || lower.contains("verify")
    {
        crate::domain::error::AppError::UpdateVerificationFailed(
            "the signed update could not be verified".to_string(),
        )
    } else if lower.contains("reqwest")
        || lower.contains("network")
        || lower.contains("fetch")
        || lower.contains("timeout")
    {
        crate::domain::error::AppError::NetworkUnavailable
    } else {
        crate::domain::error::AppError::Unknown("signed update check failed".to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn confirmation_phrase_is_version_bound() {
        let version = "1.2.3";
        assert_eq!(format!("INSTALL UPDATE {version}"), "INSTALL UPDATE 1.2.3");
        assert_ne!("INSTALL UPDATE 1.2.4", format!("INSTALL UPDATE {version}"));
    }

    #[cfg(feature = "updater")]
    #[test]
    fn updater_error_is_sanitized() {
        let error = super::updater_error("minisign signature mismatch: secret".to_string());
        assert!(matches!(
            error,
            crate::domain::error::AppError::UpdateVerificationFailed(_)
        ));
        assert!(!error.to_string().contains("secret"));
    }

    #[cfg(feature = "updater")]
    #[test]
    fn endpoint_channel_indices_are_closed() {
        assert_eq!(super::channel_index("stable"), Some(0));
        assert_eq!(super::channel_index("beta"), Some(1));
        // The channel parser is intentionally closed; unknown values never
        // become arbitrary endpoints.
        assert_eq!(super::channel_index("nightly"), None);
    }
}
