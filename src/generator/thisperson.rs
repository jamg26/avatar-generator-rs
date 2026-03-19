use anyhow::{Context, Result};
use image::DynamicImage;
use reqwest::Client;
use std::time::Duration;

use crate::generator::prompt::AvatarRequest;

/// Generates realistic 1024×1024 AI portrait faces via thispersondoesnotexist.com.
///
/// Completely free — no API key required. Each request returns a unique
/// StyleGAN2-generated face. Gender/ethnicity parameters from the request
/// are forwarded as prompt metadata but the underlying source is random;
/// the face appearance varies on each call. For deterministic output,
/// cache the returned image keyed on the seed in your own storage.
#[derive(Clone)]
pub struct ThisPersonPipeline {
    client: Client,
}

impl ThisPersonPipeline {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (compatible; avagen/0.1)")
            .build()
            .context("Failed to build HTTP client")?;
        tracing::info!("ThisPersonDoesNotExist pipeline ready (free, no key required)");
        Ok(Self { client })
    }

    /// Returns a 1024×1024 AI-generated face.
    pub async fn generate(
        &self,
        _req: &AvatarRequest,
        _size: usize,
        seed: u64,
    ) -> Result<DynamicImage> {
        // Use the seed as a cache-buster so different seeds reliably get
        // different images from the CDN.
        let url = format!("https://thispersondoesnotexist.com/?v={seed}");

        tracing::debug!("GET {url}");

        let bytes = self
            .client
            .get(&url)
            .send()
            .await
            .context("thispersondoesnotexist.com request failed")?
            .error_for_status()
            .context("thispersondoesnotexist.com returned error status")?
            .bytes()
            .await
            .context("Failed to read image bytes")?;

        let img = tokio::task::spawn_blocking(move || image::load_from_memory(&bytes))
            .await
            .map_err(|e| anyhow::anyhow!("image decode join: {e}"))?
            .context("Failed to decode image")?;

        tracing::debug!("Face image: {}×{}", img.width(), img.height());
        Ok(img)
    }
}
