pub mod avatar;
pub mod batch;
pub mod health;
pub mod keys;
pub mod usage;
pub mod video;

use std::{ sync::Arc, time::Duration };

use axum::{ middleware, routing::{ delete, get, post }, Router };
use sqlx::PgPool;
use tower_http::{ cors::CorsLayer, trace::TraceLayer };

use crate::{
    config::AppConfig,
    generator::{
        bulk::BulkPipeline,
        stablehorde::StableHordePipeline,
        video_pipeline::VideoPipeline,
    },
    middleware::{ api_key, rate_limit },
};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: AppConfig,
    pub pipeline: Arc<StableHordePipeline>,
    pub bulk_pipeline: Arc<BulkPipeline>,
    pub video_pipeline: Option<Arc<VideoPipeline>>,
}

pub fn create_router(
    pool: PgPool,
    config: AppConfig,
    pipeline: StableHordePipeline,
    bulk_pipeline: BulkPipeline,
    video_pipeline: Option<VideoPipeline>,
) -> Router {
    let state = AppState {
        pool,
        config: config.clone(),
        pipeline: Arc::new(pipeline),
        bulk_pipeline: Arc::new(bulk_pipeline),
        video_pipeline: video_pipeline.map(Arc::new),
    };

    let limiter = rate_limit::new_limiter(config.rate_limit_per_minute);

    // Background cleanup for stale rate-limit entries
    let cleanup = limiter.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            cleanup.retain_recent();
        }
    });

    // Routes that require an API key
    let authed = Router::new()
        .route("/api/v1/avatar/generate", post(avatar::generate))
        .route("/api/v1/video/generate", post(video::generate))
        .route("/api/v1/usage", get(usage::my_usage))
        .route("/api/v1/batch/generate", post(batch::submit))
        .route("/api/v1/batch/{job_id}", get(batch::get_job))
        .route("/api/v1/batch", get(batch::list_jobs))
        .layer(middleware::from_fn_with_state(state.clone(), api_key::require_api_key));

    // Admin routes (protected by admin secret in the handler)
    let admin = Router::new()
        .route("/api/admin/keys", post(keys::create_key))
        .route("/api/admin/keys", get(keys::list_keys))
        .route("/api/admin/keys/{id}", delete(keys::revoke_key));

    // Public routes
    let public = Router::new().route("/", get(health::home)).route("/health", get(health::handle));

    Router::new()
        .merge(authed)
        .merge(admin)
        .merge(public)
        .layer(
            middleware::from_fn(move |req, next| {
                let lim = limiter.clone();
                rate_limit::handle(lim, req, next)
            })
        )
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}
