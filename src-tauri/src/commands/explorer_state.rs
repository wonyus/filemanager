use tauri::{command, State};

use crate::{
    app_state::AppState,
    domain::error::PublicError,
    dto::explorer_state::{
        AddBookmarkRequest, Bookmark, ListBookmarksRequest, ListRecentLocationsRequest,
        RecentLocation, RecordRecentLocationRequest, RemoveBookmarkRequest,
    },
};

#[command]
pub async fn add_bookmark(
    state: State<'_, AppState>,
    request: AddBookmarkRequest,
) -> Result<Bookmark, PublicError> {
    state
        .database
        .add_bookmark(request)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn list_bookmarks(
    state: State<'_, AppState>,
    request: ListBookmarksRequest,
) -> Result<Vec<Bookmark>, PublicError> {
    state
        .database
        .list_bookmarks(request)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn remove_bookmark(
    state: State<'_, AppState>,
    request: RemoveBookmarkRequest,
) -> Result<(), PublicError> {
    state
        .database
        .remove_bookmark(request.id)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn record_recent_location(
    state: State<'_, AppState>,
    request: RecordRecentLocationRequest,
) -> Result<RecentLocation, PublicError> {
    state
        .database
        .record_recent_location(request)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn list_recent_locations(
    state: State<'_, AppState>,
    request: ListRecentLocationsRequest,
) -> Result<Vec<RecentLocation>, PublicError> {
    state
        .database
        .list_recent_locations(request)
        .await
        .map_err(Into::into)
}
