"""Local inference sidecar for AvaGen.

Runs as a subprocess inside the Modal container (or locally) alongside the
Rust Axum server. Exposes a local HTTP API on 127.0.0.1:8001.

Endpoints:
  GET  /health          → {"status": "ok", "mock": bool, "flux_loaded": bool, "svd_loaded": bool}
  POST /generate        → raw PNG bytes  (FLUX.1-schnell OpenVINO INT4, ~5 s on CPU)

Environment variables:
  MOCK_MODELS=1           Skip loading real models; return minimal stub outputs.
  HF_HOME=/path           Override the HuggingFace model cache directory.
  HF_TOKEN or HUGGING_FACE_HUB_TOKEN
                          Token for downloading models from HuggingFace Hub.
  SD_MODEL_REPO           OpenVINO INT4 FLUX repo (default: rupeshs/FLUX.1-schnell-openvino-int4).
"""
from __future__ import annotations

import base64
import io
import logging
import os
import threading
from contextlib import asynccontextmanager
from typing import Any, Optional

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

# Suppress tqdm progress bars — in non-TTY environments (Docker, piped logs)
# tqdm prints the final "100%" line twice (once on completion, once on close).
os.environ.setdefault("TQDM_DISABLE", "1")

# Auto-load .env from the project root (same directory as this file) so that
# `python infer.py` works without manually exporting env vars first.
_env_file = os.path.join(os.path.dirname(__file__) or ".", ".env")
if os.path.exists(_env_file):
    with open(_env_file) as _f:
        for _line in _f:
            _line = _line.strip()
            if _line and not _line.startswith("#") and "=" in _line:
                _k, _, _v = _line.partition("=")
                os.environ.setdefault(_k.strip(), _v.strip())

# Accept both HF_TOKEN (project convention) and HUGGING_FACE_HUB_TOKEN (HF Hub convention).
_hf = os.environ.get("HUGGING_FACE_HUB_TOKEN") or os.environ.get("HF_TOKEN")
if _hf:
    os.environ["HUGGING_FACE_HUB_TOKEN"] = _hf

MOCK        = os.environ.get("MOCK_MODELS", "0") in ("1", "true", "yes")
HF_HOME     = os.environ.get("HF_HOME",     os.path.expanduser("~/.cache/huggingface"))
SD_MODEL    = os.environ.get("SD_MODEL_REPO", "rupeshs/FLUX.1-schnell-openvino-int4")
VIDEO_MODEL = os.environ.get("VIDEO_MODEL_REPO", "stabilityai/stable-video-diffusion-img2vid-xt")

# ---------------------------------------------------------------------------
# Global model holders
# ---------------------------------------------------------------------------

_flux_pipe: Any = None
_svd_pipe: Any = None
_pipe_lock = threading.Lock()   # serialise SD pipeline calls — scheduler is not thread-safe
_svd_lock  = threading.Lock()

# ---------------------------------------------------------------------------
# Model loaders
# ---------------------------------------------------------------------------

def _ensure_ov_symlinks(local_dir: str) -> None:
    """rupeshs/FLUX.1-schnell-openvino-int4 uses non-standard OV filenames.

    OVFluxPipeline._search_pattern = r'(.*)?openvino(.*)?_(.*)?.xml$' so it only
    recognises files containing 'openvino_' in the name.  Standard components ship
    as <name>/<name>.xml; the VAE lives in vae/vae_decoder.xml instead of the
    expected vae_decoder/openvino_model.xml.  Create symlinks once.
    """
    import shutil

    # transformer / text_encoder / text_encoder_2
    for comp in ("transformer", "text_encoder", "text_encoder_2"):
        comp_dir = os.path.join(local_dir, comp)
        for ext in (".xml", ".bin"):
            src = os.path.join(comp_dir, f"{comp}{ext}")
            dst = os.path.join(comp_dir, f"openvino_model{ext}")
            if os.path.exists(src) and not os.path.exists(dst):
                os.symlink(src, dst)

    # vae_decoder: rupeshs puts it in vae/vae_decoder.{xml,bin}
    # OVFluxPipeline expects vae_decoder/openvino_model.{xml,bin}
    vae_decoder_dir = os.path.join(local_dir, "vae_decoder")
    os.makedirs(vae_decoder_dir, exist_ok=True)
    for ext in (".xml", ".bin"):
        src = os.path.join(local_dir, "vae", f"vae_decoder{ext}")
        dst = os.path.join(vae_decoder_dir, f"openvino_model{ext}")
        if os.path.exists(src) and not os.path.exists(dst):
            os.symlink(src, dst)
    # config.json in vae_decoder/ so the pipeline can read model metadata
    vae_cfg_src = os.path.join(local_dir, "vae", "config.json")
    vae_cfg_dst = os.path.join(vae_decoder_dir, "config.json")
    if os.path.exists(vae_cfg_src) and not os.path.exists(vae_cfg_dst):
        shutil.copy2(vae_cfg_src, vae_cfg_dst)


def _patch_ov_text_encoder() -> None:
    """Fix OVModelTextEncoder.forward for unnamed OV output tensors.

    The rupeshs INT4 CLIP text_encoder model exports some outputs without tensor
    names. optimum-intel's forward calls output.get_any_name() unconditionally,
    raising RuntimeError. Monkey-patch the class with a safe version.
    """
    import torch
    from transformers.utils import ModelOutput
    from optimum.intel.openvino.modeling_diffusion import OVModelTextEncoder

    def _safe_forward(
        self,
        input_ids,
        attention_mask=None,
        output_hidden_states=None,
        return_dict=False,
    ):
        def _name(out):
            try:
                return out.get_any_name()
            except RuntimeError:
                return ""

        self.compile()
        model_inputs = {"input_ids": input_ids}
        if "attention_mask" in self.input_names:
            model_inputs["attention_mask"] = attention_mask

        ov_outputs = self.request(model_inputs, share_inputs=True)
        model_outputs = {}

        name0 = _name(self.model.outputs[0])
        model_outputs[name0 or "last_hidden_state"] = torch.from_numpy(ov_outputs[0])

        if len(self.model.outputs) > 1:
            name1 = _name(self.model.outputs[1])
            # When output has no name (rupeshs INT4 CLIP), assume standard CLIP
            # convention: outputs = [last_hidden_state, pooler_output].
            if "pooler_output" in name1 or not name1:
                model_outputs["pooler_output"] = torch.from_numpy(ov_outputs[1])

        if self.hidden_states_output_names and "last_hidden_state" not in model_outputs:
            model_outputs["last_hidden_state"] = torch.from_numpy(
                ov_outputs[self.hidden_states_output_names[-1]]
            )
        if (
            self.hidden_states_output_names
            and output_hidden_states
            or getattr(self.config, "output_hidden_states", False)
        ):
            model_outputs["hidden_states"] = [
                torch.from_numpy(ov_outputs[n]) for n in self.hidden_states_output_names
            ]

        if return_dict:
            return model_outputs
        return ModelOutput(**model_outputs)

    OVModelTextEncoder.forward = _safe_forward


def _load_flux() -> None:
    global _flux_pipe
    if MOCK:
        log.info("MOCK_MODELS=1 — SD stub active (no GPU required)")
        _flux_pipe = "mock"
        return

    import json
    from huggingface_hub import snapshot_download
    from optimum.intel import OVFluxPipeline  # type: ignore[import]

    cache_dir = os.path.join(HF_HOME, "hub")
    log.info(f"Downloading FLUX.1-schnell OpenVINO INT4 weights: {SD_MODEL} …")
    local_dir = snapshot_download(SD_MODEL, cache_dir=cache_dir)

    # ── Fix 1: inject model_index.json (repo ships none; breaks library detect) ──
    model_index_path = os.path.join(local_dir, "model_index.json")
    if not os.path.exists(model_index_path):
        log.info("Writing model_index.json to local snapshot …")
        model_index = {
            "_class_name": "FluxPipeline",
            "_diffusers_version": "0.30.0",
            "scheduler":      ["diffusers",    "FlowMatchEulerDiscreteScheduler"],
            "text_encoder":   ["transformers", "CLIPTextModel"],
            "text_encoder_2": ["transformers", "T5EncoderModel"],
            "tokenizer":      ["transformers", "CLIPTokenizer"],
            "tokenizer_2":    ["transformers", "T5TokenizerFast"],
            "transformer":    ["diffusers",    "FluxTransformer2DModel"],
            # Note: no "vae" key — OVFluxPipeline loads vae_decoder via
            # _all_ov_model_paths, not model_index.json.
        }
        with open(model_index_path, "w") as f:
            json.dump(model_index, f, indent=2)

    # ── Fix 2: create openvino_model.xml symlinks ─────────────────────────────
    # OVFluxPipeline._search_pattern requires "openvino_*.xml" filenames but this
    # repo uses "transformer/transformer.xml", "text_encoder/text_encoder.xml", etc.
    # OVFluxPipeline._all_ov_model_paths also expects "vae_decoder/openvino_model.xml",
    # not "vae/vae_decoder.xml".  Create symlinks once so optimum finds everything.
    _ensure_ov_symlinks(local_dir)
    _patch_ov_text_encoder()  # Fix 3: handle unnamed OV output tensors

    log.info(f"Loading pipeline from: {local_dir}")
    # dynamic_shapes=False: rupeshs INT4 model has static shapes baked in;
    # dynamic reshape (the default) tries to set shape[1] on 1-D inputs → RuntimeError.
    pipe = OVFluxPipeline.from_pretrained(local_dir, dynamic_shapes=False)

    _flux_pipe = pipe
    log.info("FLUX.1-schnell OpenVINO INT4 pipeline ready")


def _load_svd() -> None:
    global _svd_pipe
    if MOCK:
        log.info("MOCK_MODELS=1 — SVD stub active (no GPU required)")
        _svd_pipe = "mock"
        return

    import torch
    from diffusers.pipelines.stable_video_diffusion.pipeline_stable_video_diffusion import StableVideoDiffusionPipeline

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
    import traceback
    try:
        _load_flux()
    except Exception:
        log.error(f"FATAL: Failed to load FLUX pipeline:\n{traceback.format_exc()}")
        raise
    yield


app = FastAPI(title="AvaGen Inference Sidecar", lifespan=lifespan)


# ---------------------------------------------------------------------------
# Request schemas
# ---------------------------------------------------------------------------

class GenerateRequest(BaseModel):
    prompt: str
    negative_prompt: str = ""
    width: int = 512
    height: int = 512
    # FLUX.1-schnell OpenVINO INT4: 2 steps, guidance_scale=1.0
    num_inference_steps: int = 2
    guidance_scale: float = 1.0
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
        raise HTTPException(503, "Model not loaded yet — retry shortly")

    if _flux_pipe == "mock":
        from PIL import Image
        img = Image.new("RGB", (req.width, req.height), color=(40, 40, 50))
        buf = io.BytesIO()
        img.save(buf, format="PNG")
        log.info(f"[mock] /generate {req.width}×{req.height} PNG")
        return Response(content=buf.getvalue(), media_type="image/png")

    import traceback
    import torch
    generator = torch.Generator().manual_seed(req.seed)
    # FLUX does not use negative_prompt
    kwargs: dict = dict(
        prompt=req.prompt,
        width=req.width,
        height=req.height,
        num_inference_steps=req.num_inference_steps,
        guidance_scale=req.guidance_scale,
        generator=generator,
        output_type="pil",
    )
    try:
        with _pipe_lock:
            result = _flux_pipe(**kwargs)
    except Exception as exc:
        tb = traceback.format_exc()
        log.error(f"Inference error:\n{tb}")
        raise HTTPException(status_code=500, detail=tb)
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
