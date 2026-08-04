use tauri::{command, State};

use crate::{
    app_state::AppState,
    domain::error::PublicError,
    dto::diagnostics::{
        DiagnosticsExportRequest, DiagnosticsExportResult, LogDirectoryResult, UpdateCheckResult,
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
    state: State<'_, AppState>,
) -> Result<UpdateCheckResult, PublicError> {
    let settings = state.settings.get().await;
    let channel = match settings.update_channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Beta => "beta",
    };
    let active_transfers = state.transfers.active_count().await;
    if active_transfers > 0 {
        return Ok(UpdateCheckResult {
            schema_version: 1,
            channel: channel.to_string(),
            available: false,
            message: format!(
                "Update checks are deferred while {active_transfers} transfer(s) are active."
            ),
        });
    }
    // Network update checks are intentionally conservative until a signed
    // manifest endpoint is configured for the selected channel.  Returning a
    // typed result keeps the UI explicit instead of following arbitrary URLs.
    Ok(UpdateCheckResult {
        schema_version: 1,
        channel: channel.to_string(),
        available: false,
        message: "No signed update manifest is configured for this build.".to_string(),
    })
}
