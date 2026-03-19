use anyhow::{Context, Result};
use image::DynamicImage;
use reqwest::Client;
use std::time::Duration;

use crate::generator::prompt::{AvatarRequest, ShotType};

/// Default model — good for realistic portrait photography.
/// Override via `HF_IMAGE_MODEL` env var.
const DEFAULT_MODEL: &str = "SG161222/Realistic_Vision_V5.1_noVAE";
const HF_BASE: &str = "https://api-inference.huggingface.co/models";

/// Generates realistic face images via the Hugging Face Inference API.
/// Uses the existing `to_prompt()` / `negative_prompt()` infrastructure.
#[derive(Clone)]
pub struct HfInferencePipeline {
    client: Client,
    hf_token: String,
    model: String,
}

impl HfInferencePipeline {
    pub fn new(hf_token: String) -> Result<Self> {
        let model = std::env::var("HF_IMAGE_MODEL")
            .unwrap_or_else(|_| DEFAULT_MODEL.into());
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent("avagen/0.1")
            .build()
            .context("Failed to build HTTP client for HF Inference")?;
        tracing::info!("HF Inference pipeline ready (model: {model})");
        Ok(Self { client, hf_token, model })
    }

    /// Calls the HF Inference API and returns the decoded image.
    pub async fn generate(
        &self,
        req: &AvatarRequest,
        size: usize,
        seed: u64,
    ) -> Result<DynamicImage> {
        let prompt = req.to_prompt();
        let neg = req.negative_prompt();

        // Clamp to model-safe dimensions (multiples of 8, max 1024)
        let px = (size.min(1024) / 8 * 8).max(256) as u32;
        let (width, height) = match req.shot_type {
            ShotType::Headshot => (px, px),
            ShotType::Body     => (px, (px * 4 / 3 / 8 * 8).max(256)),
        };

        let body = serde_json::json!({
            "inputs": prompt,
            "parameters": {
                "negative_prompt": neg,
                "width":  width,
                "height": height,
                "seed":   seed as i64,
                "num_inference_steps": 28,
                "guidance_scale": 7.0,
            },
            "options": {
                "wait_for_model": true,
                "use_cache": false,
            }
        });

        tracing::debug!("HF Inference POST {}/{} prompt={:?}", HF_BASE, self.model, &prompt[..50.min(prompt.len())]);

        let response = self
            .client
            .post(format!("{}/{}", HF_BASE, self.model))
            .header("Authorization", format!("Bearer {}", self.hf_token))
            .json(&body)
            .send()
            .await
            .context("HF Inference API request failed")?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            anyhow::bail!("HF Inference API returned {status}: {body_text}");
        }

        let bytes = response.bytes().await.context("Failed to read HF Inference response")?;

        let img = tokio::task::spawn_blocking(move || image::load_from_memory(&bytes))
            .await
            .map_err(|e| anyhow::anyhow!("image decode join: {e}"))?
            .context("Failed to decode image from HF Inference")?;

        tracing::debug!("HF Inference returned {}×{} image", img.width(), img.height());
        Ok(img)
    }
}
