//! Admin API routes — all protected by X-Admin-Secret header.
//!
//! GET  /admin                    → serve embedded admin HTML
//! GET  /api/admin/dashboard      → overview stats
//! GET  /api/admin/metrics        → usage charts data (?days=30)
//! GET  /api/admin/jobs           → list batch jobs
//! POST /api/admin/jobs/:id/cancel→ cancel an active job
//! DEL  /api/admin/jobs/:id       → delete a job record
//! GET  /api/admin/keys           → list all keys (with usage)
//! POST /api/admin/keys           → create key  (already in keys.rs, re-exported)
//! DEL  /api/admin/keys/:id       → revoke key  (already in keys.rs, re-exported)
//! DEL  /api/admin/keys/:id/hard  → hard-delete key + its usage logs
//! PATCH /api/admin/keys/:id      → update monthly_quota

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Html,
    Json,
};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{error::AppError, routes::AppState};

// ── Embedded HTML ─────────────────────────────────────────────────────────────

static ADMIN_HTML: &str = include_str!("../../admin/index.html");

pub async fn serve_admin() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

// ── Auth helper ───────────────────────────────────────────────────────────────

fn verify(h: &HeaderMap, expected: &str) -> Result<(), AppError> {
    let v = h.get("x-admin-secret")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("Missing X-Admin-Secret".into()))?;
    if v != expected {
        return Err(AppError::Forbidden("Invalid admin secret".into()));
    }
    Ok(())
}

// ── sqlx row types ────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow, serde::Serialize)]
#[allow(dead_code)]
struct DashKey { total: i64, active: i64 }

#[derive(sqlx::FromRow, serde::Serialize)]
struct RecentRow {
    name:       Option<String>,
    key_prefix: Option<String>,
    endpoint:   String,
    count:      i64,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow, serde::Serialize)]
struct AdminKeyRow {
    id:            String,
    name:          String,
    key_prefix:    String,
    monthly_quota: i64,
    is_active:     bool,
    created_at:    chrono::DateTime<chrono::Utc>,
    monthly_used:  i64,
    total_used:    i64,
}

#[derive(sqlx::FromRow, serde::Serialize)]
struct DailyRow  { d: NaiveDate, n: i64 }
#[derive(sqlx::FromRow, serde::Serialize)]
struct HourlyRow { h: i32, n: i64 }
#[derive(sqlx::FromRow, serde::Serialize)]
struct PerKeyRow { name: String, monthly_quota: i64, used: i64 }
#[derive(sqlx::FromRow, serde::Serialize)]
struct EndptRow  { endpoint: String, n: i64 }

#[derive(sqlx::FromRow, serde::Serialize)]
struct JobRow {
    id:             Uuid,
    state:          String,
    total:          i64,
    completed:      i64,
    failed_count:   i64,
    model:          String,
    download_url:   Option<String>,
    error_msg:      Option<String>,
    api_key_id:     String,
    created_at:     chrono::DateTime<chrono::Utc>,
    updated_at:     chrono::DateTime<chrono::Utc>,
    key_name:       Option<String>,
    key_prefix:     Option<String>,
    queue_position: Option<i64>,
}

// ── Dashboard ─────────────────────────────────────────────────────────────────

pub async fn dashboard(
    State(s): State<AppState>,
    h: HeaderMap,
) -> Result<Json<Value>, AppError> {
    verify(&h, &s.config.admin_secret)?;

    let (total, active): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*)::bigint, COUNT(*) FILTER(WHERE is_active)::bigint FROM api_keys",
    ).fetch_one(&s.pool).await.unwrap_or((0, 0));

    let (today,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(count),0)::bigint FROM usage_log WHERE created_at >= CURRENT_DATE",
    ).fetch_one(&s.pool).await.unwrap_or((0,));

    let (month,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(count),0)::bigint FROM usage_log \
         WHERE created_at >= DATE_TRUNC('month',NOW())",
    ).fetch_one(&s.pool).await.unwrap_or((0,));

    let (active_jobs,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::bigint FROM batch_jobs \
         WHERE state IN ('queued','running','uploading')",
    ).fetch_one(&s.pool).await.unwrap_or((0,));

    let recent: Vec<RecentRow> = sqlx::query_as(
        "SELECT ak.name, ak.key_prefix, ul.endpoint, ul.count, ul.created_at \
         FROM usage_log ul \
         LEFT JOIN api_keys ak ON ak.id = ul.api_key_id \
         ORDER BY ul.created_at DESC LIMIT 20",
    ).fetch_all(&s.pool).await.unwrap_or_default();

    let health = check_health().await;

    Ok(Json(json!({
        "keys":        { "total": total, "active": active },
        "requests":    { "today": today, "month": month },
        "active_jobs": active_jobs,
        "health":      health,
        "recent":      recent,
    })))
}

// ── Metrics ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct MetricsQuery { days: Option<i64> }

pub async fn metrics(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<MetricsQuery>,
) -> Result<Json<Value>, AppError> {
    verify(&h, &s.config.admin_secret)?;
    let days = q.days.unwrap_or(30).clamp(1, 365);

    let daily: Vec<DailyRow> = sqlx::query_as(
        "SELECT DATE(created_at) AS d, COALESCE(SUM(count),0)::bigint AS n \
         FROM usage_log WHERE created_at >= NOW()-($1::bigint*INTERVAL '1 day') \
         GROUP BY d ORDER BY d",
    ).bind(days).fetch_all(&s.pool).await.unwrap_or_default();

    let hourly: Vec<HourlyRow> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at)::int AS h, \
                COALESCE(SUM(count),0)::bigint AS n \
         FROM usage_log WHERE created_at >= CURRENT_DATE GROUP BY h ORDER BY h",
    ).fetch_all(&s.pool).await.unwrap_or_default();

    let per_key: Vec<PerKeyRow> = sqlx::query_as(
        "SELECT ak.name, ak.monthly_quota, \
                COALESCE(SUM(ul.count),0)::bigint AS used \
         FROM api_keys ak \
         LEFT JOIN usage_log ul ON ul.api_key_id = ak.id \
                AND ul.created_at >= DATE_TRUNC('month',NOW()) \
         WHERE ak.is_active = TRUE \
         GROUP BY ak.id, ak.name, ak.monthly_quota ORDER BY used DESC",
    ).fetch_all(&s.pool).await.unwrap_or_default();

    let endpoints: Vec<EndptRow> = sqlx::query_as(
        "SELECT endpoint, COALESCE(SUM(count),0)::bigint AS n \
         FROM usage_log WHERE created_at >= NOW()-INTERVAL '30 days' \
         GROUP BY endpoint ORDER BY n DESC LIMIT 10",
    ).fetch_all(&s.pool).await.unwrap_or_default();

    Ok(Json(json!({
        "daily":     daily,
        "hourly":    hourly,
        "per_key":   per_key,
        "endpoints": endpoints,
    })))
}

// ── Jobs ──────────────────────────────────────────────────────────────────────

pub async fn list_jobs(
    State(s): State<AppState>,
    h: HeaderMap,
) -> Result<Json<Value>, AppError> {
    verify(&h, &s.config.admin_secret)?;
    let jobs: Vec<JobRow> = sqlx::query_as(
        "SELECT bj.id, bj.state, bj.total, bj.completed, bj.failed_count,
                bj.model, bj.download_url, bj.error_msg, bj.api_key_id,
                bj.created_at, bj.updated_at,
                ak.name       AS key_name,
                ak.key_prefix AS key_prefix,
                CASE WHEN bj.state IN ('queued','running','uploading') THEN
                    (SELECT COUNT(*) FROM batch_jobs b2
                     WHERE  b2.state IN ('queued','running','uploading')
                       AND  b2.created_at < bj.created_at) + 1
                ELSE NULL END AS queue_position
         FROM   batch_jobs bj
         LEFT JOIN api_keys ak ON ak.id = bj.api_key_id
         ORDER BY bj.created_at DESC LIMIT 200",
    ).fetch_all(&s.pool).await?;
    Ok(Json(json!({ "jobs": jobs })))
}

pub async fn cancel_job(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    verify(&h, &s.config.admin_secret)?;
    let r = sqlx::query(
        "UPDATE batch_jobs SET state='cancelled', updated_at=NOW() \
         WHERE id=$1 AND state IN ('queued','running','uploading')",
    ).bind(id).execute(&s.pool).await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound("Job not found or already finished".into()));
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_job(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    verify(&h, &s.config.admin_secret)?;
    let r = sqlx::query("DELETE FROM batch_jobs WHERE id=$1")
        .bind(id).execute(&s.pool).await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound("Job not found".into()));
    }
    Ok(Json(json!({ "ok": true })))
}

// ── Keys (extended) ───────────────────────────────────────────────────────────

pub async fn list_keys_with_usage(
    State(s): State<AppState>,
    h: HeaderMap,
) -> Result<Json<Value>, AppError> {
    verify(&h, &s.config.admin_secret)?;
    let keys: Vec<AdminKeyRow> = sqlx::query_as(
        "SELECT ak.id, ak.name, ak.key_prefix, ak.monthly_quota, ak.is_active, ak.created_at,
                COALESCE(SUM(ul.count) FILTER(
                    WHERE ul.created_at >= DATE_TRUNC('month',NOW())), 0)::bigint AS monthly_used,
                COALESCE(SUM(ul.count), 0)::bigint AS total_used
         FROM   api_keys ak
         LEFT JOIN usage_log ul ON ul.api_key_id = ak.id
         GROUP BY ak.id ORDER BY ak.created_at DESC",
    ).fetch_all(&s.pool).await?;
    Ok(Json(json!({ "keys": keys })))
}

pub async fn hard_delete_key(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    verify(&h, &s.config.admin_secret)?;
    sqlx::query("DELETE FROM usage_log WHERE api_key_id=$1")
        .bind(&id).execute(&s.pool).await?;
    let r = sqlx::query("DELETE FROM api_keys WHERE id=$1")
        .bind(&id).execute(&s.pool).await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound("API key not found".into()));
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct UpdateKeyBody { monthly_quota: Option<i64>, name: Option<String> }

pub async fn update_key(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateKeyBody>,
) -> Result<Json<Value>, AppError> {
    verify(&h, &s.config.admin_secret)?;
    if let Some(q) = body.monthly_quota {
        sqlx::query("UPDATE api_keys SET monthly_quota=$2, updated_at=NOW() WHERE id=$1")
            .bind(&id).bind(q).execute(&s.pool).await?;
    }
    if let Some(name) = body.name {
        sqlx::query("UPDATE api_keys SET name=$2, updated_at=NOW() WHERE id=$1")
            .bind(&id).bind(name).execute(&s.pool).await?;
    }
    Ok(Json(json!({ "ok": true })))
}

// ── System info ───────────────────────────────────────────────────────────────

pub async fn system_info(
    State(s): State<AppState>,
    h: HeaderMap,
) -> Result<Json<Value>, AppError> {
    verify(&h, &s.config.admin_secret)?;

    let db = match sqlx::query_as::<_, (String,)>("SELECT version()")
        .fetch_one(&s.pool).await
    {
        Ok((v,)) => {
            let (k,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM api_keys")
                .fetch_one(&s.pool).await.unwrap_or((0,));
            let (l,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM usage_log")
                .fetch_one(&s.pool).await.unwrap_or((0,));
            let (j,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM batch_jobs")
                .fetch_one(&s.pool).await.unwrap_or((0,));
            json!({ "status": "connected", "version": v, "keys": k, "logs": l, "jobs": j })
        }
        Err(e) => json!({ "status": "error", "error": e.to_string() }),
    };

    let health = check_health().await;

    Ok(Json(json!({ "health": health, "db": db })))
}

// ── Health check helper ───────────────────────────────────────────────────────

async fn check_health() -> &'static str {
    // We're running inside the server, so it's always "online"
    "online"
}
