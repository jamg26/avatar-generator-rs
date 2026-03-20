use std::io::Cursor;

use axum::{ extract::State, http::header, response::{ IntoResponse, Response }, Extension, Json };

use crate::{ db, error::AppError, generator::prompt::{ AvatarRequest, ShotType }, routes::AppState };

/// POST /api/v1/avatar/generate
///
/// Accepts a JSON body describing the avatar and returns the generated image.
pub async fn generate(
    State(state): State<AppState>,
    Extension(key): Extension<db::ApiKeyRow>,
    Json(req): Json<AvatarRequest>
) -> Result<Response, AppError> {
    // Validate and round size to nearest multiple of 64
    let size_raw = req.size.unwrap_or(state.config.default_size);
    if !(128..=1500).contains(&size_raw) {
        return Err(AppError::BadRequest("size must be between 128 and 1500".into()));
    }
    let size = ((size_raw + 32) / 64) * 64;

    // Body shots use non-square aspect ratios; headshots stay square.
    let (width, height) = match req.shot_type {
        ShotType::Headshot => (size, size),
        ShotType::Portrait => {
            // 3:4 ratio — standard portrait (waist-up)
            let h = ((size_raw * 4 / 3) + 32) / 64 * 64;
            (size, h)
        }
        ShotType::FullBody => {
            // 2:3 ratio — full body, head to toe
            let h = ((size_raw * 3 / 2) + 32) / 64 * 64;
            (size, h)
        }
        ShotType::Landscape => {
            // 3:2 ratio — wide landscape orientation
            let w = ((size_raw * 3 / 2) + 32) / 64 * 64;
            (w, size)
        }
    };

    let seed = req.seed.unwrap_or_else(|| rand::random::<u64>());
    let format = req.format;

    tracing::info!(key_prefix = %key.key_prefix, ?req.sex, ?req.ethnicity, "Generating avatar");

    let img = state.pipeline
        .generate(&req, width, height, seed).await
        .map_err(|e| AppError::Internal(format!("generation failed: {e}")))?;

    // Resize to requested (width × height) if width ≠ height (body shot)
    let img = if width != height {
        img.resize_exact(width as u32, height as u32, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    let (bytes, content_type) = encode_image(&img, format)?;

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
