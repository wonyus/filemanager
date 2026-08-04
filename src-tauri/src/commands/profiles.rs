use tauri::{command, State};

use crate::{
    app_state::AppState,
    domain::error::PublicError,
    dto::profile::{ConnectionTestResult, ProfileDetail, ProfileDraft, ProfileSummary},
};

#[command]
pub async fn list_profiles(state: State<'_, AppState>) -> Result<Vec<ProfileSummary>, PublicError> {
    state.profiles.list_profiles().await.map_err(Into::into)
}

#[command]
pub async fn get_profile(
    state: State<'_, AppState>,
    id: String,
) -> Result<ProfileDetail, PublicError> {
    state.profiles.get_profile(&id).await.map_err(Into::into)
}

#[command]
pub async fn create_profile(
    state: State<'_, AppState>,
    draft: ProfileDraft,
) -> Result<ProfileDetail, PublicError> {
    state
        .profiles
        .create_profile(draft)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn update_profile(
    state: State<'_, AppState>,
    id: String,
    expected_revision: i64,
    draft: ProfileDraft,
) -> Result<ProfileDetail, PublicError> {
    state
        .profiles
        .update_profile(&id, expected_revision, draft)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn duplicate_profile(
    state: State<'_, AppState>,
    id: String,
) -> Result<ProfileDetail, PublicError> {
    state
        .profiles
        .duplicate_profile(&id)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn delete_profile(
    state: State<'_, AppState>,
    id: String,
    confirmation: String,
) -> Result<(), PublicError> {
    if confirmation != "DELETE" {
        return Err(crate::domain::error::AppError::Validation(
            "Type DELETE to confirm profile deletion".to_string(),
        )
        .into());
    }
    state.profiles.delete_profile(&id).await.map_err(Into::into)
}

#[command]
pub async fn test_profile(
    state: State<'_, AppState>,
    draft: ProfileDraft,
) -> Result<ConnectionTestResult, PublicError> {
    state.profiles.test_profile(draft).await.map_err(Into::into)
}
