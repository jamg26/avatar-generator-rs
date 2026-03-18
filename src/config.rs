#[derive(Clone)]
pub struct AppConfig {
    pub database_url: String,
    pub admin_secret: String,
    /// Base URL of the local inference sidecar (infer.py). Defaults to http://localhost:8001.
    pub infer_url: String,
    pub sd_model_repo: String,
    pub sd_num_steps: usize,
    pub sd_guidance_scale: f64,
    pub sd_default_size: usize,
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
            sd_model_repo: std::env
                ::var("SD_MODEL_REPO")
                .unwrap_or_else(|_| "runwayml/stable-diffusion-v1-5".into()),
            sd_num_steps: std::env
                ::var("SD_NUM_STEPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            sd_guidance_scale: std::env
                ::var("SD_GUIDANCE_SCALE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(7.5),
            sd_default_size: std::env
                ::var("SD_DEFAULT_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(512),
            video_model_repo: std::env
                ::var("VIDEO_MODEL_REPO")
                .unwrap_or_else(|_| "stabilityai/stable-video-diffusion-img2vid-xt".into()),
            rate_limit_per_minute: std::env
                ::var("RATE_LIMIT_PER_MINUTE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
        }
    }
}
