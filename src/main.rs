use avagen::{
    config::AppConfig,
    db,
    generator::{ pipeline::SdPipeline, video_pipeline::VideoPipeline },
    routes::create_router,
};
use std::net::SocketAddr;
use tracing_subscriber::{ layer::SubscriberExt, util::SubscriberInitExt };

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    // ── Logging ──────────────────────────────────────────────────────────────
    tracing_subscriber
        ::registry()
        .with(
            tracing_subscriber::EnvFilter
                ::try_from_default_env()
                .unwrap_or_else(|_| "avagen=debug,tower_http=debug".into())
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // ── Config ───────────────────────────────────────────────────────────────
    let config = AppConfig::from_env();

    // ── Database (PostgreSQL / NeonDB) ─────────────────────────────────────
    let pool = db::init_pool(&config.database_url).await;
    tracing::info!("PostgreSQL database ready");

    // ── Stable Diffusion pipeline ────────────────────────────────────────────
    let skip_sd = std::env
        ::var("SKIP_SD_PIPELINE")
        .map(|v| (v == "1" || v == "true"))
        .unwrap_or(false);
    let pipeline: Option<SdPipeline> = if skip_sd {
        tracing::warn!("SKIP_SD_PIPELINE set — avatar generation disabled");
        None
    } else {
        match SdPipeline::load(&config.sd_model_repo, &config.infer_url) {
            Ok(p) => {
                tracing::info!("Stable Diffusion pipeline ready");
                Some(p)
            }
            Err(e) => {
                tracing::error!("Failed to load SD pipeline: {e:#} — avatar generation disabled");
                None
            }
        }
    };

    // ── Video pipeline ───────────────────────────────────────────────────────
    let skip_video = std::env
        ::var("SKIP_VIDEO_PIPELINE")
        .map(|v| (v == "1" || v == "true"))
        .unwrap_or(false);
    let video_pipeline: Option<VideoPipeline> = if skip_video {
        tracing::warn!("SKIP_VIDEO_PIPELINE set — video generation disabled");
        None
    } else {
        match VideoPipeline::load(&config.video_model_repo, &config.infer_url) {
            Ok(p) => {
                tracing::info!("Video pipeline ready");
                Some(p)
            }
            Err(e) => {
                tracing::error!("Failed to load video pipeline: {e:#} — video generation disabled");
                None
            }
        }
    };

    // ── HTTP server ──────────────────────────────────────────────────────────
    let router = create_router(pool, config, pipeline, video_pipeline);

    let port: u16 = std::env
        ::var("PORT")
        .unwrap_or_else(|_| "8080".into())
        .parse()
        .expect("PORT must be a number");

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await.expect("Failed to bind");

    tracing::info!("AvaGen listening on {addr}");
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.expect(
        "Server error"
    );
}
