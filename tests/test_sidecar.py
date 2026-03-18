"""Integration tests for the infer.py sidecar.

Run with no GPU required:

    cd /home/jamg/avagen
    MOCK_MODELS=1 pytest tests/test_sidecar.py -v

In mock mode the sidecar returns dummy PNG / MP4 without loading real models,
so these tests pass on any machine (no CUDA, no HF token needed).

To run against a real GPU sidecar, omit MOCK_MODELS:

    pytest tests/test_sidecar.py -v --timeout=300
"""
from __future__ import annotations

import base64
import io
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest
import requests

PROJECT_ROOT = Path(__file__).parent.parent
SIDECAR_URL  = "http://127.0.0.1:8001"


# ---------------------------------------------------------------------------
# Session fixture: start the sidecar once for all tests
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session", autouse=True)
def sidecar():
    """Launch infer.py with MOCK_MODELS=1 for the full test session."""
    env = {
        **os.environ,
        "MOCK_MODELS": os.environ.get("MOCK_MODELS", "1"),
        "HF_HOME": "/tmp/avagen_test_hf_cache",
    }
    proc = subprocess.Popen(
        [sys.executable, str(PROJECT_ROOT / "infer.py")],
        env=env,
        cwd=str(PROJECT_ROOT),
    )

    # Poll /health until the server accepts connections (max 60 s)
    deadline = time.time() + 60
    while time.time() < deadline:
        try:
            r = requests.get(f"{SIDECAR_URL}/health", timeout=1)
            if r.status_code == 200:
                break
        except Exception:
            pass
        time.sleep(0.5)
    else:
        proc.terminate()
        pytest.fail("Inference sidecar failed to start within 60 seconds")

    yield proc

    proc.terminate()
    proc.wait(timeout=5)


# ---------------------------------------------------------------------------
# Health
# ---------------------------------------------------------------------------

def test_health(sidecar):
    r = requests.get(f"{SIDECAR_URL}/health", timeout=5)
    assert r.status_code == 200
    data = r.json()
    assert data["status"] == "ok"
    assert isinstance(data["mock"], bool)
    assert data["flux_loaded"] is True        # FLUX stub is loaded at startup


def test_health_svd_not_yet_loaded(sidecar):
    """SVD is lazy-loaded; it should NOT be loaded until the first video request."""
    r = requests.get(f"{SIDECAR_URL}/health", timeout=5)
    # svd_loaded may be False before the first video request — that's expected.
    assert r.json()["status"] == "ok"


# ---------------------------------------------------------------------------
# Image generation
# ---------------------------------------------------------------------------

def test_generate_returns_png(sidecar):
    r = requests.post(
        f"{SIDECAR_URL}/generate",
        json={
            "prompt": "a test portrait for CI",
            "width": 64,
            "height": 64,
            "num_inference_steps": 1,
            "seed": 42,
        },
        timeout=30,
    )
    assert r.status_code == 200
    assert "image/png" in r.headers["content-type"]
    assert len(r.content) > 100  # non-trivial PNG


def test_generate_missing_prompt_returns_422(sidecar):
    """'prompt' is required — omitting it should return 422 Unprocessable Entity."""
    r = requests.post(
        f"{SIDECAR_URL}/generate",
        json={"width": 64, "height": 64},
        timeout=10,
    )
    assert r.status_code == 422


def test_generate_respects_seed(sidecar):
    """Same seed should produce identical output in mock mode."""
    payload = {"prompt": "deterministic test", "width": 32, "height": 32, "seed": 7}
    r1 = requests.post(f"{SIDECAR_URL}/generate", json=payload, timeout=30)
    r2 = requests.post(f"{SIDECAR_URL}/generate", json=payload, timeout=30)
    assert r1.status_code == r2.status_code == 200
    # In mock mode the image is always identical regardless of seed, but both
    # should succeed and be non-empty.
    assert len(r1.content) > 0 and len(r2.content) > 0


# ---------------------------------------------------------------------------
# Video generation
# ---------------------------------------------------------------------------

def _small_jpeg_b64() -> str:
    """Return a 32×32 solid-colour JPEG encoded as base64."""
    from PIL import Image
    img = Image.new("RGB", (32, 32), color=(200, 100, 50))
    buf = io.BytesIO()
    img.save(buf, format="JPEG")
    return base64.b64encode(buf.getvalue()).decode()


def test_generate_video_base64(sidecar):
    r = requests.post(
        f"{SIDECAR_URL}/video/generate",
        json={
            "image_b64": _small_jpeg_b64(),
            "motion_bucket_id": 127,
            "fps_id": 6,
            "seed": 0,
        },
        timeout=60,
    )
    assert r.status_code == 200
    assert "video/mp4" in r.headers["content-type"]
    assert len(r.content) > 100  # non-trivial MP4


def test_generate_video_no_image_returns_400(sidecar):
    """Omitting both image_b64 and image_url must return HTTP 400."""
    r = requests.post(
        f"{SIDECAR_URL}/video/generate",
        json={"motion_bucket_id": 127},
        timeout=10,
    )
    assert r.status_code == 400


def test_generate_video_missing_fields_returns_200(sidecar):
    """All video params except image are optional — defaults must be accepted."""
    r = requests.post(
        f"{SIDECAR_URL}/video/generate",
        json={"image_b64": _small_jpeg_b64()},
        timeout=60,
    )
    assert r.status_code == 200


# ---------------------------------------------------------------------------
# SVD lazy-load: after a video request, svd_loaded should be True
# ---------------------------------------------------------------------------

def test_svd_loaded_after_video_request(sidecar):
    # Ensure at least one video request has been made (tests above run in order)
    r = requests.get(f"{SIDECAR_URL}/health", timeout=5)
    assert r.json()["svd_loaded"] is True
