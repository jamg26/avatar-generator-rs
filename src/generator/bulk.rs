use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Result};
use base64::Engine;
use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
/// Files ≤ this size are inlined as base64; larger ones use Git LFS.
const HF_INLINE_LIMIT: usize = 10 * 1024 * 1024;

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

fn default_concurrency() -> usize { 5 }

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
    /// HF dataset download URL — present once the job reaches `done`.
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
    client:          Client,
    pool:            PgPool,
    save_dir:        PathBuf,
    hf_token:        Option<String>,
    hf_dataset_repo: String,
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
            .timeout(Duration::from_secs(180))
            .build()?;

        let hf_token        = std::env::var("HF_TOKEN").ok();
        let hf_dataset_repo = std::env::var("HF_DATASET_REPO")
            .unwrap_or_else(|_| "jamg/avagen-batches".into());

        Ok(Self { client, pool, save_dir, hf_token, hf_dataset_repo })
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

        let client          = self.client.clone();
        let pool            = self.pool.clone();
        let hf_token        = self.hf_token.clone();
        let hf_dataset_repo = self.hf_dataset_repo.clone();

        tokio::spawn(async move {
            run_job(client, pool, job_id, req, save_path, hf_token, hf_dataset_repo).await;
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
    client:          Client,
    pool:            PgPool,
    job_id:          Uuid,
    req:             BulkRequest,
    save_path:       PathBuf,
    hf_token:        Option<String>,
    hf_dataset_repo: String,
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

    match finalize_job(&client, &pool, job_id, &save_path, hf_token.as_deref(), &hf_dataset_repo).await {
        Ok(url)  => tracing::info!(?job_id, download_url = %url, "Batch job done"),
        Err(e)   => {
            tracing::error!(?job_id, "Finalize failed: {e}");
            let _ = db::fail_batch_job(&pool, job_id, &e.to_string()).await;
        }
    }
}

// ── Post-generation: zip → upload → cleanup ──────────────────────────────────

async fn finalize_job(
    client:          &Client,
    pool:            &PgPool,
    job_id:          Uuid,
    save_path:       &PathBuf,
    hf_token:        Option<&str>,
    hf_dataset_repo: &str,
) -> Result<String> {
    let zip_path = create_zip(job_id, save_path).await?;
    tracing::info!(?job_id, ?zip_path, "Zip created");

    let download_url = match hf_token {
        Some(token) => {
            let zip_bytes = tokio::fs::read(&zip_path).await?;
            upload_zip_to_hf(client, token, hf_dataset_repo, job_id, &zip_bytes).await?
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

// ── HuggingFace Dataset upload ────────────────────────────────────────────────

async fn ensure_hf_dataset_repo(client: &Client, token: &str, repo_id: &str) -> Result<()> {
    let (namespace, name) = repo_id
        .split_once('/')
        .ok_or_else(|| anyhow!("HF_DATASET_REPO must be 'namespace/name', got '{repo_id}'"))?;

    let resp = client
        .post("https://huggingface.co/api/repos/create")
        .bearer_auth(token)
        .json(&serde_json::json!({
            "type":         "dataset",
            "name":         name,
            "organization": namespace,
            "private":      false,
        }))
        .send()
        .await?;

    let st = resp.status().as_u16();
    if st == 200 || st == 201 || st == 409 {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    Err(anyhow!("Failed to create HF dataset repo ({st}): {body}"))
}

async fn upload_zip_to_hf(
    client:          &Client,
    token:           &str,
    hf_dataset_repo: &str,
    job_id:          Uuid,
    zip_bytes:       &[u8],
) -> Result<String> {
    ensure_hf_dataset_repo(client, token, hf_dataset_repo).await?;

    let filename = format!("batch_{job_id}.zip");

    if zip_bytes.len() <= HF_INLINE_LIMIT {
        commit_inline(client, token, hf_dataset_repo, &filename, zip_bytes).await?;
    } else {
        upload_lfs_and_commit(client, token, hf_dataset_repo, &filename, zip_bytes).await?;
    }

    Ok(format!(
        "https://huggingface.co/datasets/{hf_dataset_repo}/resolve/main/{filename}"
    ))
}

/// Inline base64 commit — for files ≤ 10 MB.
async fn commit_inline(
    client:  &Client,
    token:   &str,
    repo_id: &str,
    path:    &str,
    bytes:   &[u8],
) -> Result<()> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let ndjson = format!(
        "{}\n{}\n",
        serde_json::to_string(&serde_json::json!({
            "key":   "header",
            "value": { "summary": format!("Add {path}"), "description": "avagen batch results" }
        }))?,
        serde_json::to_string(&serde_json::json!({
            "key":   "file",
            "value": { "path": path, "encoding": "base64", "content": b64 }
        }))?,
    );

    let resp = client
        .post(format!("https://huggingface.co/api/datasets/{repo_id}/commit/main"))
        .bearer_auth(token)
        .header("Content-Type", "application/x-ndjson")
        .body(ndjson)
        .send()
        .await?;

    if !resp.status().is_success() {
        let st   = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("HF inline commit failed ({st}): {body}"));
    }
    Ok(())
}

/// LFS upload + pointer commit — for files > 10 MB.
async fn upload_lfs_and_commit(
    client:  &Client,
    token:   &str,
    repo_id: &str,
    path:    &str,
    bytes:   &[u8],
) -> Result<()> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let sha256 = hex::encode(hasher.finalize());
    let size   = bytes.len() as u64;

    let lfs_url = format!(
        "https://huggingface.co/datasets/{repo_id}.git/info/lfs/objects/batch"
    );
    let lfs_resp: serde_json::Value = client
        .post(&lfs_url)
        .bearer_auth(token)
        .header("Content-Type", "application/vnd.git-lfs+json")
        .header("Accept",       "application/vnd.git-lfs+json")
        .json(&serde_json::json!({
            "operation": "upload",
            "transfers": ["basic"],
            "objects":   [{ "oid": &sha256, "size": size }],
            "hash_algo": "sha256",
        }))
        .send()
        .await?
        .json()
        .await?;

    // If server already has the object, `actions` will be absent
    if let Some(actions) = lfs_resp["objects"][0].get("actions") {
        let upload_href = actions["upload"]["href"]
            .as_str()
            .ok_or_else(|| anyhow!("No upload href in LFS response"))?;

        let mut upload_req = client.put(upload_href).body(bytes.to_vec());
        if let Some(headers) = actions["upload"]["header"].as_object() {
            for (k, v) in headers {
                if let Some(v_str) = v.as_str() {
                    upload_req = upload_req.header(k.as_str(), v_str);
                }
            }
        }

        let upload_resp = upload_req.send().await?;
        if !upload_resp.status().is_success() {
            let body = upload_resp.text().await.unwrap_or_default();
            return Err(anyhow!("LFS storage upload failed: {body}"));
        }
    }

    let ndjson = format!(
        "{}\n{}\n",
        serde_json::to_string(&serde_json::json!({
            "key":   "header",
            "value": { "summary": format!("Add {path}"), "description": "avagen batch results" }
        }))?,
        serde_json::to_string(&serde_json::json!({
            "key":   "lfsFile",
            "value": { "path": path, "algo": "sha256", "oid": sha256, "size": size }
        }))?,
    );

    let resp = client
        .post(format!("https://huggingface.co/api/datasets/{repo_id}/commit/main"))
        .bearer_auth(token)
        .header("Content-Type", "application/x-ndjson")
        .body(ndjson)
        .send()
        .await?;

    if !resp.status().is_success() {
        let st   = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("HF LFS commit failed ({st}): {body}"));
    }
    Ok(())
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

async fn generate_one(
    client: &Client,
    req:    &BulkRequest,
    seed:   u64,
    out:    &Path,
) -> Result<()> {
    if matches!(req.model, BulkModel::LocalGpu) {
        return generate_one_local(client, req, seed, out).await;
    }

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
    let deadline = Instant::now() + Duration::from_secs(300);
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
            return Err(anyhow!("Horde timed out after 300s"));
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
