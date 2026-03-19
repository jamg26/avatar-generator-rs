use anyhow::{Context, Result};
use image::DynamicImage;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use crate::generator::prompt::{AvatarRequest, ShotType};

/// FLUX Schnell — 4-step model, excellent quality, ~1-2s on Replicate GPU.
const DEFAULT_MODEL: &str = "black-forest-labs/flux-schnell";
const REPLICATE_BASE: &str = "https://api.replicate.com/v1";

#[derive(Deserialize)]
struct Prediction {
    status: String,
    output: Option<Vec<String>>,
    error: Option<String>,
    urls: Option<PredictionUrls>,
}

#[derive(Deserialize)]
struct PredictionUrls {
    get: Option<String>,
}

/// Generates realistic face images via the Replicate API (FLUX Schnell).
/// Override model with `REPLICATE_MODEL` env var.
#[derive(Clone)]
pub struct ReplicatePipeline {
    client: Client,
    token: String,
    model: String,
}

impl ReplicatePipeline {
    pub fn new(token: String) -> Result<Self> {
        let model = std::env::var("REPLICATE_MODEL")
            .unwrap_or_else(|_| DEFAULT_MODEL.into());
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent("avagen/0.1")
            .build()
            .context("Failed to build HTTP client for Replicate")?;
        tracing::info!("Replicate pipeline ready (model: {model})");
        Ok(Self { client, token, model })
    }

    pub async fn generate(
        &self,
        req: &AvatarRequest,
        size: usize,
        seed: u64,
    ) -> Result<DynamicImage> {
        let prompt = req.to_prompt();

        // Clamp to FLUX-safe dimensions (multiples of 8, max 1440)
        let px = (size.min(1440) / 8 * 8).max(256) as u32;
        let (width, height) = match req.shot_type {
            ShotType::Headshot => (px, px),
            ShotType::Body     => (px, (px * 4 / 3 / 8 * 8).max(256)),
        };

        let body = serde_json::json!({
            "input": {
                "prompt": prompt,
                "width": width,
                "height": height,
                "seed": seed as i64,
                "num_outputs": 1,
                "num_inference_steps": 4,
                "output_format": "png",
                "output_quality": 100,
                "go_fast": true,
            }
        });

        tracing::debug!(
            "Replicate POST {REPLICATE_BASE}/models/{} prompt={:?}",
            self.model,
            &prompt[..60.min(prompt.len())]
        );

        // Create prediction — use `Prefer: wait` for synchronous response (up to 60s)
        let resp = self
            .client
            .post(format!("{REPLICATE_BASE}/models/{}/predictions", self.model))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Prefer", "wait")
            .json(&body)
            .send()
            .await
            .context("Replicate API request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let txt = resp.text().await.unwrap_or_default();
            anyhow::bail!("Replicate API returned {status}: {txt}");
        }

        let pred: Prediction = resp.json().await.context("Failed to parse Replicate prediction")?;

        // If prediction didn't finish synchronously, poll the get URL
        let output_url = if pred.status == "succeeded" {
            pred.output
                .and_then(|o| o.into_iter().next())
                .context("Replicate prediction succeeded but output is empty")?
        } else if pred.status == "failed" {
            anyhow::bail!("Replicate prediction failed: {}", pred.error.unwrap_or_default());
        } else {
            // Poll the get URL
            let get_url = pred
                .urls
                .and_then(|u| u.get)
                .context("No polling URL in Replicate response")?;
            self.poll_prediction(&get_url).await?
        };

        // Download the output image
        tracing::debug!("Downloading Replicate output: {output_url}");
        let img_bytes = self
            .client
            .get(&output_url)
            .send()
            .await
            .context("Failed to download Replicate output")?
            .bytes()
            .await
            .context("Failed to read Replicate image bytes")?;

        let img = tokio::task::spawn_blocking(move || image::load_from_memory(&img_bytes))
            .await
            .map_err(|e| anyhow::anyhow!("image decode join: {e}"))?
            .context("Failed to decode image from Replicate")?;

        tracing::debug!("Replicate returned {}×{} image", img.width(), img.height());
        Ok(img)
    }

    async fn poll_prediction(&self, url: &str) -> Result<String> {
        for attempt in 0..30 {
            tokio::time::sleep(Duration::from_millis(if attempt < 5 { 500 } else { 1000 })).await;

            let pred: Prediction = self
                .client
                .get(url)
                .header("Authorization", format!("Bearer {}", self.token))
                .send()
                .await
                .context("Poll request failed")?
                .json()
                .await
                .context("Failed to parse poll response")?;

            match pred.status.as_str() {
                "succeeded" => {
                    return pred
                        .output
                        .and_then(|o| o.into_iter().next())
                        .context("Prediction succeeded but output is empty");
                }
                "failed" | "canceled" => {
                    anyhow::bail!("Prediction {}: {}", pred.status, pred.error.unwrap_or_default());
                }
                _ => {} // processing | starting
            }
        }
        anyhow::bail!("Replicate prediction timed out after 30 polls")
    }
}
