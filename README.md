# AvaGen — AI Avatar Generation API

Serverless micro-SaaS API for generating AI avatar images from structured demographic descriptions. Built with Rust/Axum, backed by PostgreSQL, and powered by the HuggingFace Inference API (FLUX.1-schnell).

## Architecture

| Layer         | Technology                                                                                                                                           |
| ------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| Web framework | [Axum](https://github.com/tokio-rs/axum) (Rust)                                                                                                      |
| Database      | PostgreSQL ([NeonDB](https://neon.tech) — serverless, free tier available)                                                                           |
| Image model   | [FLUX.1-schnell](https://huggingface.co/black-forest-labs/FLUX.1-schnell) via [HuggingFace Inference API](https://huggingface.co/docs/api-inference) |
| Deployment    | [Modal.com](https://modal.com) serverless container (scale-to-zero)                                                                                  |
| Auth          | API key (SHA-256 hashed, stored in DB)                                                                                                               |

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- A [NeonDB](https://neon.tech) (or any PostgreSQL) database
- A [HuggingFace](https://huggingface.co/settings/tokens) access token (free)

## Quick Start

```bash
# 1. Copy and fill in config
cp .env.example .env
# Edit .env — set DATABASE_URL, ADMIN_SECRET, and HF_TOKEN at minimum

# 2. Run locally
cargo run --release

# 3. Create an API key
curl -s -X POST http://localhost:8080/api/admin/keys \
  -H "Content-Type: application/json" \
  -H "X-Admin-Secret: <your-admin-secret>" \
  -d '{"name": "my-app", "quota": 1000}'

# 4. Generate an avatar
curl -s -X POST http://localhost:8080/api/v1/avatar/generate \
  -H "Content-Type: application/json" \
  -H "X-API-Key: avg_<key-from-step-3>" \
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

# 5. Check usage
curl -s http://localhost:8080/api/v1/usage \
  -H "X-API-Key: avg_<key>"
```

## Deploy to Modal

```bash
# Install Modal client
pip install modal

# Authenticate
modal setup

# Deploy (reads secrets from your .env file)
modal deploy modal_app.py
```

The app scales to zero when idle — you only pay for actual inference time.

## API Reference

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
  "size": 1024, // integer, 128–1500 — rounded to nearest multiple of 64 (default: 1024)
  "seed": 42, // optional — use for reproducible results
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

| Variable                | Default                            | Description                                     |
| ----------------------- | ---------------------------------- | ----------------------------------------------- |
| `DATABASE_URL`          | _(required)_                       | PostgreSQL connection string                    |
| `ADMIN_SECRET`          | _(required)_                       | Secret for admin endpoints                      |
| `HF_TOKEN`              | _(required)_                       | HuggingFace access token                        |
| `SD_MODEL_REPO`         | `black-forest-labs/FLUX.1-schnell` | HuggingFace model repo                          |
| `SD_NUM_STEPS`          | `4`                                | Inference steps (4 is optimal for FLUX-schnell) |
| `SD_GUIDANCE_SCALE`     | `3.5`                              | Classifier-free guidance scale                  |
| `SD_DEFAULT_SIZE`       | `1024`                             | Default output size in pixels                   |
| `SKIP_SD_PIPELINE`      | `0`                                | Set to `1` to disable generation (returns 503)  |
| `PORT`                  | `8080`                             | HTTP port                                       |
| `RUST_LOG`              | `avagen=info,tower_http=info`      | Log filter                                      |
| `RATE_LIMIT_PER_MINUTE` | `60`                               | Per-IP rate limit                               |

## Running the Test Suite

```bash
# Run against a local instance
ADMIN_SECRET=<your-secret> ./test.sh

# Run against a deployed instance
BASE=https://your-deployment.modal.run ADMIN_SECRET=<your-secret> ./test.sh
```

## License

MIT
