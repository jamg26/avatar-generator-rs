#[derive(Clone)]
pub struct AppConfig {
    pub database_url: String,
    pub admin_secret: String,
    /// Directory where bulk-generated images are saved. Defaults to `./avatars`.
    pub save_dir: std::path::PathBuf,

    /// Base URL of the local inference sidecar (used by the video pipeline). Defaults to http://localhost:8001.
    pub infer_url: String,
    /// Default output size in pixels (128–1500). Defaults to 256.
    pub default_size: usize,
    pub video_model_repo: String,
    pub rate_limit_per_minute: u32,
}

impl AppConfig {
    pub fn from_env() -> Self {
        Self {
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            admin_secret: std::env::var("ADMIN_SECRET").expect("ADMIN_SECRET must be set"),

            infer_url: std::env
                ::var("INFER_URL")
                .unwrap_or_else(|_| "http://localhost:8001".into()),
            default_size: std::env
                ::var("DEFAULT_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(256),
            video_model_repo: std::env
                ::var("VIDEO_MODEL_REPO")
                .unwrap_or_else(|_| "stabilityai/stable-video-diffusion-img2vid-xt".into()),
            rate_limit_per_minute: std::env
                ::var("RATE_LIMIT_PER_MINUTE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            save_dir: std::env::var("SAVE_DIR")
                .unwrap_or_else(|_| "./avatars".into())
                .into(),
        }
    }
}
