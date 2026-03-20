//! AvaGen Admin Panel – standalone Rust binary.
//!
//! Build:  cargo build --release --bin avagen-admin
//! Run:    ./target/release/avagen-admin
//!         (reads ADMIN_SECRET / DATABASE_URL / API_BASE_URL / ADMIN_PORT from .env)

use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::Context;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use dotenvy::dotenv;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

// ── Embedded UI ───────────────────────────────────────────────────────────────

static HTML: &str = include_str!("../../admin/index.html");

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    pool:         PgPool,
    admin_secret: String,
    api_base:     String,
    sessions:     Arc<RwLock<HashSet<String>>>,
    http:         Client,
}

// ── DB row types ──────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow, Serialize)]
struct KeyCounts { total: i64, active: i64 }

#[derive(sqlx::FromRow, Serialize)]
struct RecentItem {
    api_key_id: String,
    name:       Option<String>,
    key_prefix: Option<String>,
    endpoint:   String,
    count:      i64,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow, Serialize)]
struct AdminKeyRow {
    id:            String,
    name:          String,
    key_prefix:    String,
    monthly_quota: i64,
    is_active:     bool,
    created_at:    DateTime<Utc>,
    updated_at:    DateTime<Utc>,
    monthly_used:  i64,
    total_used:    i64,
}

#[derive(sqlx::FromRow, Serialize)]
struct DailyRow  { d: NaiveDate, n: i64 }

#[derive(sqlx::FromRow, Serialize)]
struct HourlyRow { h: i32, n: i64 }

#[derive(sqlx::FromRow, Serialize)]
struct PerKeyRow {
    name:          String,
    key_prefix:    String,
    monthly_quota: i64,
    used:          i64,
}

#[derive(sqlx::FromRow, Serialize)]
struct EndpointRow { endpoint: String, n: i64 }

#[derive(sqlx::FromRow, Serialize)]
struct AdminJobRow {
    id:             Uuid,
    state:          String,
    total:          i64,
    completed:      i64,
    failed_count:   i64,
    model:          String,
    download_url:   Option<String>,
    error_msg:      Option<String>,
    api_key_id:     String,
    created_at:     DateTime<Utc>,
    updated_at:     DateTime<Utc>,
    key_name:       Option<String>,
    key_prefix:     Option<String>,
    queue_position: Option<i64>,
}

// ── Auth helpers ──────────────────────────────────────────────────────────────

fn token_ok(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .map(|t| state.sessions.read().unwrap().contains(t))
        .unwrap_or(false)
}

/// Early-return 401 if token is missing / invalid.
macro_rules! auth {
    ($state:expr, $headers:expr) => {
        if !token_ok(&$state, &$headers) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            )
                .into_response();
        }
    };
}

// ── Route handlers ────────────────────────────────────────────────────────────

async fn serve_html() -> Html<&'static str> { Html(HTML) }

async fn auth_status(State(s): State<AppState>, h: HeaderMap) -> impl IntoResponse {
    Json(json!({"ok": token_ok(&s, &h)}))
}

async fn login(State(s): State<AppState>, Json(body): Json<Value>) -> impl IntoResponse {
    let secret = body.get("secret").and_then(|v| v.as_str()).unwrap_or("");
    if s.admin_secret.is_empty() {
        return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "ADMIN_SECRET not configured"}))).into_response();
    }
    if secret != s.admin_secret {
        return (StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Invalid secret"}))).into_response();
    }
    use rand::Rng;
    let token: String = hex::encode(rand::thread_rng().gen::<[u8; 32]>());
    s.sessions.write().unwrap().insert(token.clone());
    Json(json!({"ok": true, "token": token})).into_response()
}

async fn logout(State(s): State<AppState>, Json(body): Json<Value>) -> impl IntoResponse {
    if let Some(t) = body.get("token").and_then(|v| v.as_str()) {
        s.sessions.write().unwrap().remove(t);
    }
    Json(json!({"ok": true}))
}

async fn dashboard(State(s): State<AppState>, h: HeaderMap) -> impl IntoResponse {
    auth!(s, h);

    let keys = sqlx::query_as::<_, KeyCounts>(
        "SELECT COUNT(*)::bigint AS total,
                COUNT(*) FILTER(WHERE is_active)::bigint AS active
         FROM api_keys",
    )
    .fetch_one(&s.pool)
    .await
    .unwrap_or(KeyCounts { total: 0, active: 0 });

    let (today,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(count),0)::bigint FROM usage_log WHERE created_at >= CURRENT_DATE",
    )
    .fetch_one(&s.pool).await.unwrap_or((0,));

    let (month,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(count),0)::bigint FROM usage_log
         WHERE created_at >= DATE_TRUNC('month',NOW())",
    )
    .fetch_one(&s.pool).await.unwrap_or((0,));

    let (active_jobs,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::bigint FROM batch_jobs
         WHERE state IN ('queued','running','uploading')",
    )
    .fetch_one(&s.pool).await.unwrap_or((0,));

    let recent = sqlx::query_as::<_, RecentItem>(
        "SELECT ul.api_key_id, ak.name, ak.key_prefix,
                ul.endpoint, ul.count, ul.created_at
         FROM   usage_log ul
         LEFT JOIN api_keys ak ON ak.id = ul.api_key_id
         ORDER BY ul.created_at DESC LIMIT 15",
    )
    .fetch_all(&s.pool).await.unwrap_or_default();

    let health = match s.http
        .get(format!("{}/health", s.api_base))
        .timeout(Duration::from_secs(3))
        .send().await
    {
        Ok(r) if r.status().is_success() => "online",
        Ok(_)  => "degraded",
        Err(_) => "offline",
    };

    Json(json!({
        "keys":            {"total": keys.total, "active": keys.active},
        "requests":        {"today": today, "month": month},
        "active_jobs":     active_jobs,
        "health":          health,
        "recent_activity": recent,
    }))
    .into_response()
}

async fn list_keys(State(s): State<AppState>, h: HeaderMap) -> impl IntoResponse {
    auth!(s, h);
    match sqlx::query_as::<_, AdminKeyRow>(
        "SELECT ak.id, ak.name, ak.key_prefix, ak.monthly_quota,
                ak.is_active, ak.created_at, ak.updated_at,
                COALESCE(SUM(ul.count) FILTER(
                    WHERE ul.created_at >= DATE_TRUNC('month',NOW())),
                    0)::bigint AS monthly_used,
                COALESCE(SUM(ul.count), 0)::bigint AS total_used
         FROM   api_keys ak
         LEFT JOIN usage_log ul ON ul.api_key_id = ak.id
         GROUP BY ak.id
         ORDER BY ak.created_at DESC",
    )
    .fetch_all(&s.pool).await
    {
        Ok(rows) => Json(json!({"keys": rows})).into_response(),
        Err(e)   => (StatusCode::INTERNAL_SERVER_ERROR,
                     Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn create_key(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    auth!(s, h);
    match s.http
        .post(format!("{}/api/admin/keys", s.api_base))
        .header("X-Admin-Secret", &s.admin_secret)
        .json(&body)
        .send().await
    {
        Ok(r) => {
            let st = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::OK);
            let b: Value = r.json().await.unwrap_or(json!({}));
            (st, Json(b)).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY,
                   Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn revoke_key(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(key_id): Path<String>,
) -> impl IntoResponse {
    auth!(s, h);
    match s.http
        .delete(format!("{}/api/admin/keys/{}", s.api_base, key_id))
        .header("X-Admin-Secret", &s.admin_secret)
        .send().await
    {
        Ok(r) => {
            let st = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::OK);
            let b: Value = r.json().await.unwrap_or(json!({}));
            (st, Json(b)).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY,
                   Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
struct MetricsQuery { days: Option<i64> }

async fn metrics(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<MetricsQuery>,
) -> impl IntoResponse {
    auth!(s, h);
    let days = q.days.unwrap_or(30);

    let daily = sqlx::query_as::<_, DailyRow>(
        "SELECT DATE(created_at) AS d, COALESCE(SUM(count),0)::bigint AS n
         FROM   usage_log
         WHERE  created_at >= NOW() - ($1::bigint * INTERVAL '1 day')
         GROUP BY d ORDER BY d",
    )
    .bind(days).fetch_all(&s.pool).await.unwrap_or_default();

    let per_key = sqlx::query_as::<_, PerKeyRow>(
        "SELECT ak.name, ak.key_prefix, ak.monthly_quota,
                COALESCE(SUM(ul.count),0)::bigint AS used
         FROM   api_keys ak
         LEFT JOIN usage_log ul
                ON ul.api_key_id = ak.id
               AND ul.created_at >= DATE_TRUNC('month',NOW())
         WHERE  ak.is_active = TRUE
         GROUP BY ak.id, ak.name, ak.key_prefix, ak.monthly_quota
         ORDER BY used DESC",
    )
    .fetch_all(&s.pool).await.unwrap_or_default();

    let hourly = sqlx::query_as::<_, HourlyRow>(
        "SELECT EXTRACT(HOUR FROM created_at)::int AS h,
                COALESCE(SUM(count),0)::bigint      AS n
         FROM   usage_log WHERE created_at >= CURRENT_DATE
         GROUP BY h ORDER BY h",
    )
    .fetch_all(&s.pool).await.unwrap_or_default();

    let endpoints = sqlx::query_as::<_, EndpointRow>(
        "SELECT endpoint, COALESCE(SUM(count),0)::bigint AS n
         FROM   usage_log WHERE created_at >= NOW() - INTERVAL '30 days'
         GROUP BY endpoint ORDER BY n DESC LIMIT 10",
    )
    .fetch_all(&s.pool).await.unwrap_or_default();

    Json(json!({
        "daily":     daily,
        "per_key":   per_key,
        "hourly":    hourly,
        "endpoints": endpoints,
    }))
    .into_response()
}

async fn list_jobs(State(s): State<AppState>, h: HeaderMap) -> impl IntoResponse {
    auth!(s, h);
    match sqlx::query_as::<_, AdminJobRow>(
        "SELECT bj.id, bj.state, bj.total, bj.completed, bj.failed_count,
                bj.model, bj.download_url, bj.error_msg, bj.api_key_id,
                bj.created_at, bj.updated_at,
                ak.name  AS key_name,
                ak.key_prefix,
                CASE WHEN bj.state IN ('queued','running','uploading') THEN
                    (SELECT COUNT(*) FROM batch_jobs b2
                     WHERE  b2.state IN ('queued','running','uploading')
                       AND  b2.created_at < bj.created_at) + 1
                ELSE NULL END AS queue_position
         FROM   batch_jobs bj
         LEFT JOIN api_keys ak ON ak.id = bj.api_key_id
         ORDER BY bj.created_at DESC LIMIT 200",
    )
    .fetch_all(&s.pool).await
    {
        Ok(rows) => Json(json!({"jobs": rows})).into_response(),
        Err(e)   => (StatusCode::INTERNAL_SERVER_ERROR,
                     Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn system_info(State(s): State<AppState>, h: HeaderMap) -> impl IntoResponse {
    auth!(s, h);

    let api = match s.http
        .get(format!("{}/health", s.api_base))
        .timeout(Duration::from_secs(5))
        .send().await
    {
        Ok(r) => {
            let body: Value = r.json().await.unwrap_or(json!({}));
            json!({"status": "online", "url": s.api_base, "response": body})
        }
        Err(e) => json!({"status": "offline", "url": s.api_base, "error": e.to_string()}),
    };

    let db = match sqlx::query_as::<_, (String,)>("SELECT version()")
        .fetch_one(&s.pool).await
    {
        Ok((v,)) => {
            let (keys,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM api_keys")
                .fetch_one(&s.pool).await.unwrap_or((0,));
            let (logs,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM usage_log")
                .fetch_one(&s.pool).await.unwrap_or((0,));
            let (jobs,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM batch_jobs")
                .fetch_one(&s.pool).await.unwrap_or((0,));
            json!({
                "status":    "connected",
                "version":   v,
                "key_count": keys,
                "log_count": logs,
                "job_count": jobs,
            })
        }
        Err(e) => json!({"status": "error", "error": e.to_string()}),
    };

    Json(json!({
        "api":    api,
        "db":     db,
        "config": {
            "api_base_url":            s.api_base,
            "admin_secret_configured": !s.admin_secret.is_empty(),
            "db_configured":           true,
        },
    }))
    .into_response()
}

// ── Router ────────────────────────────────────────────────────────────────────

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/",               get(serve_html))
        .route("/auth/status",    get(auth_status))
        .route("/auth/login",     post(login))
        .route("/auth/logout",    post(logout))
        .route("/api/dashboard",  get(dashboard))
        .route("/api/keys",       get(list_keys))
        .route("/api/keys",       post(create_key))
        .route("/api/keys/{id}",   delete(revoke_key))
        .route("/api/metrics",    get(metrics))
        .route("/api/jobs",       get(list_jobs))
        .route("/api/system",     get(system_info))
        .with_state(state)
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenv();

    let admin_secret = std::env::var("ADMIN_SECRET").unwrap_or_default();
    let database_url  = std::env::var("DATABASE_URL").unwrap_or_default();
    let api_base      = std::env::var("API_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:8080".into());
    let port: u16     = std::env::var("ADMIN_PORT")
        .ok().and_then(|p| p.parse().ok()).unwrap_or(5001);

    let bar = "─".repeat(50);
    println!("\n  {bar}");
    println!("    🎨  AvaGen Admin Panel");
    println!("  {bar}");
    println!("    URL    →  http://127.0.0.1:{port}");
    println!("    API    →  {api_base}");
    println!("    DB     →  {}", if database_url.is_empty() { "✗ NOT configured" } else { "✓ configured" });
    println!("    Secret →  {}", if admin_secret.is_empty() { "✗ NOT configured" } else { "✓ configured" });
    println!("  {bar}\n");

    if database_url.is_empty() {
        eprintln!("  ✗  DATABASE_URL is required. Set it in .env");
        std::process::exit(1);
    }

    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&database_url)
        .await
        .context("Failed to connect to PostgreSQL")?;

    let state = AppState {
        pool,
        admin_secret,
        api_base,
        sessions: Arc::new(RwLock::new(HashSet::new())),
        http: Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?,
    };

    let addr     = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        println!("  Opening http://127.0.0.1:{port} in browser …");
        open_browser(&format!("http://127.0.0.1:{port}"));
    });

    println!("  Listening on http://127.0.0.1:{port}  (Ctrl+C to stop)\n");
    axum::serve(listener, build_router(state)).await?;
    Ok(())
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
}
