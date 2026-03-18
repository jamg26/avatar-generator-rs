use axum::{ extract::{ Request, State }, middleware::Next, response::Response };
use sha2::{ Digest, Sha256 };

use crate::{ db, error::AppError, routes::AppState };

/// Extracts the API key from the `X-API-Key` header, validates it against the
/// database, checks the monthly quota, and injects the resolved key row into
/// request extensions so downstream handlers can access it.
pub async fn require_api_key(
    State(state): State<AppState>,
    mut req: Request,
    next: Next
) -> Result<Response, AppError> {
    let header = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("Missing X-API-Key header".into()))?;

    let key_hash = hash_key(header);

    let key_row = db
        ::find_api_key_by_hash(&state.pool, &key_hash).await?
        .ok_or_else(|| AppError::Unauthorized("Invalid API key".into()))?;

    // Quota check
    let used = db::monthly_usage_count(&state.pool, &key_row.id).await?;
    if used >= key_row.monthly_quota {
        return Err(AppError::QuotaExceeded);
    }

    // Stash key info for downstream handlers
    req.extensions_mut().insert(key_row);

    Ok(next.run(req).await)
}

/// SHA-256 hash of the raw API key string (hex-encoded).
pub fn hash_key(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}
