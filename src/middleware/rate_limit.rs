use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{ Request, StatusCode },
    middleware::Next,
    response::{ IntoResponse, Response },
};
use governor::{ clock::DefaultClock, state::keyed::DashMapStateStore, Quota, RateLimiter };
use serde_json::json;
use std::{ net::{ IpAddr, SocketAddr }, num::NonZeroU32, sync::Arc };

// ── Per-IP keyed limiter ─────────────────────────────────────────────────────

pub type KeyedRateLimiter = Arc<RateLimiter<IpAddr, DashMapStateStore<IpAddr>, DefaultClock>>;

pub fn new_limiter(per_minute: u32) -> KeyedRateLimiter {
    let quota = Quota::per_minute(NonZeroU32::new(per_minute).expect("rate > 0"));
    Arc::new(RateLimiter::dashmap(quota))
}

pub async fn handle(limiter: KeyedRateLimiter, req: Request<Body>, next: Next) -> Response {
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]));

    match limiter.check_key(&ip) {
        Ok(_) => next.run(req).await,
        Err(_) => {
            let body = axum::Json(json!({ "error": "Rate limit exceeded" }));
            (StatusCode::TOO_MANY_REQUESTS, body).into_response()
        }
    }
}
