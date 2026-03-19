---
title: AvaGen
emoji: 🎭
colorFrom: blue
colorTo: purple
sdk: docker
pinned: false
app_port: 7860
license: mit
short_description: AI avatar API — LCM_Dreamshaper_v7, Rust/Axum
---

# AvaGen — AI Avatar Generation API

REST API that generates photorealistic AI avatar portraits from structured
demographic descriptions. Built with **Rust/Axum** + a **Python LCM_Dreamshaper_v7
inference sidecar**, deployed on HuggingFace Spaces (CPU, no GPU required).

## Setup — Space Secrets

Add the following in **Settings → Secrets** before the Space will work:

| Secret         | Description                                                     |
| -------------- | --------------------------------------------------------------- |
| `DATABASE_URL` | PostgreSQL connection string (e.g. [NeonDB](https://neon.tech)) |
| `ADMIN_SECRET` | Random string used to create/revoke API keys                    |
| `HF_TOKEN`     | HuggingFace token (read access is sufficient)                   |

## Quick API Reference

### Create an API key (admin only)

```bash
curl -X POST https://jamg-avagen.hf.space/api/admin/keys \
  -H "Content-Type: application/json" \
  -H "X-Admin-Secret: <ADMIN_SECRET>" \
  -d '{"name": "my-app"}'
```

### Generate an avatar

```bash
curl -X POST https://jamg-avagen.hf.space/api/v1/avatar/generate \
  -H "Content-Type: application/json" \
  -H "X-API-Key: avg_<key>" \
  -d '{
    "age": "young_adult",
    "sex": "female",
    "ethnicity": "east_asian",
    "style": "photorealistic"
  }' --output avatar.png
```

Returns a PNG image. `headshot` (default) produces a **512×512** square; `body` produces a **512×682** portrait. Generation takes ~10–20 s on CPU.

### Avatar parameters

| Field        | Values (default first)                                                           |
| ------------ | -------------------------------------------------------------------------------- |
| `age`        | `young_adult` `baby` `toddler` `child` `teenager` `adult` `middle_aged` `senior` |
| `sex`        | `female` `male`                                                                  |
| `ethnicity`  | `caucasian` `african` `east_asian` `south_asian` `hispanic` `middle_eastern` …   |
| `hair_color` | `black` `brown` `blonde` `red` `gray` `white` …                                  |
| `hair_style` | `long_straight` `short` `bun` `afro` `braids` …                                  |
| `expression` | `neutral` `happy` `serious` `confident` `friendly` …                             |
| `style`      | `photorealistic` `digital_art` `anime` `cartoon` …                               |
| `size`       | integer 128–1500 (default `512`)                                                 |
| `seed`       | integer (optional, for reproducible results)                                     |
| `shot_type`  | `headshot` (default) `body` — square crop vs portrait 3:4 canvas                 |

### Check usage

```bash
curl https://jamg-avagen.hf.space/api/v1/usage \
  -H "X-API-Key: avg_<key>"
```

## Architecture

```
HF Space container
├── Rust/Axum server  :7860  ← public HTTPS traffic
│     auth, rate-limiting, DB, routing
└── Python sidecar    :8001  ← localhost only
      LCM_Dreamshaper_v7 inference (~10-20 s/image on CPU, 4-step LCM)
```

Model weights (~2 GB) are baked into the Docker image — cold starts load
from disk in ~10 s without any network download.
