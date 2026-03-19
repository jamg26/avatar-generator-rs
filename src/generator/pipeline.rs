use anyhow::{ bail, Context, Result };
use image::DynamicImage;
use reqwest::Client;
use std::time::Duration;

/// Wrapper around the local inference sidecar for text-to-image generation.
///
/// The sidecar (`infer.py`) runs as a sibling process on port 8001 and
/// executes FLUX.1-schnell directly on the GPU — no HuggingFace API calls.
///
/// A single instance is shared across all requests via `Arc<SdPipeline>`.
#[derive(Clone)]
pub struct SdPipeline {
    infer_url: String,
    client: Client,
}

impl SdPipeline {
    /// Creates a pipeline that calls the local inference sidecar.
    /// `infer_url` is typically `http://localhost:8001`.
    pub fn load(model_repo: &str, infer_url: &str) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .context("Failed to build HTTP client")?;

        tracing::info!(
            "Local inference pipeline ready (model: {model_repo}, sidecar: {infer_url})"
        );

        Ok(Self {
            infer_url: infer_url.to_string(),
            client,
        })
    }

    /// Generates an image via the local inference sidecar and returns a `DynamicImage`.
    pub async fn generate(
        &self,
        prompt: &str,
        negative_prompt: &str,
        width: usize,
        height: usize,
        num_steps: usize,
        guidance_scale: f64,
        seed: u64
    ) -> Result<DynamicImage> {
        let url = format!("{}/generate", self.infer_url);

        let body =
            serde_json::json!({
            "prompt":              prompt,
            "negative_prompt":    negative_prompt,
            "width":              width,
            "height":             height,
            "num_inference_steps": num_steps,
            "guidance_scale":     guidance_scale,
            "seed":               seed,
        });

        tracing::debug!("Calling local sidecar: POST {url} ({width}x{height}, {num_steps} steps)");

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
                    anyhow::anyhow!("Inference request failed: {e}")
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

        let bytes = response.bytes().await.context("Failed to read image bytes from sidecar")?;
        let img = tokio::task
            ::spawn_blocking(move || image::load_from_memory(&bytes)).await
            .map_err(|e| anyhow::anyhow!("image decode join: {e}"))?
            .context("Failed to decode PNG returned by inference server")?;

        tracing::debug!("Inference server returned image: {}x{}", img.width(), img.height());
        Ok(img)
    }
}
