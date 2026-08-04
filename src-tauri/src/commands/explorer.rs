use tauri::{command, State};

use crate::{
    app_state::AppState,
    domain::error::PublicError,
    dto::{
        explorer::{ListEntriesPage, ListEntriesRequest},
        profile::BucketSummary,
    },
};

#[command]
pub async fn list_buckets(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<BucketSummary>, PublicError> {
    state
        .profiles
        .list_buckets(&profile_id)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn list_entries(
    state: State<'_, AppState>,
    request: ListEntriesRequest,
) -> Result<ListEntriesPage, PublicError> {
    state
        .profiles
        .list_entries(request)
        .await
        .map_err(Into::into)
}
