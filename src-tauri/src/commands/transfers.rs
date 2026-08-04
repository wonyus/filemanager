use tauri::{command, State};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    domain::error::PublicError,
    dto::transfer::{
        ClearTransferHistoryRequest, ListTransfersRequest, StartTransferRequest, TransferDetails,
        TransferHistoryPage, TransferJob,
    },
};

#[command]
pub async fn start_transfer(
    state: State<'_, AppState>,
    request: StartTransferRequest,
) -> Result<TransferJob, PublicError> {
    state
        .transfer_service
        .start(request)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn list_transfers(
    state: State<'_, AppState>,
    request: ListTransfersRequest,
) -> Result<TransferHistoryPage, PublicError> {
    state.transfers.list_page(request).await.map_err(Into::into)
}

#[command]
pub async fn get_transfer_details(
    state: State<'_, AppState>,
    transfer_id: String,
) -> Result<TransferDetails, PublicError> {
    let id = Uuid::parse_str(&transfer_id).map_err(|_| {
        crate::domain::error::AppError::Validation("transferId must be a UUID".to_string())
    })?;
    state.transfers.details(id).await.map_err(Into::into)
}

#[command]
pub async fn pause_transfer(
    state: State<'_, AppState>,
    transfer_id: String,
) -> Result<TransferJob, PublicError> {
    let id = parse_id(&transfer_id)?;
    state.transfers.request_pause(id).await.map_err(Into::into)
}

#[command]
pub async fn resume_transfer(
    state: State<'_, AppState>,
    transfer_id: String,
) -> Result<TransferJob, PublicError> {
    let id = parse_id(&transfer_id)?;
    state.transfers.request_resume(id).await.map_err(Into::into)
}

#[command]
pub async fn cancel_transfer(
    state: State<'_, AppState>,
    transfer_id: String,
) -> Result<TransferJob, PublicError> {
    let id = parse_id(&transfer_id)?;
    state.transfers.request_cancel(id).await.map_err(Into::into)
}

#[command]
pub async fn retry_transfer(
    state: State<'_, AppState>,
    transfer_id: String,
) -> Result<TransferJob, PublicError> {
    let id = parse_id(&transfer_id)?;
    state.transfer_service.retry(id).await.map_err(Into::into)
}

#[command]
pub async fn clear_transfer_history(
    state: State<'_, AppState>,
    request: ClearTransferHistoryRequest,
) -> Result<usize, PublicError> {
    state
        .transfers
        .clear_history(request)
        .await
        .map_err(Into::into)
}

#[command]
pub async fn interrupt_active_transfers(state: State<'_, AppState>) -> Result<usize, PublicError> {
    Ok(state.transfers.interrupt_active().await)
}

#[allow(clippy::result_large_err)]
fn parse_id(value: &str) -> Result<Uuid, PublicError> {
    Uuid::parse_str(value).map_err(|_| {
        crate::domain::error::AppError::Validation("transferId must be a UUID".to_string()).into()
    })
}
