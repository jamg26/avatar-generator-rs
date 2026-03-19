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
    "diffusers>=0.31" \
    "transformers>=4.39" \
    "accelerate>=0.27" \
    "optimum-intel[openvino]" \
    fastapi \
    "uvicorn[standard]" \
    pydantic \
    Pillow \
    protobuf \
    huggingface_hub

# FLUX.1-schnell OpenVINO INT4 weights are downloaded at first startup and cached.
# ~5 s per image on CPU using AVX-512 / OpenVINO acceleration.
ENV HF_HOME=/app/model_cache

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
    SD_MODEL_REPO=OpenVINO/stable-diffusion-v1-5-fp16-ov \
    SD_NUM_STEPS=20 \
    SD_GUIDANCE_SCALE=7.5 \
    SD_DEFAULT_SIZE=512 \
    SKIP_VIDEO_PIPELINE=1

EXPOSE 7860

CMD ["./start.sh"]
