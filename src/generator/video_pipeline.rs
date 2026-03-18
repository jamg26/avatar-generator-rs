use anyhow::{ bail, Context, Result };
use reqwest::Client;
use std::time::Duration;

/// Wrapper around the local inference sidecar for image-to-video generation.
///
/// The sidecar (`infer.py`) runs SVD XT (Stable Video Diffusion XT) directly
/// on the GPU — no HuggingFace API calls.
///
/// A single instance is shared across all requests via `Arc<VideoPipeline>`.
#[derive(Clone)]
pub struct VideoPipeline {
    infer_url: String,
    client: Client,
}

impl VideoPipeline {
    pub fn load(model_repo: &str, infer_url: &str) -> Result<Self> {
        // Video inference is slower than image — give it up to 10 min.
        let client = Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .context("Failed to build HTTP client for video pipeline")?;

        tracing::info!("Local video pipeline ready (model: {model_repo}, sidecar: {infer_url})");

        Ok(Self {
            infer_url: infer_url.to_string(),
            client,
        })
    }

    /// Animate a face image into a short MP4 clip using SVD XT.
    ///
    /// Provide either `image_b64` (raw base64 JPEG/PNG) or `image_url` (public URL).
    /// - `motion_bucket_id`: 0–255 (40 = subtle, 127 = natural, 210 = expressive)
    /// - `noise_aug_strength`: 0.0–1.0 (SVD default is 0.02)
    /// - `fps_id`: target FPS (3 = cinematic, 6 = smooth, 8 = fluid)
    /// - `seed`: for reproducibility
    ///
    /// Returns raw MP4 bytes.
    pub async fn generate(
        &self,
        image_b64: Option<&str>,
        image_url: Option<&str>,
        motion_bucket_id: u32,
        noise_aug_strength: f32,
        fps_id: u32,
        seed: u64
    ) -> Result<Vec<u8>> {
        let url = format!("{}/video/generate", self.infer_url);

        let mut body =
            serde_json::json!({
            "motion_bucket_id":   motion_bucket_id,
            "noise_aug_strength": noise_aug_strength,
            "fps_id":             fps_id,
            "seed":               seed,
        });

        match (image_b64, image_url) {
            (Some(b64), _) => {
                body["image_b64"] = serde_json::json!(b64);
            }
            (None, Some(u)) => {
                body["image_url"] = serde_json::json!(u);
            }
            (None, None) => bail!("Provide image_b64 or image_url"),
        }

        tracing::debug!(
            "Calling local sidecar video: POST {url} \
             (motion={motion_bucket_id}, fps={fps_id}, seed={seed})"
        );

        let response = self.client
            .post(&url)
            .json(&body)
            .send().await
            .map_err(|e| {
                if e.is_connect() {
                    anyhow::anyhow!(
                        "Inference server not reachable (still loading models) — retry in ~60 s"
                    )
                } else {
                    anyhow::anyhow!("Video inference request failed: {e}")
                }
            })?;

        let status = response.status();

        if status == 503 {
            let body_text = response.text().await.unwrap_or_default();
            bail!("Inference server returned 503 (model loading): {}", body_text);
        }

        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            bail!("Inference server returned {}: {}", status, body_text);
        }

        let video_bytes = response
            .bytes().await
            .context("Failed to read video bytes from inference server")?;

        if video_bytes.is_empty() {
            bail!("Inference server returned empty video");
        }

        Ok(video_bytes.to_vec())
    }
}
