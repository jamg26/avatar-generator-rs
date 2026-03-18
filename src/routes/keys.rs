use axum::{ extract::State, http::HeaderMap, response::Json };
use rand::Rng;
use serde::Deserialize;
use serde_json::{ json, Value };

use crate::{ db, error::AppError, middleware::api_key::hash_key, routes::AppState };

#[derive(Deserialize)]
pub struct CreateKeyRequest {
    /// Human-readable label for this key.
    pub name: String,
    /// Monthly request quota (default 500).
    #[serde(default = "default_quota")]
    pub monthly_quota: i64,
}

fn default_quota() -> i64 {
    500
}

/// POST /api/admin/keys — create a new API key (admin only).
pub async fn create_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateKeyRequest>
) -> Result<Json<Value>, AppError> {
    verify_admin(&headers, &state.config.admin_secret)?;

    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }

    // Generate raw API key: avg_<32 hex chars>
    let raw_key = generate_raw_key();
    let prefix = &raw_key[..8]; // "avg_XXXX"
    let hashed = hash_key(&raw_key);
    let id = uuid::Uuid::new_v4().to_string();

    db::insert_api_key(
        &state.pool,
        &id,
        body.name.trim(),
        &hashed,
        prefix,
        body.monthly_quota
    ).await?;

    // Return the raw key ONCE — it cannot be retrieved again.
    Ok(
        Json(
            json!({
        "id": id,
        "name": body.name,
        "key": raw_key,
        "prefix": prefix,
        "monthly_quota": body.monthly_quota,
        "message": "Store this key securely — it will not be shown again."
    })
        )
    )
}

/// GET /api/admin/keys — list all keys (admin only).
pub async fn list_keys(
    State(state): State<AppState>,
    headers: HeaderMap
) -> Result<Json<Value>, AppError> {
    verify_admin(&headers, &state.config.admin_secret)?;
    let keys = db::list_api_keys(&state.pool).await?;

    let list: Vec<Value> = keys
        .iter()
        .map(|k| {
            json!({
                "id": k.id,
                "name": k.name,
                "prefix": k.key_prefix,
                "monthly_quota": k.monthly_quota,
                "is_active": k.is_active,
                "created_at": k.created_at,
            })
        })
        .collect();

    Ok(Json(json!({ "keys": list })))
}

/// DELETE /api/admin/keys/:id — revoke a key (admin only).
pub async fn revoke_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>
) -> Result<Json<Value>, AppError> {
    verify_admin(&headers, &state.config.admin_secret)?;

    let revoked = db::revoke_api_key(&state.pool, &id).await?;
    if !revoked {
        return Err(AppError::NotFound("API key not found".into()));
    }

    Ok(Json(json!({ "revoked": true })))
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn verify_admin(headers: &HeaderMap, expected: &str) -> Result<(), AppError> {
    let provided = headers
        .get("x-admin-secret")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("Missing X-Admin-Secret header".into()))?;

    if provided != expected {
        return Err(AppError::Forbidden("Invalid admin secret".into()));
    }
    Ok(())
}

fn generate_raw_key() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.gen();
    format!("avg_{}", hex::encode(bytes))
}
