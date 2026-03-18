"""Local inference sidecar for AvaGen.

Runs as a subprocess inside the Modal container alongside the Rust Axum server.
Exposes a local HTTP API on 127.0.0.1:8001 — the Rust binary calls these
endpoints instead of the remote HuggingFace Inference API.

Endpoints:
  GET  /health          → {"status": "ok", "mock": bool, "flux_loaded": bool, "svd_loaded": bool}
  POST /generate        → raw PNG bytes  (FLUX.1-schnell text-to-image)
  POST /video/generate  → raw MP4 bytes  (SVD XT image-to-video)

Environment variables:
  MOCK_MODELS=1           Skip loading real models; return minimal stub outputs.
                          Use this for local integration tests without a GPU.
  HF_HOME=/path           Override the HuggingFace model cache directory.
  HF_TOKEN or HUGGING_FACE_HUB_TOKEN
                          Token for downloading gated models from HuggingFace Hub.
  SD_MODEL_REPO           FLUX model repo  (default: black-forest-labs/FLUX.1-schnell).
  VIDEO_MODEL_REPO        SVD model repo   (default: stabilityai/stable-video-diffusion-img2vid-xt).
"""
from __future__ import annotations

import base64
import io
import logging
import os
import threading
from contextlib import asynccontextmanager
from typing import Optional

import uvicorn
from fastapi import FastAPI, HTTPException
from fastapi.responses import Response
from pydantic import BaseModel

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s — %(message)s",
)
log = logging.getLogger("infer")

# ---------------------------------------------------------------------------
# Environment setup
# ---------------------------------------------------------------------------

# Accept both HF_TOKEN (project convention) and HUGGING_FACE_HUB_TOKEN (HF Hub convention).
_hf = os.environ.get("HUGGING_FACE_HUB_TOKEN") or os.environ.get("HF_TOKEN")
if _hf:
    os.environ["HUGGING_FACE_HUB_TOKEN"] = _hf

MOCK        = os.environ.get("MOCK_MODELS", "0") in ("1", "true", "yes")
HF_HOME     = os.environ.get("HF_HOME", "/cache/hf")
SD_MODEL    = os.environ.get("SD_MODEL_REPO",    "black-forest-labs/FLUX.1-schnell")
VIDEO_MODEL = os.environ.get("VIDEO_MODEL_REPO", "stabilityai/stable-video-diffusion-img2vid-xt")

# ---------------------------------------------------------------------------
# Global model holders
# ---------------------------------------------------------------------------

_flux_pipe = None
_svd_pipe  = None
_svd_lock  = threading.Lock()

# ---------------------------------------------------------------------------
# Model loaders
# ---------------------------------------------------------------------------

def _load_flux() -> None:
    global _flux_pipe
    if MOCK:
        log.info("MOCK_MODELS=1 — FLUX stub active (no GPU required)")
        _flux_pipe = "mock"
        return

    import torch
    from diffusers import FluxPipeline

    log.info(f"Loading FLUX model: {SD_MODEL} …")
    pipe = FluxPipeline.from_pretrained(
        SD_MODEL,
        torch_dtype=torch.bfloat16,
        cache_dir=os.path.join(HF_HOME, "hub"),
    )
    # Sequential offload keeps only the active layer on-GPU — essential because
    # the FLUX transformer (12 B params × 2 B = ~24 GiB) exceeds the A10G's
    # 22 GiB; model_cpu_offload loads entire sub-modules which still OOMs.
    pipe.enable_sequential_cpu_offload()
    pipe.enable_attention_slicing("max")
    pipe.vae.enable_tiling()
    pipe.vae.enable_slicing()
    _flux_pipe = pipe
    log.info("FLUX pipeline ready")


def _load_svd() -> None:
    global _svd_pipe
    if MOCK:
        log.info("MOCK_MODELS=1 — SVD stub active (no GPU required)")
        _svd_pipe = "mock"
        return

    import torch
    from diffusers import StableVideoDiffusionPipeline

    log.info(f"Loading SVD model: {VIDEO_MODEL} …")
    pipe = StableVideoDiffusionPipeline.from_pretrained(
        VIDEO_MODEL,
        torch_dtype=torch.float16,
        variant="fp16",
        cache_dir=os.path.join(HF_HOME, "hub"),
    )
    pipe.enable_sequential_cpu_offload()
    pipe.enable_attention_slicing("max")
    pipe.vae.enable_tiling()
    pipe.vae.enable_slicing()
    _svd_pipe = pipe
    log.info("SVD XT pipeline ready")


# ---------------------------------------------------------------------------
# App lifecycle
# ---------------------------------------------------------------------------

@asynccontextmanager
async def lifespan(app: FastAPI):
    _load_flux()
    yield


app = FastAPI(title="AvaGen Inference Sidecar", lifespan=lifespan)


# ---------------------------------------------------------------------------
# Request schemas
# ---------------------------------------------------------------------------

class GenerateRequest(BaseModel):
    prompt: str
    negative_prompt: str = ""
    width: int = 1024
    height: int = 1024
    # FLUX.1-schnell is optimised for 4 steps; guidance_scale=0.0 (distilled, no CFG)
    num_inference_steps: int = 4
    guidance_scale: float = 0.0
    seed: int = 0


class VideoGenerateRequest(BaseModel):
    image_b64: Optional[str] = None
    image_url: Optional[str] = None
    motion_bucket_id: int = 127
    noise_aug_strength: float = 0.02
    fps_id: int = 6
    seed: int = 0


# ---------------------------------------------------------------------------
# Video encoding helper
# ---------------------------------------------------------------------------

def _frames_to_mp4(frames: list, fps: int) -> bytes:
    """Encode a list of PIL Images (or H×W×3 numpy arrays) to an in-memory MP4."""
    import av
    import numpy as np

    buf = io.BytesIO()
    container = av.open(buf, mode="w", format="mp4")

    first = np.array(frames[0]) if not isinstance(frames[0], np.ndarray) else frames[0]
    h, w = first.shape[:2]

    stream = container.add_stream("h264", rate=fps)
    stream.width  = w
    stream.height = h
    stream.pix_fmt = "yuv420p"
    stream.options = {"crf": "23", "preset": "fast"}

    for frame in frames:
        arr = np.array(frame) if not isinstance(frame, np.ndarray) else frame
        vf = av.VideoFrame.from_ndarray(arr, format="rgb24").reformat(format="yuv420p")
        for pkt in stream.encode(vf):
            container.mux(pkt)

    for pkt in stream.encode():
        container.mux(pkt)

    container.close()
    buf.seek(0)
    return buf.read()


# ---------------------------------------------------------------------------
# Endpoints
# ---------------------------------------------------------------------------

@app.get("/health")
def health():
    return {
        "status":      "ok",
        "mock":        MOCK,
        "flux_loaded": _flux_pipe is not None,
        "svd_loaded":  _svd_pipe  is not None,
    }


@app.post("/generate")
def generate(req: GenerateRequest) -> Response:
    if _flux_pipe is None:
        raise HTTPException(503, "FLUX model not loaded yet — retry shortly")

    if _flux_pipe == "mock":
        from PIL import Image
        img = Image.new("RGB", (req.width, req.height), color=(40, 40, 50))
        buf = io.BytesIO()
        img.save(buf, format="PNG")
        log.info(f"[mock] /generate {req.width}×{req.height} PNG")
        return Response(content=buf.getvalue(), media_type="image/png")

    import torch
    torch.cuda.empty_cache()
    generator = torch.Generator().manual_seed(req.seed)
    result = _flux_pipe(
        prompt=req.prompt,
        negative_prompt=req.negative_prompt or None,
        width=req.width,
        height=req.height,
        num_inference_steps=req.num_inference_steps,
        guidance_scale=req.guidance_scale,
        generator=generator,
        output_type="pil",
    )
    img = result.images[0]
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    log.info(f"Generated {req.width}×{req.height} image")
    return Response(content=buf.getvalue(), media_type="image/png")


@app.post("/video/generate")
def generate_video(req: VideoGenerateRequest) -> Response:
    # Validate inputs early before any model/GPU code so we can return 400 cleanly.
    if not req.image_b64 and not req.image_url:
        raise HTTPException(400, "Provide image_b64 or image_url")

    global _svd_pipe

    # SVD is lazy-loaded on first request to keep idle VRAM footprint low.
    with _svd_lock:
        if _svd_pipe is None:
            _load_svd()

    if _svd_pipe == "mock":
        import numpy as np
        frame = np.zeros((576, 1024, 3), dtype=np.uint8)
        mp4 = _frames_to_mp4([frame], req.fps_id)
        log.info("[mock] /video/generate — single black frame")
        return Response(content=mp4, media_type="video/mp4")

    from PIL import Image
    import torch

    if req.image_b64:
        raw = base64.b64decode(req.image_b64)
        img = Image.open(io.BytesIO(raw)).convert("RGB")
    elif req.image_url:
        import urllib.request
        with urllib.request.urlopen(req.image_url, timeout=30) as resp:
            img = Image.open(io.BytesIO(resp.read())).convert("RGB")
    else:
        raise HTTPException(400, "Provide image_b64 or image_url")

    img = img.resize((1024, 576))  # SVD XT fixed input resolution
    generator = torch.Generator().manual_seed(req.seed)

    torch.cuda.empty_cache()
    with torch.no_grad():
        output = _svd_pipe(
            img,
            motion_bucket_id=req.motion_bucket_id,
            noise_aug_strength=req.noise_aug_strength,
            fps=req.fps_id,
            generator=generator,
            output_type="pil",
            decode_chunk_size=2,
        )

    frames = output.frames[0]
    mp4 = _frames_to_mp4(frames, req.fps_id)
    log.info(f"Generated video: {len(frames)} frames @ {req.fps_id} fps")
    return Response(content=mp4, media_type="video/mp4")


if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=8001, log_level="info")
