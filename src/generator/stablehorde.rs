use anyhow::{anyhow, Result};
use base64::Engine;
use image::DynamicImage;
use reqwest::{header, Client};
use serde::Deserialize;
use tokio::time::{sleep, Duration, Instant};

use super::prompt::AvatarRequest;

const HORDE_API: &str = "https://stablehorde.net/api/v2";
/// Used when STABLE_HORDE_KEY env var is not set. Registered free accounts get higher priority.
const ANON_KEY: &str = "0000000000";
/// Correct model name on Stable Horde (case-sensitive)
const DEFAULT_MODEL: &str = "Flux.1-Schnell fp8 (Compact)";

pub struct StableHordePipeline {
    client: Client,
}

#[derive(Deserialize)]
struct AsyncResponse {
    id: String,
}

#[derive(Deserialize)]
struct CheckResponse {
    done: bool,
    faulted: Option<bool>,
}

#[derive(Deserialize)]
struct StatusResponse {
    generations: Vec<Generation>,
}

#[derive(Deserialize)]
struct Generation {
    img: String,
    #[serde(default)]
    censored: bool,
}

impl StableHordePipeline {
    pub fn new() -> Result<Self> {
        let api_key = std::env::var("STABLE_HORDE_KEY")
            .unwrap_or_else(|_| ANON_KEY.to_string());

        if api_key == ANON_KEY {
            tracing::warn!(
                "STABLE_HORDE_KEY not set — using anonymous key (lower queue priority). \
                 Register for free at stablehorde.net for faster generation."
            );
        }

        let mut headers = header::HeaderMap::new();
        headers.insert(
            "apikey",
            header::HeaderValue::from_str(&api_key)
                .map_err(|e| anyhow!("Invalid STABLE_HORDE_KEY: {e}"))?,
        );
        let client = Client::builder()
            .default_headers(headers)
            .user_agent("avagen/1.0")
            .timeout(Duration::from_secs(600))
            .build()?;
        Ok(Self { client })
    }

    pub async fn generate(
        &self,
        req: &AvatarRequest,
        width: usize,
        height: usize,
        seed: u64,
    ) -> Result<DynamicImage> {
        let prompt = req.to_prompt();
        // FLUX.1-Schnell ignores negative prompts
        let full_prompt = prompt;

        // Round to nearest 64, enforce minimum 512 for FLUX quality
        let dim_w = (((width as u32 + 32) / 64) * 64).max(512);
        let dim_h = (((height as u32 + 32) / 64) * 64).max(512);

        // FLUX.1-Schnell optimal parameters
        let (steps, sampler, cfg) = (4u32, "k_euler", 1.0f64);
        // Always FLUX — no fallback model
        let models = vec![DEFAULT_MODEL];

        // Retry up to 3 attempts total (fault/censored/timeout can be transient)
        let mut last_err = anyhow!("generation failed");
        let mut current_seed = seed;
        for attempt in 0..3u32 {
            if attempt > 0 {
                tracing::warn!("Stable Horde attempt {attempt} after: {last_err}");
                sleep(Duration::from_secs(5)).await;
                current_seed = rand::random();
            }
            match self.try_generate(&full_prompt, dim_w, dim_h, steps, sampler, cfg, &models, current_seed).await {
                Ok(img) => return Ok(img),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    async fn try_generate(
        &self,
        full_prompt: &str,
        dim_w: u32,
        dim_h: u32,
        steps: u32,
        sampler: &str,
        cfg: f64,
        models: &[&str],
        seed: u64,
    ) -> Result<DynamicImage> {
        let resp = self
            .client
            .post(format!("{HORDE_API}/generate/async"))
            .json(&serde_json::json!({
                "prompt": full_prompt,
                "params": {
                    "steps": steps,
                    "width": dim_w,
                    "height": dim_h,
                    "n": 1,
                    "sampler_name": sampler,
                    "cfg_scale": cfg,
                    "seed": seed.to_string(),
                },
                "nsfw": false,
                "censor_nsfw": true,
                "slow_workers": true,
                "models": models,
                "r2": true,
            }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Stable Horde submit failed {code}: {body}"));
        }

        let job: AsyncResponse = resp.json().await?;
        let job_id = job.id;

        // Poll /check until done (timeout 900s — matches bulk pipeline; FLUX queues can be long)
        let deadline = Instant::now() + Duration::from_secs(900);
        // First check after 3s — jobs often complete in 3-10s on fast workers
        sleep(Duration::from_secs(3)).await;
        loop {
            let check: CheckResponse = self
                .client
                .get(format!("{HORDE_API}/generate/check/{job_id}"))
                .send()
                .await?
                .json()
                .await?;

            if check.faulted.unwrap_or(false) {
                return Err(anyhow!("Stable Horde generation faulted"));
            }
            if check.done {
                break;
            }
            if Instant::now() > deadline {
                return Err(anyhow!("Stable Horde generation timed out after 900s"));
            }
            sleep(Duration::from_secs(3)).await;
        }

        // Retrieve the result
        let status: StatusResponse = self
            .client
            .get(format!("{HORDE_API}/generate/status/{job_id}"))
            .send()
            .await?
            .json()
            .await?;

        let gen = status
            .generations
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Stable Horde returned no generations"))?;

        if gen.censored {
            return Err(anyhow!("Stable Horde censored the generation"));
        }

        // img is either a Cloudflare R2 URL or a base64 data URL
        let img_bytes: Vec<u8> = if gen.img.starts_with("data:") {
            let b64 = gen.img.split(',').nth(1).ok_or_else(|| anyhow!("Bad data URL"))?;
            base64::engine::general_purpose::STANDARD.decode(b64)?
        } else {
            self.client
                .get(&gen.img)
                .send()
                .await?
                .bytes()
                .await?
                .to_vec()
        };

        let img = image::load_from_memory(&img_bytes)?;
        Ok(img)
    }
}
