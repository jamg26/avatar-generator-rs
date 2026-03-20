use chrono::{ DateTime, NaiveDate, Utc };
use sqlx::postgres::{ PgPool, PgPoolOptions };

// ── Schema initialisation ────────────────────────────────────────────────────

pub async fn init_pool(database_url: &str) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url).await
        .expect("Failed to connect to PostgreSQL");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS api_keys (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            key_hash      TEXT NOT NULL UNIQUE,
            key_prefix    TEXT NOT NULL,
            monthly_quota BIGINT NOT NULL DEFAULT 500,
            is_active     BOOLEAN NOT NULL DEFAULT TRUE,
            created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )"
    )
        .execute(&pool).await
        .expect("Failed to create api_keys table");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS usage_log (
            id          BIGSERIAL PRIMARY KEY,
            api_key_id  TEXT NOT NULL REFERENCES api_keys(id),
            endpoint    TEXT NOT NULL,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )"
    )
        .execute(&pool).await
        .expect("Failed to create usage_log table");

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_usage_key_date
         ON usage_log(api_key_id, created_at)"
    )
        .execute(&pool).await
        .expect("Failed to create usage index");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS batch_jobs (
            id           UUID PRIMARY KEY,
            state        TEXT NOT NULL DEFAULT 'queued',
            total        BIGINT NOT NULL,
            completed    BIGINT NOT NULL DEFAULT 0,
            failed_count BIGINT NOT NULL DEFAULT 0,
            model        TEXT NOT NULL DEFAULT 'flux',
            download_url TEXT,
            error_msg    TEXT,
            created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )"
    )
        .execute(&pool).await
        .expect("Failed to create batch_jobs table");

    pool
}

// ── API key operations ───────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct ApiKeyRow {
    pub id: String,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub monthly_quota: i64,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn insert_api_key(
    pool: &PgPool,
    id: &str,
    name: &str,
    key_hash: &str,
    key_prefix: &str,
    monthly_quota: i64
) -> Result<(), sqlx::Error> {
    sqlx
        ::query(
            "INSERT INTO api_keys (id, name, key_hash, key_prefix, monthly_quota)
         VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(id)
        .bind(name)
        .bind(key_hash)
        .bind(key_prefix)
        .bind(monthly_quota)
        .execute(pool).await?;
    Ok(())
}

pub async fn find_api_key_by_hash(
    pool: &PgPool,
    key_hash: &str
) -> Result<Option<ApiKeyRow>, sqlx::Error> {
    sqlx
        ::query_as::<_, ApiKeyRow>(
            "SELECT * FROM api_keys WHERE key_hash = $1 AND is_active = TRUE"
        )
        .bind(key_hash)
        .fetch_optional(pool).await
}

pub async fn revoke_api_key(pool: &PgPool, id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx
        ::query("UPDATE api_keys SET is_active = FALSE, updated_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(pool).await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_api_keys(pool: &PgPool) -> Result<Vec<ApiKeyRow>, sqlx::Error> {
    sqlx
        ::query_as::<_, ApiKeyRow>("SELECT * FROM api_keys ORDER BY created_at DESC")
        .fetch_all(pool).await
}

// ── Usage tracking ───────────────────────────────────────────────────────────

pub async fn record_usage(
    pool: &PgPool,
    api_key_id: &str,
    endpoint: &str
) -> Result<(), sqlx::Error> {
    sqlx
        ::query("INSERT INTO usage_log (api_key_id, endpoint) VALUES ($1, $2)")
        .bind(api_key_id)
        .bind(endpoint)
        .execute(pool).await?;
    Ok(())
}

/// Returns the number of requests made by this key in the current calendar month.
pub async fn monthly_usage_count(pool: &PgPool, api_key_id: &str) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx
        ::query_as(
            "SELECT COUNT(*) FROM usage_log
         WHERE api_key_id = $1
           AND created_at >= DATE_TRUNC('month', NOW())"
        )
        .bind(api_key_id)
        .fetch_one(pool).await?;
    Ok(row.0)
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct DailyUsage {
    pub day: NaiveDate,
    pub count: i64,
}

pub async fn daily_usage(
    pool: &PgPool,
    api_key_id: &str,
    days: i64
) -> Result<Vec<DailyUsage>, sqlx::Error> {
    sqlx
        ::query_as::<_, DailyUsage>(
            "SELECT created_at::date AS day, COUNT(*) AS count
         FROM usage_log
         WHERE api_key_id = $1
           AND created_at >= NOW() - ($2::bigint * INTERVAL '1 day')
         GROUP BY day
         ORDER BY day"
        )
        .bind(api_key_id)
        .bind(days)
        .fetch_all(pool).await
}
// ── Batch job tracking ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct BatchJobRow {
    pub id:           uuid::Uuid,
    pub state:        String,
    pub total:        i64,
    pub completed:    i64,
    pub failed_count: i64,
    pub model:        String,
    pub download_url: Option<String>,
    pub error_msg:    Option<String>,
    pub created_at:   DateTime<Utc>,
    pub updated_at:   DateTime<Utc>,
}

pub async fn create_batch_job(
    pool: &PgPool,
    id:    uuid::Uuid,
    total: i64,
    model: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO batch_jobs (id, total, model, state)
         VALUES ($1, $2, $3, 'queued')"
    )
    .bind(id)
    .bind(total)
    .bind(model)
    .execute(pool).await?;
    Ok(())
}

pub async fn update_batch_job_progress(
    pool:         &PgPool,
    id:           uuid::Uuid,
    completed:    i64,
    failed_count: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE batch_jobs
         SET completed = $2, failed_count = $3, updated_at = NOW()
         WHERE id = $1"
    )
    .bind(id)
    .bind(completed)
    .bind(failed_count)
    .execute(pool).await?;
    Ok(())
}

pub async fn update_batch_job_state(
    pool:  &PgPool,
    id:    uuid::Uuid,
    state: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE batch_jobs SET state = $2, updated_at = NOW() WHERE id = $1"
    )
    .bind(id)
    .bind(state)
    .execute(pool).await?;
    Ok(())
}

pub async fn complete_batch_job(
    pool:         &PgPool,
    id:           uuid::Uuid,
    download_url: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE batch_jobs
         SET state = 'done', download_url = $2, updated_at = NOW()
         WHERE id = $1"
    )
    .bind(id)
    .bind(download_url)
    .execute(pool).await?;
    Ok(())
}

pub async fn fail_batch_job(
    pool:  &PgPool,
    id:    uuid::Uuid,
    error: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE batch_jobs
         SET state = 'failed', error_msg = $2, updated_at = NOW()
         WHERE id = $1"
    )
    .bind(id)
    .bind(error)
    .execute(pool).await?;
    Ok(())
}

pub async fn get_batch_job(
    pool: &PgPool,
    id:   uuid::Uuid,
) -> Result<Option<BatchJobRow>, sqlx::Error> {
    sqlx::query_as::<_, BatchJobRow>("SELECT * FROM batch_jobs WHERE id = $1")
        .bind(id)
        .fetch_optional(pool).await
}

pub async fn list_batch_jobs(pool: &PgPool) -> Result<Vec<BatchJobRow>, sqlx::Error> {
    sqlx::query_as::<_, BatchJobRow>(
        "SELECT * FROM batch_jobs ORDER BY created_at DESC"
    )
    .fetch_all(pool).await
}