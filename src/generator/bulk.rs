use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Result};
use base64::Engine;
use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::time::{sleep, Duration, Instant};
use uuid::Uuid;

use crate::db;
use super::prompt::{
    Age, ArtStyle, AvatarRequest, Background, EyeColor, Ethnicity, Expression,
    FacialHair, HairColor, HairStyle, ImageFormat, Sex, ShotType,
};

const HORDE_API: &str = "https://stablehorde.net/api/v2";
const ANON_KEY: &str = "0000000000";

// ── Model selection ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BulkModel {
    /// Local NVIDIA GPU via the diffusers inference sidecar (dev-only).
    LocalGpu,
    /// FLUX.1-Schnell via Stable Horde — works everywhere including HF Spaces
    #[default]
    Flux,
    /// Dreamshaper via Stable Horde
    Dreamshaper,
}

impl BulkModel {
    fn horde_name(&self) -> &'static str {
        match self {
            Self::LocalGpu => unreachable!("LocalGpu uses local sidecar, not Stable Horde"),
            Self::Flux => "Flux.1-Schnell fp8 (Compact)",
            Self::Dreamshaper => "Dreamshaper",
        }
    }
    fn steps(&self) -> u32 {
        match self {
            Self::LocalGpu | Self::Flux => 4,
            Self::Dreamshaper => 25,
        }
    }
    fn guidance(&self) -> f32 {
        match self {
            Self::LocalGpu | Self::Flux => 1.0,
            Self::Dreamshaper => 7.0,
        }
    }
    fn use_negative(&self) -> bool {
        matches!(self, Self::Dreamshaper)
    }
    fn db_name(&self) -> &'static str {
        match self {
            Self::LocalGpu  => "local_gpu",
            Self::Flux      => "flux",
            Self::Dreamshaper => "dreamshaper",
        }
    }
}

// ── Request / response types ─────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct BulkRequest {
    pub sex: Sex,
    pub age: Age,
    #[serde(default)]
    pub ethnicity: Option<Ethnicity>,
    pub count: usize,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default)]
    pub model: BulkModel,
}

fn default_concurrency() -> usize { 1 }

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Uploading,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchJobStatus {
    pub job_id:       Uuid,
    pub state:        JobState,
    pub total:        usize,
    pub completed:    usize,
    pub failed:       usize,
    /// HF bucket download URL — present once the job reaches `done`.
    pub download_url: Option<String>,
}

impl From<crate::db::BatchJobRow> for BatchJobStatus {
    fn from(row: crate::db::BatchJobRow) -> Self {
        Self {
            job_id: row.id,
            state: match row.state.as_str() {
                "running"   => JobState::Running,
                "uploading" => JobState::Uploading,
                "done"      => JobState::Done,
                "failed"    => JobState::Failed,
                _           => JobState::Queued,
            },
            total:        row.total as usize,
            completed:    row.completed as usize,
            failed:       row.failed_count as usize,
            download_url: row.download_url,
        }
    }
}

// ── Pipeline ──────────────────────────────────────────────────────────────────

pub struct BulkPipeline {
    client:       Client,
    pool:         PgPool,
    save_dir:     PathBuf,
    hf_token:     Option<String>,
    hf_bucket_id: String,
}

impl BulkPipeline {
    pub fn new(save_dir: PathBuf, pool: PgPool) -> Result<Self> {
        let api_key = std::env::var("STABLE_HORDE_KEY")
            .unwrap_or_else(|_| ANON_KEY.into());

        let mut hdrs = header::HeaderMap::new();
        hdrs.insert(
            "apikey",
            header::HeaderValue::from_str(&api_key)
                .map_err(|e| anyhow!("Invalid STABLE_HORDE_KEY: {e}"))?,
        );

        let client = Client::builder()
            .default_headers(hdrs)
            .user_agent("avagen-bulk/1.0")
            .timeout(Duration::from_secs(600))
            .build()?;

        let hf_token     = std::env::var("HF_TOKEN").ok();
        let hf_bucket_id = std::env::var("HF_BUCKET_ID")
            .unwrap_or_else(|_| "jamg/avagen-batches".into());

        Ok(Self { client, pool, save_dir, hf_token, hf_bucket_id })
    }

    /// Submit a batch job — returns immediately; generation + upload run in background.
    pub async fn submit(&self, req: BulkRequest) -> Result<BatchJobStatus> {
        let job_id    = Uuid::new_v4();
        let save_path = self.save_dir.join(job_id.to_string());

        db::create_batch_job(&self.pool, job_id, req.count as i64, req.model.db_name())
            .await
            .map_err(|e| anyhow!("DB error inserting batch job: {e}"))?;

        let status = BatchJobStatus {
            job_id,
            state:        JobState::Queued,
            total:        req.count,
            completed:    0,
            failed:       0,
            download_url: None,
        };

        let client       = self.client.clone();
        let pool         = self.pool.clone();
        let hf_token     = self.hf_token.clone();
        let hf_bucket_id = self.hf_bucket_id.clone();

        tokio::spawn(async move {
            run_job(client, pool, job_id, req, save_path, hf_token, hf_bucket_id).await;
        });

        Ok(status)
    }

    pub async fn get_status(&self, job_id: Uuid) -> Result<Option<BatchJobStatus>> {
        db::get_batch_job(&self.pool, job_id)
            .await
            .map(|opt| opt.map(BatchJobStatus::from))
            .map_err(|e| anyhow!("DB error: {e}"))
    }

    pub async fn list_all(&self) -> Result<Vec<BatchJobStatus>> {
        db::list_batch_jobs(&self.pool)
            .await
            .map(|rows| rows.into_iter().map(BatchJobStatus::from).collect())
            .map_err(|e| anyhow!("DB error: {e}"))
    }
}

// ── Background worker pool ────────────────────────────────────────────────────

async fn run_job(
    client:       Client,
    pool:         PgPool,
    job_id:       Uuid,
    req:          BulkRequest,
    save_path:    PathBuf,
    hf_token:     Option<String>,
    hf_bucket_id: String,
) {
    if let Err(e) = tokio::fs::create_dir_all(&save_path).await {
        tracing::error!(?job_id, "Failed to create save dir: {e}");
        let _ = db::fail_batch_job(&pool, job_id, &e.to_string()).await;
        return;
    }

    let _ = db::update_batch_job_state(&pool, job_id, "running").await;

    let concurrency = req.concurrency.clamp(1, 20);
    let count       = req.count;
    let req         = Arc::new(req);

    let (work_tx, work_rx)      = tokio::sync::mpsc::channel::<usize>(concurrency * 2);
    let work_rx                 = Arc::new(tokio::sync::Mutex::new(work_rx));
    let (prog_tx, mut prog_rx)  = tokio::sync::mpsc::channel::<bool>(concurrency * 2);

    for _ in 0..concurrency {
        let rx  = work_rx.clone();
        let ptx = prog_tx.clone();
        let c   = client.clone();
        let sp  = save_path.clone();
        let r   = req.clone();

        tokio::spawn(async move {
            loop {
                let idx = rx.lock().await.recv().await;
                match idx {
                    Some(i) => {
                        let seed = rand::random::<u64>();
                        let out  = sp.join(format!("{i:08}.jpg"));
                        let ok   = generate_one(&c, &r, seed, &out).await.is_ok();
                        let _ = ptx.send(ok).await;
                    }
                    None => break,
                }
            }
        });
    }
    drop(prog_tx);

    tokio::spawn(async move {
        for i in 0..count {
            if work_tx.send(i).await.is_err() { break; }
        }
    });

    let mut completed:    i64   = 0;
    let mut failed_count: i64   = 0;
    let mut last_db             = Instant::now();

    while let Some(ok) = prog_rx.recv().await {
        if ok { completed += 1; } else { failed_count += 1; }
        if last_db.elapsed() >= Duration::from_secs(2) {
            let _ = db::update_batch_job_progress(&pool, job_id, completed, failed_count).await;
            last_db = Instant::now();
        }
    }

    let _ = db::update_batch_job_progress(&pool, job_id, completed, failed_count).await;

    tracing::info!(?job_id, completed, failed_count, "Generation complete — zipping and uploading");
    let _ = db::update_batch_job_state(&pool, job_id, "uploading").await;

    match finalize_job(&pool, job_id, &save_path, hf_token.as_deref(), &hf_bucket_id).await {
        Ok(url)  => tracing::info!(?job_id, download_url = %url, "Batch job done"),
        Err(e)   => {
            tracing::error!(?job_id, "Finalize failed: {e}");
            let _ = db::fail_batch_job(&pool, job_id, &e.to_string()).await;
        }
    }
}

// ── Post-generation: zip → upload → cleanup ──────────────────────────────────

async fn finalize_job(
    pool:         &PgPool,
    job_id:       Uuid,
    save_path:    &PathBuf,
    hf_token:     Option<&str>,
    hf_bucket_id: &str,
) -> Result<String> {
    let zip_path = create_zip(job_id, save_path).await?;
    tracing::info!(?job_id, ?zip_path, "Zip created");

    let download_url = match hf_token {
        Some(token) => {
            let filename = format!("batch_{job_id}.zip");
            upload_zip_to_bucket(token, hf_bucket_id, &zip_path, &filename).await?
        }
        None => {
            tracing::warn!(?job_id, "HF_TOKEN not set — skipping upload");
            db::complete_batch_job(pool, job_id, None).await?;
            return Ok(String::new());
        }
    };

    db::complete_batch_job(pool, job_id, Some(&download_url)).await?;

    if let Err(e) = tokio::fs::remove_dir_all(save_path).await {
        tracing::warn!(?job_id, "Failed to remove image dir: {e}");
    }
    if let Err(e) = tokio::fs::remove_file(&zip_path).await {
        tracing::warn!(?job_id, "Failed to remove zip: {e}");
    }

    Ok(download_url)
}

// ── Zip creation ──────────────────────────────────────────────────────────────

async fn create_zip(job_id: Uuid, save_path: &Path) -> Result<PathBuf> {
    let save_path = save_path.to_path_buf();
    let zip_path  = save_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{job_id}.zip"));
    let zip_path2 = zip_path.clone();

    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(&save_path)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        let file = std::fs::File::create(&zip_path2)?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        for entry in entries {
            let fname = entry.file_name().to_string_lossy().to_string();
            zip.start_file(&fname, opts)?;
            let bytes = std::fs::read(entry.path())?;
            zip.write_all(&bytes)?;
        }
        zip.finish()?;
        Ok(())
    })
    .await??;

    Ok(zip_path)
}

// ── HuggingFace Bucket upload ─────────────────────────────────────────────────

/// Upload `zip_path` to the HF bucket `bucket_id` via the `huggingface_hub`
/// Python library (subprocess). Returns the public HTTPS download URL.
async fn upload_zip_to_bucket(
    token:        &str,
    bucket_id:    &str,
    zip_path:     &Path,
    filename:     &str,
) -> Result<String> {
    // Pass data through argv (not embedded in the script string) to avoid
    // shell injection; token goes via env-var so it never touches the script.
    let script = r#"
import os, sys
from huggingface_hub import HfApi, batch_bucket_files
bucket_id = sys.argv[1]
zip_path  = sys.argv[2]
filename  = sys.argv[3]
api = HfApi()
try:
    api.create_bucket(bucket_id, exist_ok=True)
except Exception as e:
    print(f"Warning: create_bucket: {e}", file=sys.stderr)
batch_bucket_files(bucket_id, add=[(zip_path, filename)])
print(f"https://huggingface.co/buckets/{bucket_id}/{filename}")
"#;

    let output = tokio::process::Command::new("/opt/hfvenv/bin/python3")
        .arg("-c")
        .arg(script)
        .arg(bucket_id)
        .arg(zip_path)
        .arg(filename)
        .env("HF_TOKEN", token)
        .output()
        .await
        .map_err(|e| anyhow!("Failed to run python3: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Bucket upload failed: {stderr}"));
    }

    // Take the last non-empty line (the URL we printed)
    let url = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(url)
}

// ── Single-image generator ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AsyncResp  { id: String }
#[derive(Deserialize)]
struct CheckResp  { done: bool, faulted: Option<bool> }
#[derive(Deserialize)]
struct StatusResp { generations: Vec<Gen> }
#[derive(Deserialize)]
struct Gen        { img: String }

/// Retry wrapper — up to 3 Stable Horde attempts per image with a new seed.
async fn generate_one(
    client: &Client,
    req:    &BulkRequest,
    seed:   u64,
    out:    &Path,
) -> Result<()> {
    if matches!(req.model, BulkModel::LocalGpu) {
        return generate_one_local(client, req, seed, out).await;
    }

    let mut current_seed = seed;
    for attempt in 1u32..=3 {
        match try_generate_horde(client, req, current_seed, out).await {
            Ok(()) => return Ok(()),
            Err(e) if attempt < 3 => {
                tracing::warn!("Horde attempt {attempt}/3 failed: {e} — retrying in 5s");
                sleep(Duration::from_secs(5)).await;
                current_seed = rand::random();
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

async fn try_generate_horde(
    client: &Client,
    req:    &BulkRequest,
    seed:   u64,
    out:    &Path,
) -> Result<()> {
    let avatar      = make_avatar_request(req);
    let prompt      = avatar.to_prompt();
    let full_prompt = if req.model.use_negative() {
        format!("{prompt} ### {neg}", neg = avatar.negative_prompt())
    } else {
        prompt
    };

    let resp = client
        .post(format!("{HORDE_API}/generate/async"))
        .json(&serde_json::json!({
            "prompt": full_prompt,
            "params": {
                "steps":        req.model.steps(),
                "width":        512,
                "height":       512,
                "n":            1,
                "sampler_name": "k_euler",
                "cfg_scale":    req.model.guidance(),
                "seed":         seed.to_string(),
            },
            "nsfw":         false,
            "censor_nsfw":  false,
            "slow_workers": true,
            "models":       [req.model.horde_name()],
            "r2":           true,
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let code = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Horde submit failed {code}: {body}"));
    }

    let horde_id = resp.json::<AsyncResp>().await?.id;
    let deadline = Instant::now() + Duration::from_secs(900);
    sleep(Duration::from_secs(3)).await;

    loop {
        let check = client
            .get(format!("{HORDE_API}/generate/check/{horde_id}"))
            .send().await?
            .json::<CheckResp>().await?;

        if check.faulted.unwrap_or(false) {
            return Err(anyhow!("Horde generation faulted"));
        }
        if check.done { break; }
        if Instant::now() > deadline {
            return Err(anyhow!("Horde timed out after 900s"));
        }
        sleep(Duration::from_secs(3)).await;
    }

    let gen = client
        .get(format!("{HORDE_API}/generate/status/{horde_id}"))
        .send().await?
        .json::<StatusResp>().await?
        .generations
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("No generations returned"))?;

    let bytes: Vec<u8> = if gen.img.starts_with("data:") {
        let b64 = gen.img.split(',').nth(1)
            .ok_or_else(|| anyhow!("Bad data URL"))?;
        base64::engine::general_purpose::STANDARD.decode(b64)?
    } else {
        client.get(&gen.img).send().await?.bytes().await?.to_vec()
    };

    let img = image::load_from_memory(&bytes)?;
    img.save(out)?;
    Ok(())
}

// ── Local GPU sidecar generator ───────────────────────────────────────────────

#[derive(Serialize)]
struct GpuReq<'a> { prompt: &'a str, width: u32, height: u32, steps: u32, seed: u64 }
#[derive(Deserialize)]
struct GpuResp    { image: String }

async fn generate_one_local(
    client: &Client,
    req:    &BulkRequest,
    seed:   u64,
    out:    &Path,
) -> Result<()> {
    let gpu_url = std::env::var("LOCAL_GPU_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8765".into());

    let avatar = make_avatar_request(req);
    let prompt = avatar.to_prompt();

    let resp = client
        .post(format!("{gpu_url}/generate"))
        .timeout(Duration::from_secs(120))
        .json(&GpuReq { prompt: &prompt, width: 512, height: 512, steps: 4, seed })
        .send()
        .await
        .map_err(|e| anyhow!(
            "Local GPU sidecar unreachable at {gpu_url}: {e}\n\
             Hint: start it with `./start_inference.sh`"
        ))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Local GPU sidecar error: {body}"));
    }

    let r: GpuResp = resp.json().await?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(&r.image)?;
    let img = image::load_from_memory(&bytes)?;
    img.save(out)?;
    Ok(())
}

// ── Variety helpers ───────────────────────────────────────────────────────────

fn make_avatar_request(req: &BulkRequest) -> AvatarRequest {
    let ethnicity = req.ethnicity.unwrap_or_else(random_ethnicity);
    AvatarRequest {
        sex:         req.sex,
        age:         req.age,
        ethnicity,
        hair_color:  random_hair_color(),
        hair_style:  random_hair_style(req.sex),
        eye_color:   EyeColor::Brown,
        skin_tone:   None,
        facial_hair: random_facial_hair(req.sex),
        expression:  random_expression(),
        accessories: vec![],
        background:  Background::Studio,
        style:       ArtStyle::Photorealistic,
        format:      ImageFormat::Jpeg,
        size:        Some(512),
        seed:        None,
        shot_type:   ShotType::Headshot,
    }
}

fn random_ethnicity() -> Ethnicity {
    use Ethnicity::*;
    const C: [Ethnicity; 6] = [Caucasian, African, EastAsian, SouthAsian, Hispanic, MiddleEastern];
    C[rand::random::<usize>() % C.len()]
}

fn random_hair_color() -> HairColor {
    use HairColor::*;
    const C: [HairColor; 5] = [Black, Brown, Blonde, Red, Auburn];
    C[rand::random::<usize>() % C.len()]
}

fn random_hair_style(sex: Sex) -> HairStyle {
    use HairStyle::*;
    match sex {
        Sex::Male => {
            const C: [HairStyle; 3] = [Short, BuzzCut, Medium];
            C[rand::random::<usize>() % C.len()]
        }
        Sex::Female => {
            const C: [HairStyle; 6] = [Medium, LongStraight, LongWavy, LongCurly, Bun, Ponytail];
            C[rand::random::<usize>() % C.len()]
        }
    }
}

fn random_expression() -> Expression {
    use Expression::*;
    const C: [Expression; 5] = [Neutral, Happy, Serious, Confident, Friendly];
    C[rand::random::<usize>() % C.len()]
}

fn random_facial_hair(sex: Sex) -> FacialHair {
    match sex {
        Sex::Female => FacialHair::None,
        Sex::Male => {
            use FacialHair::*;
            const C: [FacialHair; 6] = [None, None, None, Stubble, Goatee, FullBeard];
            C[rand::random::<usize>() % C.len()]
        }
    }
}
