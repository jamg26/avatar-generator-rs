use std::io::Cursor;

use axum::{ extract::State, http::header, response::{ IntoResponse, Response }, Extension, Json };

use crate::{ db, error::AppError, generator::prompt::AvatarRequest, routes::AppState };

/// POST /api/v1/avatar/generate
///
/// Accepts a JSON body describing the avatar and returns the generated image.
pub async fn generate(
    State(state): State<AppState>,
    Extension(key): Extension<db::ApiKeyRow>,
    Json(req): Json<AvatarRequest>
) -> Result<Response, AppError> {
    // Validate size and round to nearest multiple of 64 (required by FLUX for efficient inference)
    let size_raw = req.size.unwrap_or(state.config.sd_default_size);
    if !(128..=1500).contains(&size_raw) {
        return Err(AppError::BadRequest("size must be between 128 and 1500".into()));
    }
    let size = ((size_raw + 32) / 64) * 64;

    let prompt = req.to_prompt();
    let negative = req.negative_prompt().to_string();
    let num_steps = state.config.sd_num_steps;
    let guidance = state.config.sd_guidance_scale;
    let seed = req.seed.unwrap_or_else(|| rand::random::<u64>());
    let format = req.format;
    let pipeline = state.pipeline
        .clone()
        .ok_or_else(|| {
            AppError::ServiceUnavailable(
                "Avatar generation is not available: model not loaded".into()
            )
        })?;

    tracing::info!(key_prefix = %key.key_prefix, %prompt, "Generating avatar");

    // Call async HF Inference API — no spawn_blocking needed
    let img = pipeline
        .generate(&prompt, &negative, size, size, num_steps, guidance, seed).await
        .map_err(|e| AppError::Internal(format!("generation failed: {e}")))?;

    // Encode to requested format
    let (bytes, content_type) = encode_image(&img, format)?;

    // Record usage
    db::record_usage(&state.pool, &key.id, "/api/v1/avatar/generate").await?;

    Ok(([(header::CONTENT_TYPE, content_type)], bytes).into_response())
}

fn encode_image(
    img: &image::DynamicImage,
    format: crate::generator::prompt::ImageFormat
) -> Result<(Vec<u8>, &'static str), AppError> {
    let mut buf = Cursor::new(Vec::new());

    match format {
        crate::generator::prompt::ImageFormat::Png => {
            img
                .write_to(&mut buf, image::ImageFormat::Png)
                .map_err(|e| AppError::Internal(format!("png encode: {e}")))?;
            Ok((buf.into_inner(), "image/png"))
        }
        crate::generator::prompt::ImageFormat::Jpeg => {
            img
                .write_to(&mut buf, image::ImageFormat::Jpeg)
                .map_err(|e| AppError::Internal(format!("jpeg encode: {e}")))?;
            Ok((buf.into_inner(), "image/jpeg"))
        }
        crate::generator::prompt::ImageFormat::Webp => {
            img
                .write_to(&mut buf, image::ImageFormat::WebP)
                .map_err(|e| AppError::Internal(format!("webp encode: {e}")))?;
            Ok((buf.into_inner(), "image/webp"))
        }
    }
}
