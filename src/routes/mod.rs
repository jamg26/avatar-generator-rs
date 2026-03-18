pub mod avatar;
pub mod health;
pub mod keys;
pub mod usage;

use std::{ sync::Arc, time::Duration };

use axum::{ middleware, routing::{ delete, get, post }, Router };
use sqlx::PgPool;
use tower_http::{ cors::CorsLayer, trace::TraceLayer };

use crate::{
    config::AppConfig,
    generator::pipeline::SdPipeline,
    middleware::{ api_key, rate_limit },
};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: AppConfig,
    pub pipeline: Option<Arc<SdPipeline>>,
}

pub fn create_router(pool: PgPool, config: AppConfig, pipeline: Option<SdPipeline>) -> Router {
    let state = AppState {
        pool,
        config: config.clone(),
        pipeline: pipeline.map(Arc::new),
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
        .route("/api/v1/usage", get(usage::my_usage))
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
