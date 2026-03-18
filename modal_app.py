"""
Modal deployment for AvaGen — AI Avatar Generation API.

Architecture: CPU-only container running two processes side-by-side:
  • Rust / Axum server  — external HTTPS traffic on port 8080
                          (API keys, rate-limiting, DB, response encoding)
  • Python sidecar       — local inference on port 8001
                          (sd-turbo text→image, ~2-4 s per image)

The Rust binary calls localhost:8001 for all generation work.
Model weights (~2.5 GB) are downloaded on first cold start and cached in a
Modal Volume so subsequent starts skip the download (~5–10 s to load).

Deploy:
    modal deploy modal_app.py

Serve (ephemeral / test):
    modal serve modal_app.py

Test with mock models (no HF token needed):
    MOCK_MODELS=1 modal serve modal_app.py
"""

import modal

app = modal.App("avagen")

# ---------------------------------------------------------------------------
# Modal Volume — persists HF model weights across container restarts.
#
# First cold start: ~5 min to download sd-turbo (~2.5 GB)
# Subsequent starts: weights served from Volume, loading in ~5–10 s.
# ---------------------------------------------------------------------------
model_cache = modal.Volume.from_name("avagen-model-cache", create_if_missing=True)

# ---------------------------------------------------------------------------
# Container image — Python 3.11 + CPU PyTorch + diffusers stack
# ---------------------------------------------------------------------------
image = (
    modal.Image.debian_slim(python_version="3.11")
    .apt_install("libssl3", "ca-certificates", "libgomp1")
    # CPU-only PyTorch — smaller image, no GPU driver requirements
    .pip_install(
        "torch",
        "torchvision",
        extra_options="--extra-index-url https://download.pytorch.org/whl/cpu",
    )
    # Inference stack + sidecar API server
    .pip_install(
        "diffusers>=0.27",
        "transformers>=4.39",
        "accelerate>=0.27",
        "sentencepiece",
        "protobuf",
        "fastapi>=0.109",
        "uvicorn[standard]>=0.27",
        "pydantic>=2.5",
        "Pillow>=10",
    )
    # Bake the pre-built Rust binary
    .add_local_file(
        local_path="target/release/avagen",
        remote_path="/app/avagen",
        copy=True,
    )
    # Bake the inference sidecar
    .add_local_file(
        local_path="infer.py",
        remote_path="/app/infer.py",
        copy=True,
    )
    .run_commands("chmod +x /app/avagen")
    .workdir("/app")
)

# ---------------------------------------------------------------------------
# Web server function
# ---------------------------------------------------------------------------
@app.function(
    image=image,
    secrets=[modal.Secret.from_dotenv()],
    volumes={"/cache": model_cache},
    # Keep 1 container always warm so models stay loaded
    min_containers=1,
    # sd-turbo on CPU takes ~3–5 s per image; 300 s is plenty of headroom
    timeout=300,
    # Enough RAM for sd-turbo (~2.5 GB model + working memory)
    memory=8192,
)
@modal.web_server(8080, startup_timeout=120.0)
def serve():
    """
    Launch the Python inference sidecar (port 8001) then the Rust Axum server (port 8080).
    Modal routes all external HTTPS traffic to port 8080.

    The sidecar loads sd-turbo from the Volume cache on startup (~5–10 s after first download).
    Generation requests that arrive before the model is ready receive a 503 and
    should be retried — this only affects cold starts, not warm containers.
    """
    import os
    import subprocess

    sidecar_env = {
        **os.environ,
        # Point HF Hub at the Modal Volume so weights are cached across restarts
        "HF_HOME": "/cache/hf",
        "TRANSFORMERS_CACHE": "/cache/hf",
        # Normalise both token conventions — HF Hub prefers HUGGING_FACE_HUB_TOKEN
        "HUGGING_FACE_HUB_TOKEN": os.environ.get("HF_TOKEN", ""),
        # sd-turbo settings
        "SD_MODEL_REPO": "stabilityai/sd-turbo",
        "SD_NUM_STEPS": "1",
        "SD_GUIDANCE_SCALE": "0.0",
        "SD_DEFAULT_SIZE": "512",
    }

    # Start the inference sidecar — model loading happens asynchronously inside
    subprocess.Popen(["python3", "/app/infer.py"], env=sidecar_env)

    # Start the Rust server — ready immediately on port 8080; returns 503 for
    # generation endpoints until the sidecar finishes loading its models
    subprocess.Popen(["/app/avagen"], env=os.environ)
