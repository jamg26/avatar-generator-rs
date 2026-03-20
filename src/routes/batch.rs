use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use uuid::Uuid;

use crate::{
    db,
    error::AppError,
    generator::bulk::{BatchJobStatus, BulkRequest},
    routes::AppState,
};

/// POST /api/v1/batch/generate
///
/// Submits a bulk generation job. Returns 202 immediately; generation runs in
/// the background saving images to `{SAVE_DIR}/{job_id}/`.
pub async fn submit(
    State(state): State<AppState>,
    Extension(key): Extension<db::ApiKeyRow>,
    Json(req): Json<BulkRequest>,
) -> Result<(StatusCode, Json<BatchJobStatus>), AppError> {
    if req.count == 0 || req.count > 1_000_000 {
        return Err(AppError::BadRequest(
            "count must be between 1 and 1,000,000".into(),
        ));
    }

    tracing::info!(
        key_prefix = %key.key_prefix,
        count = req.count,
        model = ?req.model,
        "Submitting bulk job"
    );

    // Pre-check: ensure the key has enough quota for the entire batch.
    let used = db::monthly_usage_count(&state.pool, &key.id).await?;
    let remaining = key.monthly_quota - used;
    if req.count as i64 > remaining {
        return Err(AppError::QuotaExceeded);
    }

    let image_count = req.count as i64;
    let status = state.bulk_pipeline.submit(req).await?;

    // Charge all images up-front so the quota is immediately reflected.
    db::record_usage(&state.pool, &key.id, "/api/v1/batch/generate", image_count).await?;

    Ok((StatusCode::ACCEPTED, Json(status)))
}

/// GET /api/v1/batch/{job_id}
///
/// Returns the current status of a bulk job.
pub async fn get_job(
    State(state): State<AppState>,
    Extension(_key): Extension<db::ApiKeyRow>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<BatchJobStatus>, AppError> {
    state
        .bulk_pipeline
        .get_status(job_id)
        .await?
        .map(Json)
        .ok_or_else(|| AppError::NotFound(format!("job {job_id} not found")))
}

/// GET /api/v1/batch
///
/// Returns all bulk jobs (in-memory, not persisted across restarts).
pub async fn list_jobs(
    State(state): State<AppState>,
    Extension(_key): Extension<db::ApiKeyRow>,
) -> Result<Json<Vec<BatchJobStatus>>, AppError> {
    Ok(Json(state.bulk_pipeline.list_all().await?))
}
