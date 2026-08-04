use tauri::{command, State};

use crate::{app_state::AppState, domain::error::PublicError, dto::profile::ProfileSummary};

#[command]
pub async fn list_profiles(state: State<'_, AppState>) -> Result<Vec<ProfileSummary>, PublicError> {
    state.database.list_profiles().await.map_err(Into::into)
}
