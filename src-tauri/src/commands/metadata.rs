use tauri::{command, State};

use crate::{
    app_state::AppState,
    domain::error::PublicError,
    dto::metadata::{
        ObjectMetadata, ObjectRequest, PreviewRequest, PreviewResult, ShareLink, ShareLinkRequest,
    },
};

#[command]
pub async fn head_object(
    state: State<'_, AppState>,
    request: ObjectRequest,
) -> Result<ObjectMetadata, PublicError> {
    state
        .profiles
        .head_object(request)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn preview_object(
    state: State<'_, AppState>,
    request: PreviewRequest,
) -> Result<PreviewResult, PublicError> {
    state
        .profiles
        .preview_object(request)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn create_share_link(
    state: State<'_, AppState>,
    request: ShareLinkRequest,
) -> Result<ShareLink, PublicError> {
    state
        .profiles
        .create_share_link(request)
        .await
        .map_err(Into::into)
}
