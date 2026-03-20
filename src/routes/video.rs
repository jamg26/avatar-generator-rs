use axum::{ extract::State, http::header, response::{ IntoResponse, Response }, Extension, Json };
use serde::Deserialize;

use crate::{ db, error::AppError, routes::AppState };

// ── Request types ─────────────────────────────────────────────────────────────

/// How much movement SVD adds to the animated face.
#[derive(Debug, Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum MotionIntensity {
    /// Subtle head-sway and micro-expressions — best for formal auditions.
    Subtle,
    /// Natural breathing, slight head movement — recommended default.
    #[default]
    Natural,
    /// Expressive gestures and more pronounced motion — best for dramatic scenes.
    Expressive,
}

impl MotionIntensity {
    /// Maps intensity to SVD `motion_bucket_id` (0–255).
    fn motion_bucket_id(self) -> u32 {
        match self {
            Self::Subtle => 40,
            Self::Natural => 127,
            Self::Expressive => 210,
        }
    }

    /// Maps intensity to `noise_aug_strength`.
    /// Higher = more variety / less faithful to the input face.
    fn noise_aug_strength(self) -> f32 {
        match self {
            Self::Subtle => 0.01,
            Self::Natural => 0.02,
            Self::Expressive => 0.05,
        }
    }
}

/// Output frame-rate preset.
#[derive(Debug, Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum FrameRate {
    /// 3 fps — cinematic, dream-like feel.
    Cinematic,
    /// 6 fps — smooth, natural motion (SVD default). Recommended.
    #[default]
    Smooth,
    /// 8 fps — higher fluidity for energetic scenes.
    Fluid,
}

impl FrameRate {
    fn fps_id(self) -> u32 {
        match self {
            Self::Cinematic => 3,
            Self::Smooth => 6,
            Self::Fluid => 8,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct VideoRequest {
    /// Base64-encoded face image — JPEG or PNG, ideally a forward-facing
    /// close-up or headshot. Max ~2 MB encoded.
    ///
    /// Provide either this or `image_url` (not both).
    pub face_image: Option<String>,

    /// Publicly accessible URL of the face image.
    ///
    /// The URL is forwarded directly to the HuggingFace Inference API —
    /// it must be reachable from the public internet.
    /// Provide either this or `face_image` (not both).
    pub image_url: Option<String>,

    /// Controls how much motion SVD adds to the face.
    /// subtle | natural (default) | expressive
    #[serde(default)]
    pub motion_intensity: MotionIntensity,

    /// Output frame-rate preset.
    /// cinematic | smooth (default) | fluid
    #[serde(default)]
    pub frame_rate: FrameRate,

    /// Optional seed for reproducible output.
    pub seed: Option<u64>,
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// POST /api/v1/video/generate
///
/// Animates a face image into a short (~4 s) MP4 audition clip using
/// Stable Video Diffusion (SVD XT) via the HuggingFace Inference API.
///
/// Returns raw `video/mp4` bytes.
pub async fn generate(
    State(state): State<AppState>,
    Extension(key): Extension<db::ApiKeyRow>,
    Json(req): Json<VideoRequest>
) -> Result<Response, AppError> {
    // ── Resolve image input ────────────────────────────────────────────────
    let (face_b64, img_url): (Option<&str>, Option<&str>) = match
        (req.face_image.as_deref(), req.image_url.as_deref())
    {
        (Some(b64), _) => {
            // Guard against extremely large payloads (2 MB base64 ≈ 1.5 MB raw)
            if b64.len() > 2_800_000 {
                return Err(
                    AppError::BadRequest(
                        "face_image must not exceed 2 MB when base64-encoded".into()
                    )
                );
            }
            (Some(b64), None)
        }
        (None, Some(url)) => {
            if !url.starts_with("https://") && !url.starts_with("http://") {
                return Err(AppError::BadRequest("image_url must be an http/https URL".into()));
            }
            (None, Some(url))
        }
        (None, None) => {
            return Err(
                AppError::BadRequest("provide either face_image (base64) or image_url".into())
            );
        }
    };

    let pipeline = state.video_pipeline
        .clone()
        .ok_or_else(|| {
            AppError::ServiceUnavailable(
                "Video generation is not available: pipeline not loaded".into()
            )
        })?;

    let motion_bucket_id = req.motion_intensity.motion_bucket_id();
    let noise_aug = req.motion_intensity.noise_aug_strength();
    let fps_id = req.frame_rate.fps_id();
    let seed = req.seed.unwrap_or_else(|| rand::random::<u64>());

    tracing::info!(
        key_prefix = %key.key_prefix,
        motion_bucket_id,
        fps_id,
        "Generating audition video"
    );

    let video_bytes = pipeline
        .generate(face_b64, img_url, motion_bucket_id, noise_aug, fps_id, seed).await
        .map_err(|e| AppError::Internal(format!("video generation failed: {e}")))?;

    db::record_usage(&state.pool, &key.id, "/api/v1/video/generate", 1).await?;

    Ok(
        (
            [
                (header::CONTENT_TYPE, "video/mp4"),
                (header::CONTENT_DISPOSITION, "attachment; filename=\"audition.mp4\""),
            ],
            video_bytes,
        ).into_response()
    )
}
