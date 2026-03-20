---
title: AvaGen
emoji: 🧑‍🎨
colorFrom: purple
colorTo: blue
sdk: docker
pinned: false
---

# AvaGen — AI Avatar Generation API

Serverless micro-SaaS API for generating AI avatar images from structured demographic descriptions. Built with Rust/Axum, backed by PostgreSQL, and powered by a local [FLUX.1-schnell OpenVINO INT4](https://huggingface.co/rupeshs/FLUX.1-schnell-openvino-int4) inference sidecar.

## Architecture

| Layer         | Technology                                                                                                                                        |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Web framework | [Axum](https://github.com/tokio-rs/axum) (Rust)                                                                                                   |
| Database      | PostgreSQL ([NeonDB](https://neon.tech) — serverless, free tier available)                                                                        |
| Image model   | [FLUX.1-schnell OpenVINO INT4](https://huggingface.co/rupeshs/FLUX.1-schnell-openvino-int4) — 2-step, ~5 s on CPU, OpenVINO-accelerated (AVX-512) |
| Deployment    | [HuggingFace Spaces](https://huggingface.co/spaces/jamg/avagen) — Docker, CPU-only, public                                                        |
| Auth          | API key (SHA-256 hashed, stored in DB)                                                                                                            |

The Rust server handles routing, auth, rate-limiting, and DB — it proxies generation
requests to a Python sidecar (`infer.py`) running on `localhost:8001`. The sidecar
loads the model on startup and serves PNG results.

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- Python 3.10+ with [diffusers](https://github.com/huggingface/diffusers) (`pip install diffusers transformers accelerate torch Pillow`)
- A [NeonDB](https://neon.tech) (or any PostgreSQL) database
- A [HuggingFace](https://huggingface.co/settings/tokens) access token (free, needed to download model weights)

## Quick Start

```bash
# 1. Copy and fill in config
cp .env.example .env
# Edit .env — set DATABASE_URL, ADMIN_SECRET, and HF_TOKEN at minimum

# 2. Install Python dependencies (one-time)
python -m venv .venv && source .venv/bin/activate
pip install diffusers transformers accelerate torch Pillow fastapi uvicorn pydantic

# 3. Start the inference sidecar
# First run downloads FLUX.1-schnell OpenVINO INT4 weights
python infer.py
# Wait for: "Pipeline ready on cpu" before proceeding

# 4. In a separate terminal, start the Rust server
cargo run

# 5. Create an API key
curl -s -X POST http://localhost:8080/api/admin/keys \
  -H "Content-Type: application/json" \
  -H "X-Admin-Secret: <your-admin-secret>" \
  -d '{"name": "my-app"}'

# 6. Generate an avatar
curl -s -X POST http://localhost:8080/api/v1/avatar/generate \
  -H "Content-Type: application/json" \
  -H "X-API-Key: avg_<key-from-step-5>" \
  -d '{
    "age": "young_adult",
    "sex": "female",
    "ethnicity": "east_asian",
    "hair_color": "black",
    "hair_style": "long_straight",
    "eye_color": "brown",
    "expression": "happy",
    "style": "photorealistic",
    "background": "white"
  }' --output avatar.png

# 7. Check usage
curl -s http://localhost:8080/api/v1/usage \
  -H "X-API-Key: avg_<key>"
```

### Skip model loading (fast local dev)

```bash
# Start sidecar with a mock stub (returns a solid-color PNG instantly, no model download)
MOCK_MODELS=1 python infer.py
```

## Deploy to HuggingFace Spaces

```bash
# One-command deploy (reads DATABASE_URL, ADMIN_SECRET, HF_TOKEN from .env)
python deploy_spaces.py
```

This will:

1. Create the public Docker Space `jamg/avagen` on HuggingFace (if it doesn't already exist)
2. Push three secrets into the Space (`DATABASE_URL`, `ADMIN_SECRET`, `HF_TOKEN`)
3. Upload all source files and trigger a Docker build (~5 min for Rust + model download)

The FLUX.1-schnell OpenVINO INT4 weights are **downloaded on first startup** and cached in
`HF_HOME`. Subsequent starts load from disk in seconds. OpenVINO uses CPU AI instructions
(AVX-512 etc.) for ~5 s/image at 512×512 without a GPU.

The Space is publicly accessible at `https://jamg-avagen.hf.space`.

## Avatar Generation

### Public

| Method | Path      | Description  |
| ------ | --------- | ------------ |
| GET    | `/`       | Service name |
| GET    | `/health` | Health check |

### Admin (requires `X-Admin-Secret` header)

| Method | Path                   | Description          |
| ------ | ---------------------- | -------------------- |
| POST   | `/api/admin/keys`      | Create a new API key |
| GET    | `/api/admin/keys`      | List all API keys    |
| DELETE | `/api/admin/keys/{id}` | Revoke an API key    |

### Authenticated (requires `X-API-Key` header)

| Method | Path                      | Description              |
| ------ | ------------------------- | ------------------------ |
| POST   | `/api/v1/avatar/generate` | Generate an avatar image |
| GET    | `/api/v1/usage`           | View your usage stats    |

### Avatar Generation Parameters

```jsonc
{
  // Required
  "age": "young_adult", // baby | toddler | child | teenager | young_adult | adult | middle_aged | senior | elderly
  "sex": "female", // male | female
  "ethnicity": "east_asian", // caucasian | african | east_asian | south_asian | southeast_asian
  // hispanic | middle_eastern | native_american | pacific_islander | mixed

  // Optional (sensible defaults)
  "hair_color": "black", // black | brown | blonde | red | gray | white | auburn | strawberry_blonde
  "hair_style": "long_straight", // bald | buzz_cut | short | medium | long_straight | long_wavy
  // long_curly | afro | braids | ponytail | bun | mohawk | dreadlocks
  "eye_color": "brown", // brown | blue | green | hazel | gray | amber
  "skin_tone": "medium_light", // very_light | light | medium_light | medium | medium_dark | dark | very_dark
  "facial_hair": "none", // none | stubble | mustache | goatee | full_beard | long_beard
  "expression": "neutral", // neutral | happy | serious | confident | friendly | thoughtful | surprised
  "accessories": ["glasses"], // glasses | sunglasses | earrings | nose_ring | headband | hat
  // hijab | turban | necklace | scarf
  "background": "white", // white | gray | blue | gradient | nature | studio
  "style": "photorealistic", // photorealistic | digital_art | anime | cartoon | watercolor | oil_painting | pixel_art
  "format": "png", // png | jpeg | webp
  "size": 512, // integer, 128–1500 — rounded to nearest multiple of 64 (default: 512)
  "seed": 42, // optional — use for reproducible results
  "shot_type": "headshot", // headshot (default) | body
  // headshot → square canvas (e.g. 512×512), tight face+shoulders crop
  // body     → portrait 3:4 canvas (e.g. 512×682), half-body centered
}
```

### Create API Key Request Body

```jsonc
{
  "name": "my-app", // human-readable label
  "quota": 1000, // max total generations allowed (omit for unlimited)
}
```

## Environment Variables

| Variable                | Default                                | Description                                          |
| ----------------------- | -------------------------------------- | ---------------------------------------------------- |
| `DATABASE_URL`          | _(required)_                           | PostgreSQL connection string                         |
| `ADMIN_SECRET`          | _(required)_                           | Secret for admin endpoints                           |
| `HF_TOKEN`              | _(required)_                           | HuggingFace token for downloading model weights      |
| `SD_MODEL_REPO`         | `rupeshs/FLUX.1-schnell-openvino-int4` | OpenVINO INT4 FLUX repo on HuggingFace               |
| `SD_NUM_STEPS`          | `2`                                    | Inference steps (2 gives best speed/quality balance) |
| `SD_GUIDANCE_SCALE`     | `1.0`                                  | Required value for FLUX.1-schnell with OpenVINO      |
| `SD_DEFAULT_SIZE`       | `512`                                  | Default output size in pixels                        |
| `SKIP_SD_PIPELINE`      | `0`                                    | Set to `1` to disable avatar generation (503)        |
| `HF_HOME`               | `~/.cache/huggingface`                 | Local model weight cache directory                   |
| `PORT`                  | `8080`                                 | HTTP port                                            |
| `RUST_LOG`              | `avagen=info,tower_http=info`          | Log filter                                           |
| `RATE_LIMIT_PER_MINUTE` | `60`                                   | Per-IP rate limit                                    |

## Running the Test Suite

```bash
# Run against a local instance (both sidecar and cargo run must be running)
ADMIN_SECRET=<your-secret> ./test.sh

# Run against the deployed HF Space
BASE=https://jamg-avagen.hf.space ADMIN_SECRET=<your-secret> ./test.sh

# Run sidecar unit tests without starting Cargo
MOCK_MODELS=1 pytest tests/
```

## License

MIT
