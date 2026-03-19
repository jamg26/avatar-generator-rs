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
/// High-worker-count model for realistic portraits — more workers = faster queue
const DEFAULT_MODEL: &str = "Dreamshaper";

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
            .timeout(Duration::from_secs(180))
            .build()?;
        Ok(Self { client })
    }

    pub async fn generate(
        &self,
        req: &AvatarRequest,
        size: usize,
        seed: u64,
    ) -> Result<DynamicImage> {
        let prompt = req.to_prompt();
        let neg = req.negative_prompt();

        // Stable Horde uses "prompt ### negative" format
        let full_prompt = format!("{prompt} ### {neg}");

        // Clamp to multiples of 64; anonymous tier works best at 512x512
        let dim = ((size.min(768) as u32).max(512) / 64) * 64;

        // Submit generation job
        let resp = self
            .client
            .post(format!("{HORDE_API}/generate/async"))
            .json(&serde_json::json!({
                "prompt": full_prompt,
                "params": {
                    "steps": 25,
                    "width": dim,
                    "height": dim,
                    "n": 1,
                    "sampler_name": "k_euler_a",
                    "cfg_scale": 7.0,
                    "seed": seed.to_string(),
                },
                "nsfw": false,
                "models": [DEFAULT_MODEL],
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

        // Poll /check until done (timeout 120s)
        let deadline = Instant::now() + Duration::from_secs(120);
        // First check after 2s — jobs often complete in 2-5s on fast workers
        sleep(Duration::from_secs(2)).await;
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
                return Err(anyhow!("Stable Horde generation timed out after 120s"));
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
