# ── Stage 1: Build the Rust binary ─────────────────────────────────────────
FROM rust:1-slim AS builder
WORKDIR /build

RUN apt-get update && apt-get install -y pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Cache dependency compilation separately from source changes.
# A dummy main.rs lets cargo fetch+compile all deps without the real source.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs && \
    cargo build --release 2>/dev/null; \
    rm -f target/release/deps/avagen*

# Now compile with the real source
COPY src ./src
RUN cargo build --release

# ── Stage 2: Python runtime + pre-cached model ──────────────────────────────
FROM python:3.11-slim
WORKDIR /app

RUN apt-get update && apt-get install -y \
    libssl3 ca-certificates libgomp1 && \
    rm -rf /var/lib/apt/lists/*

# CPU-only PyTorch first (large wheel — separate layer for better caching)
RUN pip install --no-cache-dir \
    torch torchvision \
    --extra-index-url https://download.pytorch.org/whl/cpu

# Inference + sidecar API stack
RUN pip install --no-cache-dir \
    "diffusers>=0.27" \
    "transformers>=4.39" \
    "accelerate>=0.27" \
    fastapi \
    "uvicorn[standard]" \
    pydantic \
    Pillow \
    sentencepiece \
    protobuf \
    huggingface_hub

# Pre-download LCM_Dreamshaper_v7 weights into this image layer so cold starts are fast
# (~2 GB public model; LCM-distilled SD1.5, 4-step inference, good face quality)
ENV HF_HOME=/app/model_cache
RUN python3 -c "\
import torch; \
from diffusers import AutoPipelineForText2Image; \
AutoPipelineForText2Image.from_pretrained( \
    'SimianLuo/LCM_Dreamshaper_v7', \
    torch_dtype=torch.float32, \
    cache_dir='/app/model_cache/hub' \
)"

# Copy Rust binary from the builder stage
COPY --from=builder /build/target/release/avagen ./avagen
RUN chmod +x ./avagen

# Copy Python inference sidecar
COPY infer.py ./infer.py

# Startup script
COPY start.sh ./start.sh
RUN chmod +x ./start.sh

# ── Runtime environment ──────────────────────────────────────────────────────
# PORT=7860 matches HF Spaces' default app_port
ENV PORT=7860 \
    RUST_LOG=avagen=info,tower_http=info \
    HF_HOME=/app/model_cache \
    SD_MODEL_REPO=SimianLuo/LCM_Dreamshaper_v7 \
    SD_NUM_STEPS=4 \
    SD_GUIDANCE_SCALE=8.0 \
    SD_USE_LCM=1 \
    SD_DEFAULT_SIZE=512 \
    SKIP_VIDEO_PIPELINE=1

EXPOSE 7860

CMD ["./start.sh"]
