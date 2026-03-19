"""AvaGen inference sidecar — cartoon avatar generation via py-avataaars.

Generates structured avatar traits as a cartoon-style portrait PNG in <1 second.
No ML models required — uses SVG compositing (py-avataaars + cairosvg).

Endpoints:
  GET  /health    → {"status": "ok", ...}
  POST /generate  → raw PNG bytes  (<1 s, CPU-only)
"""
from __future__ import annotations

import io
import logging
import os
from typing import Any, Dict, List, Optional

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

_env_file = os.path.join(os.path.dirname(__file__) or ".", ".env")
if os.path.exists(_env_file):
    with open(_env_file) as _f:
        for _line in _f:
            _line = _line.strip()
            if _line and not _line.startswith("#") and "=" in _line:
                _k, _, _v = _line.partition("=")
                os.environ.setdefault(_k.strip(), _v.strip())

MOCK = os.environ.get("MOCK_MODELS", "0") in ("1", "true", "yes")

# ---------------------------------------------------------------------------
# py-avataaars trait mappings
# ---------------------------------------------------------------------------

import py_avataaars as pa
from PIL import Image

SKIN_COLOR_MAP: Dict[str, Any] = {
    "very_light":   pa.SkinColor.PALE,
    "light":        pa.SkinColor.LIGHT,
    "medium_light": pa.SkinColor.TANNED,
    "medium":       pa.SkinColor.YELLOW,
    "medium_dark":  pa.SkinColor.BROWN,
    "dark":         pa.SkinColor.DARK_BROWN,
    "very_dark":    pa.SkinColor.BLACK,
}

ETHNICITY_SKIN_MAP: Dict[str, Any] = {
    "caucasian":        pa.SkinColor.LIGHT,
    "african":          pa.SkinColor.DARK_BROWN,
    "east_asian":       pa.SkinColor.YELLOW,
    "south_asian":      pa.SkinColor.BROWN,
    "southeast_asian":  pa.SkinColor.TANNED,
    "hispanic":         pa.SkinColor.BROWN,
    "middle_eastern":   pa.SkinColor.TANNED,
    "native_american":  pa.SkinColor.BROWN,
    "pacific_islander": pa.SkinColor.TANNED,
    "mixed":            pa.SkinColor.TANNED,
}

HAIR_COLOR_MAP: Dict[str, Any] = {
    "black":             pa.HairColor.BLACK,
    "brown":             pa.HairColor.BROWN,
    "blonde":            pa.HairColor.BLONDE_GOLDEN,
    "red":               pa.HairColor.RED,
    "gray":              pa.HairColor.SILVER_GRAY,
    "white":             pa.HairColor.PLATINUM,
    "auburn":            pa.HairColor.AUBURN,
    "strawberry_blonde": pa.HairColor.BLONDE,
}

HAIR_STYLE_MAP: Dict[str, Any] = {
    "bald":         pa.TopType.NO_HAIR,
    "buzz_cut":     pa.TopType.SHORT_HAIR_THE_CAESAR,
    "short":        pa.TopType.SHORT_HAIR_SHORT_ROUND,
    "medium":       pa.TopType.SHORT_HAIR_SHORT_WAVED,
    "long_straight": pa.TopType.LONG_HAIR_STRAIGHT,
    "long_wavy":    pa.TopType.LONG_HAIR_CURVY,
    "long_curly":   pa.TopType.LONG_HAIR_CURLY,
    "afro":         pa.TopType.LONG_HAIR_FRO,
    "braids":       pa.TopType.LONG_HAIR_DREADS,
    "ponytail":     pa.TopType.LONG_HAIR_BOB,
    "bun":          pa.TopType.LONG_HAIR_BUN,
    "mohawk":       pa.TopType.SHORT_HAIR_SIDES,
    "dreadlocks":   pa.TopType.SHORT_HAIR_DREADS_01,
}

MOUTH_MAP: Dict[str, Any] = {
    "neutral":    pa.MouthType.SERIOUS,
    "happy":      pa.MouthType.SMILE,
    "serious":    pa.MouthType.DEFAULT,
    "confident":  pa.MouthType.TWINKLE,
    "friendly":   pa.MouthType.TONGUE,
    "thoughtful": pa.MouthType.CONCERNED,
    "surprised":  pa.MouthType.SCREAM_OPEN,
}

FACIAL_HAIR_MAP: Dict[str, Any] = {
    "none":       pa.FacialHairType.DEFAULT,
    "stubble":    pa.FacialHairType.BEARD_LIGHT,
    "mustache":   pa.FacialHairType.MOUSTACHE_FANCY,
    "goatee":     pa.FacialHairType.BEARD_LIGHT,
    "full_beard": pa.FacialHairType.BEARD_MEDIUM,
    "long_beard": pa.FacialHairType.BEARD_MAJESTIC,
}

_FHC_MAP: Dict[Any, Any] = {
    pa.HairColor.AUBURN:        pa.FacialHairColor.AUBURN,
    pa.HairColor.BLACK:         pa.FacialHairColor.BLACK,
    pa.HairColor.BLONDE_GOLDEN: pa.FacialHairColor.BLONDE_GOLDEN,
    pa.HairColor.BLONDE:        pa.FacialHairColor.BLONDE,
    pa.HairColor.BROWN:         pa.FacialHairColor.BROWN,
    pa.HairColor.BROWN_DARK:    pa.FacialHairColor.BROWN_DARK,
    pa.HairColor.PASTEL_PINK:   pa.FacialHairColor.PASTEL_PINK,
    pa.HairColor.PLATINUM:      pa.FacialHairColor.PLATINUM,
    pa.HairColor.RED:           pa.FacialHairColor.RED,
    pa.HairColor.SILVER_GRAY:   pa.FacialHairColor.SILVER_GRAY,
}

ACCESSORIES_MAP: Dict[str, Any] = {
    "glasses":    pa.AccessoriesType.PRESCRIPTION_01,
    "sunglasses": pa.AccessoriesType.SUNGLASSES,
    "hat":        pa.AccessoriesType.KURT,
}

BACKGROUND_COLOR_MAP: Dict[str, tuple] = {
    "white":       (255, 255, 255),
    "gray":        (180, 180, 185),
    "blue":        (100, 140, 200),
    "gradient":    (210, 185, 235),
    "nature":      (120, 170, 110),
    "studio":      (50,  55,  70),
    "studio_grey": (100, 100, 105),
}

# ---------------------------------------------------------------------------
# Avatar builder
# ---------------------------------------------------------------------------

def _build_avatar_png(traits: Dict[str, Any], out_size: int) -> bytes:
    """Map AvaGen traits → py_avataaars SVG → composite PNG bytes in <1 s."""
    import cairosvg

    t = traits

    # Skin
    skin_raw = t.get("skin_tone") or ""
    skin = (
        SKIN_COLOR_MAP.get(skin_raw)
        or ETHNICITY_SKIN_MAP.get(t.get("ethnicity", "caucasian"), pa.SkinColor.LIGHT)
    )

    # Hair
    hair_color = HAIR_COLOR_MAP.get(t.get("hair_color", "brown"), pa.HairColor.BROWN)
    top_type   = HAIR_STYLE_MAP.get(t.get("hair_style", "medium"), pa.TopType.SHORT_HAIR_SHORT_WAVED)

    # Mouth
    mouth = MOUTH_MAP.get(t.get("expression", "neutral"), pa.MouthType.DEFAULT)

    # Facial hair
    fh_type  = FACIAL_HAIR_MAP.get(t.get("facial_hair", "none"), pa.FacialHairType.DEFAULT)
    fh_color = _FHC_MAP.get(hair_color, pa.FacialHairColor.BROWN)

    # Accessories (use first recognised one)
    acc = pa.AccessoriesType.DEFAULT
    for name in t.get("accessories", []):
        if name in ACCESSORIES_MAP:
            acc = ACCESSORIES_MAP[name]
            break

    # Clothes — simple sex-based choice
    clothe = (
        pa.ClotheType.OVERALL
        if t.get("sex", "male") == "female"
        else pa.ClotheType.SHIRT_CREW_NECK
    )

    avatar = pa.PyAvataaar(
        style=pa.AvatarStyle.TRANSPARENT,
        skin_color=skin,
        hair_color=hair_color,
        facial_hair_type=fh_type,
        facial_hair_color=fh_color,
        top_type=top_type,
        hat_color=pa.Color.BLACK,
        mouth_type=mouth,
        eye_type=pa.EyeType.DEFAULT,
        eyebrow_type=pa.EyebrowType.DEFAULT_NATURAL,
        nose_type=pa.NoseType.DEFAULT,
        accessories_type=acc,
        clothe_type=clothe,
        clothe_color=pa.Color.HEATHER,
        clothe_graphic_type=pa.ClotheGraphicType.BAT,
    )

    # Render SVG → PNG in memory (no disk I/O)
    png_bytes = cairosvg.svg2png(
        bytestring=avatar._render(),
        output_width=out_size,
        output_height=out_size,
    )

    # Composite transparent avatar PNG on solid background
    bg_color = BACKGROUND_COLOR_MAP.get(t.get("background", "white"), (255, 255, 255))
    av_img   = Image.open(io.BytesIO(png_bytes)).convert("RGBA")
    bg_img   = Image.new("RGBA", (out_size, out_size), bg_color + (255,))
    bg_img.paste(av_img, (0, 0), av_img)

    buf = io.BytesIO()
    bg_img.convert("RGB").save(buf, format="PNG")
    return buf.getvalue()


# ---------------------------------------------------------------------------
# App + request schemas
# ---------------------------------------------------------------------------

app = FastAPI(title="AvaGen Inference Sidecar")


class GenerateRequest(BaseModel):
    prompt: str = ""
    negative_prompt: str = ""
    width: int = 512
    height: int = 512
    num_inference_steps: int = 4
    guidance_scale: float = 7.5
    seed: int = 0
    traits: Optional[Dict[str, Any]] = None


class VideoGenerateRequest(BaseModel):
    image_b64: Optional[str] = None
    image_url: Optional[str] = None
    motion_bucket_id: int = 127
    noise_aug_strength: float = 0.02
    fps_id: int = 6
    seed: int = 0


# ---------------------------------------------------------------------------
# Endpoints
# ---------------------------------------------------------------------------

@app.get("/health")
def health():
    return {
        "status":      "ok",
        "mock":        MOCK,
        "flux_loaded": True,   # no model load needed — always ready
        "svd_loaded":  False,
    }


@app.post("/generate")
def generate(req: GenerateRequest) -> Response:
    traits = req.traits or {}

    # MOCK or no traits → plain colour stub
    if MOCK or not traits:
        img = Image.new("RGB", (req.width, req.height), color=(40, 40, 50))
        buf = io.BytesIO()
        img.save(buf, format="PNG")
        log.info(f"[stub] /generate {req.width}×{req.height}")
        return Response(content=buf.getvalue(), media_type="image/png")

    try:
        out_size = max(req.width, req.height)
        result   = _build_avatar_png(traits, out_size)
        log.info(
            f"Avatar rendered: {traits.get('sex','?')} "
            f"{traits.get('ethnicity','?')} {out_size}px"
        )
        return Response(content=result, media_type="image/png")
    except Exception:
        import traceback
        log.error(f"Avatar render error:\n{traceback.format_exc()}")
        raise HTTPException(status_code=500, detail="Avatar render failed")


@app.post("/video/generate")
def generate_video(_req: VideoGenerateRequest) -> Response:
    raise HTTPException(status_code=503, detail="Video generation is not available")


if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=8001, log_level="info")
