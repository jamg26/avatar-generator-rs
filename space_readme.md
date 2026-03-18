---
title: AvaGen
emoji: 🎭
colorFrom: blue
colorTo: purple
sdk: docker
pinned: false
app_port: 7860
license: mit
short_description: AI avatar generation API — sd-turbo, Rust/Axum, CPU
---

# AvaGen — AI Avatar Generation API

REST API that generates photorealistic AI avatar portraits from structured
demographic descriptions. Built with **Rust/Axum** + a **Python sd-turbo
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

Returns a **512×512 PNG**. Generation takes ~3–5 s on CPU.

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
      sd-turbo inference (~3-5 s/image on CPU)
```

Model weights (~2.5 GB) are baked into the Docker image — cold starts load
from disk in ~5–10 s without any network download.
