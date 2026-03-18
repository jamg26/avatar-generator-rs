use axum::{ extract::State, response::Json, Extension };
use serde_json::{ json, Value };

use crate::{ db, error::AppError, routes::AppState };

/// GET /api/v1/usage — returns usage stats for the authenticated API key.
pub async fn my_usage(
    State(state): State<AppState>,
    Extension(key): Extension<db::ApiKeyRow>
) -> Result<Json<Value>, AppError> {
    let monthly = db::monthly_usage_count(&state.pool, &key.id).await?;
    let daily = db::daily_usage(&state.pool, &key.id, 30).await?;

    Ok(
        Json(
            json!({
        "key_id": key.id,
        "name": key.name,
        "monthly_quota": key.monthly_quota,
        "monthly_used": monthly,
        "monthly_remaining": key.monthly_quota - monthly,
        "daily_breakdown": daily,
    })
        )
    )
}
