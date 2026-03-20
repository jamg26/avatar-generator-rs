use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Result};
use base64::Engine;
use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration, Instant};
use uuid::Uuid;

use super::prompt::{
    Age, ArtStyle, AvatarRequest, Background, EyeColor, Ethnicity, Expression,
    FacialHair, HairColor, HairStyle, ImageFormat, Sex, ShotType,
};

const HORDE_API: &str = "https://stablehorde.net/api/v2";
const ANON_KEY: &str = "0000000000";

// ── Model selection ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BulkModel {
    /// Local NVIDIA GPU via the diffusers inference sidecar (fastest, no quota).
    /// Run `python3 inference_server.py` before starting avagen.
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
            Self::LocalGpu => 4,
            Self::Flux => 4,
            Self::Dreamshaper => 25,
        }
    }
    fn guidance(&self) -> f32 {
        match self {
            Self::LocalGpu => 0.0,
            Self::Flux => 1.0,
            Self::Dreamshaper => 7.0,
        }
    }
    /// FLUX/LocalGpu ignore negative prompts; Dreamshaper benefits from them.
    fn use_negative(&self) -> bool {
        matches!(self, Self::Dreamshaper)
    }
}

// ── Request / response types ─────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct BulkRequest {
    /// male | female
    pub sex: Sex,
    /// baby | toddler | child | teenager | young_adult | adult | middle_aged | senior | elderly
    pub age: Age,
    /// Optional — randomised if omitted
    #[serde(default)]
    pub ethnicity: Option<Ethnicity>,
    /// Number of images to generate (1 – 1,000,000)
    pub count: usize,
    /// Concurrent Stable Horde requests (default 5, max 20)
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// flux | dreamshaper (default: dreamshaper)
    #[serde(default)]
    pub model: BulkModel,
}

fn default_concurrency() -> usize {
    5
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Done,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchJobStatus {
    pub job_id: Uuid,
    pub state: JobState,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    /// Absolute path where images are being saved
    pub save_path: String,
}

// ── Internal job entry ───────────────────────────────────────────────────────

struct JobEntry {
    status: Mutex<BatchJobStatus>,
}

// ── Pipeline ─────────────────────────────────────────────────────────────────

pub struct BulkPipeline {
    client: Client,
    save_dir: PathBuf,
    jobs: RwLock<HashMap<Uuid, Arc<JobEntry>>>,
}

impl BulkPipeline {
    pub fn new(save_dir: PathBuf) -> Result<Self> {
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

        Ok(Self {
            client,
            save_dir,
            jobs: RwLock::new(HashMap::new()),
        })
    }

    /// Submit a batch job. Returns immediately; generation runs in background.
    pub async fn submit(&self, req: BulkRequest) -> BatchJobStatus {
        let job_id = Uuid::new_v4();
        let save_path = self.save_dir.join(job_id.to_string());

        let status = BatchJobStatus {
            job_id,
            state: JobState::Queued,
            total: req.count,
            completed: 0,
            failed: 0,
            save_path: save_path.to_string_lossy().into_owned(),
        };

        let entry = Arc::new(JobEntry {
            status: Mutex::new(status.clone()),
        });

        self.jobs.write().await.insert(job_id, entry.clone());

        // Clone what the background task needs (no Arc<Self> required)
        let client = self.client.clone();
        tokio::spawn(async move {
            run_job(client, job_id, entry, req, save_path).await;
        });

        status
    }

    pub async fn get_status(&self, job_id: Uuid) -> Option<BatchJobStatus> {
        self.jobs
            .read()
            .await
            .get(&job_id)
            .map(|e| e.status.lock().unwrap().clone())
    }

    pub async fn list_all(&self) -> Vec<BatchJobStatus> {
        self.jobs
            .read()
            .await
            .values()
            .map(|e| e.status.lock().unwrap().clone())
            .collect()
    }
}

// ── Background worker pool ────────────────────────────────────────────────────

async fn run_job(
    client: Client,
    job_id: Uuid,
    entry: Arc<JobEntry>,
    req: BulkRequest,
    save_path: PathBuf,
) {
    if let Err(e) = tokio::fs::create_dir_all(&save_path).await {
        tracing::error!(?job_id, "Failed to create save dir: {e}");
        return;
    }

    entry.status.lock().unwrap().state = JobState::Running;

    let concurrency = req.concurrency.clamp(1, 20);
    let count = req.count;
    let req = Arc::new(req);

    // Bounded work channel (backpressure prevents memory explosion for large counts)
    let (work_tx, work_rx) =
        tokio::sync::mpsc::channel::<usize>(concurrency * 2);
    let work_rx = Arc::new(tokio::sync::Mutex::new(work_rx));

    // Progress channel: each worker sends true (ok) or false (failed)
    let (prog_tx, mut prog_rx) =
        tokio::sync::mpsc::channel::<bool>(concurrency * 2);

    // Spawn N worker tasks
    for _ in 0..concurrency {
        let rx = work_rx.clone();
        let ptx = prog_tx.clone();
        let c = client.clone();
        let sp = save_path.clone();
        let r = req.clone();

        tokio::spawn(async move {
            loop {
                let idx = rx.lock().await.recv().await;
                match idx {
                    Some(i) => {
                        let seed = rand::random::<u64>();
                        let out = sp.join(format!("{i:08}.jpg"));
                        let ok = generate_one(&c, &r, seed, &out).await.is_ok();
                        let _ = ptx.send(ok).await;
                    }
                    None => break,
                }
            }
        });
    }

    // Drop extra prog_tx so the channel closes when all workers finish
    drop(prog_tx);

    // Feed work indices (spawned separately so it doesn't block)
    tokio::spawn(async move {
        for i in 0..count {
            if work_tx.send(i).await.is_err() {
                break;
            }
        }
        // work_tx drops here → workers exit their loops
    });

    // Collect progress and update job status
    while let Some(ok) = prog_rx.recv().await {
        let mut st = entry.status.lock().unwrap();
        if ok {
            st.completed += 1;
        } else {
            st.failed += 1;
        }
    }

    entry.status.lock().unwrap().state = JobState::Done;
    tracing::info!(?job_id, "Bulk job complete");
}

// ── Single-image generator ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AsyncResp {
    id: String,
}
#[derive(Deserialize)]
struct CheckResp {
    done: bool,
    faulted: Option<bool>,
}
#[derive(Deserialize)]
struct StatusResp {
    generations: Vec<Gen>,
}
#[derive(Deserialize)]
struct Gen {
    img: String,
}

async fn generate_one(
    client: &Client,
    req: &BulkRequest,
    seed: u64,
    out: &Path,
) -> Result<()> {
    // Dispatch local GPU jobs to the Python inference sidecar
    if matches!(req.model, BulkModel::LocalGpu) {
        return generate_one_local(client, req, seed, out).await;
    }

    let avatar = make_avatar_request(req);
    let prompt = avatar.to_prompt();
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
                "steps": req.model.steps(),
                "width": 512,
                "height": 512,
                "n": 1,
                "sampler_name": "k_euler",
                "cfg_scale": req.model.guidance(),
                "seed": seed.to_string(),
            },
            "nsfw": false,
            "censor_nsfw": false,
            "slow_workers": true,
            "models": [req.model.horde_name()],
            "r2": true,
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let code = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Horde submit failed {code}: {body}"));
    }

    let horde_job_id = resp.json::<AsyncResp>().await?.id;

    let deadline = Instant::now() + Duration::from_secs(300);
    sleep(Duration::from_secs(3)).await;
    loop {
        let check = client
            .get(format!("{HORDE_API}/generate/check/{horde_job_id}"))
            .send()
            .await?
            .json::<CheckResp>()
            .await?;

        if check.faulted.unwrap_or(false) {
            return Err(anyhow!("Horde generation faulted"));
        }
        if check.done {
            break;
        }
        if Instant::now() > deadline {
            return Err(anyhow!("Horde timed out after 300s"));
        }
        sleep(Duration::from_secs(3)).await;
    }

    let gen = client
        .get(format!("{HORDE_API}/generate/status/{horde_job_id}"))
        .send()
        .await?
        .json::<StatusResp>()
        .await?
        .generations
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("No generations returned"))?;

    let bytes: Vec<u8> = if gen.img.starts_with("data:") {
        let b64 = gen
            .img
            .split(',')
            .nth(1)
            .ok_or_else(|| anyhow!("Bad data URL"))?;
        base64::engine::general_purpose::STANDARD.decode(b64)?
    } else {
        client
            .get(&gen.img)
            .send()
            .await?
            .bytes()
            .await?
            .to_vec()
    };

    let img = image::load_from_memory(&bytes)?;
    img.save(out)?;
    Ok(())
}

// ── Local GPU sidecar generator ───────────────────────────────────────────────

#[derive(serde::Serialize)]
struct GpuReq<'a> {
    prompt: &'a str,
    width: u32,
    height: u32,
    steps: u32,
    seed: u64,
}

#[derive(serde::Deserialize)]
struct GpuResp {
    image: String,
}

/// Calls the local Python inference sidecar (inference_server.py).
async fn generate_one_local(
    client: &Client,
    req: &BulkRequest,
    seed: u64,
    out: &Path,
) -> Result<()> {
    let gpu_url = std::env::var("LOCAL_GPU_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8765".into());

    let avatar = make_avatar_request(req);
    let prompt = avatar.to_prompt();

    let resp = client
        .post(format!("{gpu_url}/generate"))
        .timeout(Duration::from_secs(120))
        .json(&GpuReq {
            prompt: &prompt,
            width: 512,
            height: 512,
            steps: 4,
            seed,
        })
        .send()
        .await
        .map_err(|e| {
            anyhow!(
                "Local GPU sidecar unreachable at {gpu_url}: {e}\n\
                 Hint: start it with `python3 inference_server.py`"
            )
        })?;

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

/// Build an AvatarRequest with randomised attributes for variety across a bulk run.
fn make_avatar_request(req: &BulkRequest) -> AvatarRequest {
    let ethnicity = req.ethnicity.unwrap_or_else(random_ethnicity);
    AvatarRequest {
        sex: req.sex,
        age: req.age,
        ethnicity,
        hair_color: random_hair_color(),
        hair_style: random_hair_style(req.sex),
        eye_color: EyeColor::Brown,
        skin_tone: None,
        facial_hair: random_facial_hair(req.sex),
        expression: random_expression(),
        accessories: vec![],
        background: Background::Studio,
        style: ArtStyle::Photorealistic,
        format: ImageFormat::Jpeg,
        size: Some(512),
        seed: None,
        shot_type: ShotType::Headshot,
    }
}

fn random_ethnicity() -> Ethnicity {
    use Ethnicity::*;
    const CHOICES: [Ethnicity; 6] =
        [Caucasian, African, EastAsian, SouthAsian, Hispanic, MiddleEastern];
    CHOICES[rand::random::<usize>() % CHOICES.len()]
}

fn random_hair_color() -> HairColor {
    use HairColor::*;
    const CHOICES: [HairColor; 5] = [Black, Brown, Blonde, Red, Auburn];
    CHOICES[rand::random::<usize>() % CHOICES.len()]
}

fn random_hair_style(sex: Sex) -> HairStyle {
    use HairStyle::*;
    match sex {
        Sex::Male => {
            const C: [HairStyle; 3] = [Short, BuzzCut, Medium];
            C[rand::random::<usize>() % C.len()]
        }
        Sex::Female => {
            const C: [HairStyle; 6] =
                [Medium, LongStraight, LongWavy, LongCurly, Bun, Ponytail];
            C[rand::random::<usize>() % C.len()]
        }
    }
}

fn random_expression() -> Expression {
    use Expression::*;
    const CHOICES: [Expression; 5] =
        [Neutral, Happy, Serious, Confident, Friendly];
    CHOICES[rand::random::<usize>() % CHOICES.len()]
}

fn random_facial_hair(sex: Sex) -> FacialHair {
    match sex {
        Sex::Female => FacialHair::None,
        Sex::Male => {
            use FacialHair::*;
            // Weight toward None (cleaner-looking), but add variety
            const CHOICES: [FacialHair; 6] =
                [None, None, None, Stubble, Goatee, FullBeard];
            CHOICES[rand::random::<usize>() % CHOICES.len()]
        }
    }
}
