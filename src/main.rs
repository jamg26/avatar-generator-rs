use avagen::{
    config::AppConfig,
    db,
    generator::{ stablehorde::StableHordePipeline, video_pipeline::VideoPipeline },
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

    // ── Avatar pipeline (Stable Horde — free, community GPU, no key) ─────────────
    let pipeline = StableHordePipeline::new().unwrap_or_else(|e| {
        tracing::error!("Failed to create StableHorde pipeline: {e:#}");
        std::process::exit(1);
    });

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
