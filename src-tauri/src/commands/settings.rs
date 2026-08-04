use tauri::{command, State};

use crate::{
    app_state::AppState,
    domain::error::{AppError, PublicError},
    dto::settings::{SettingsPatch, SettingsSnapshot},
};

#[command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<SettingsSnapshot, PublicError> {
    Ok(state.settings.get().await)
}

#[command]
pub async fn update_settings(
    state: State<'_, AppState>,
    patch: SettingsPatch,
) -> Result<SettingsSnapshot, PublicError> {
    let previous = state.settings.get().await;
    let snapshot = state
        .settings
        .update(patch)
        .await
        .map_err(settings_validation_error)?;
    if let Err(error) = state.database.save_settings(&snapshot).await {
        let _ = state.settings.replace(previous).await;
        return Err(PublicError::from(error));
    }
    state
        .transfers
        .set_max_concurrent_jobs(usize::from(snapshot.concurrent_jobs));
    Ok(snapshot)
}

#[command]
pub async fn reset_settings(state: State<'_, AppState>) -> Result<SettingsSnapshot, PublicError> {
    let previous = state.settings.get().await;
    let snapshot = state.settings.reset().await;
    if let Err(error) = state.database.save_settings(&snapshot).await {
        let _ = state.settings.replace(previous).await;
        return Err(PublicError::from(error));
    }
    state
        .transfers
        .set_max_concurrent_jobs(usize::from(snapshot.concurrent_jobs));
    Ok(snapshot)
}

fn settings_validation_error(
    issues: Vec<crate::dto::settings::SettingsValidationIssue>,
) -> PublicError {
    let mut error = PublicError::from(AppError::Validation(
        "One or more settings values are invalid.".to_string(),
    ));
    for issue in issues {
        error.field_errors.insert(issue.field, issue.message);
    }
    error
}
