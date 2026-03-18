use anyhow::{ bail, Context, Result };
use image::DynamicImage;
use reqwest::Client;
use std::time::Duration;

/// Wraps the HuggingFace Inference API for text-to-image generation.
/// A single instance is shared across all requests via `Arc<SdPipeline>`.
#[derive(Clone)]
pub struct SdPipeline {
    hf_token: String,
    model_repo: String,
    client: Client,
}

impl SdPipeline {
    /// Creates a pipeline that delegates generation to the HF Inference API.
    /// No model weights are downloaded locally — inference runs on HF's GPU.
    pub fn load(model_repo: &str, hf_token: &str) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .context("Failed to build HTTP client")?;

        tracing::info!("HF Inference API pipeline ready (model: {model_repo})");

        Ok(Self {
            hf_token: hf_token.to_string(),
            model_repo: model_repo.to_string(),
            client,
        })
    }

    /// Generates an image via the HF Inference API and returns a `DynamicImage`.
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
        let url = format!("https://router.huggingface.co/hf-inference/models/{}", self.model_repo);

        let body =
            serde_json::json!({
            "inputs": prompt,
            "parameters": {
                "negative_prompt": negative_prompt,
                "width":  width,
                "height": height,
                "num_inference_steps": num_steps,
                "guidance_scale": guidance_scale,
                "seed": seed,
            }
        });

        tracing::debug!(
            "Calling HF Inference API: POST {url} ({width}x{height}, {num_steps} steps)"
        );

        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.hf_token))
            .json(&body)
            .send().await
            .context("HF Inference API request failed")?;

        let status = response.status();

        if status == 503 {
            let body_text = response.text().await.unwrap_or_default();
            bail!("HF model is loading or unavailable (503). Try again in a moment. Details: {}", body_text);
        }

        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            bail!("HF Inference API returned {}: {}", status, body_text);
        }

        let bytes = response.bytes().await.context("Failed to read image bytes from HF API")?;
        let img = tokio::task
            ::spawn_blocking(move || { image::load_from_memory(&bytes) }).await
            .map_err(|e| anyhow::anyhow!("image decode join: {e}"))?
            .context("Failed to decode image returned by HF Inference API")?;

        tracing::debug!("HF Inference API returned image: {}x{}", img.width(), img.height());
        Ok(img)
    }
}
