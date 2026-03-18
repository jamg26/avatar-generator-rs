#!/usr/bin/env python3
"""Deploy AvaGen to HuggingFace Spaces.

Usage:
    python deploy_spaces.py

Reads DATABASE_URL, ADMIN_SECRET, and HF_TOKEN from the local .env file (or
environment variables) and pushes everything to the HF Space.
"""
from __future__ import annotations

import os
import sys
import shutil
import tempfile
from pathlib import Path

SPACE_ID = "jamg/avagen"
REPO_ROOT = Path(__file__).parent


def _load_dotenv(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    try:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    k, _, v = line.partition("=")
                    env[k.strip()] = v.strip()
    except FileNotFoundError:
        pass
    return env


def main() -> None:
    local_env = _load_dotenv(REPO_ROOT / ".env")

    hf_token = os.environ.get("HF_TOKEN") or local_env.get("HF_TOKEN")
    if not hf_token:
        print("ERROR: HF_TOKEN not found in environment or .env file", file=sys.stderr)
        sys.exit(1)

    try:
        from huggingface_hub import HfApi
    except ImportError:
        print("Installing huggingface_hub ...")
        os.system(f"{sys.executable} -m pip install -q huggingface_hub")
        from huggingface_hub import HfApi

    api = HfApi(token=hf_token)

    # ── 1. Create / verify Space ─────────────────────────────────────────────
    print(f"→ Creating Space: {SPACE_ID}")
    api.create_repo(
        repo_id=SPACE_ID,
        repo_type="space",
        space_sdk="docker",
        private=False,
        exist_ok=True,
    )
    print("  Space ready")

    # ── 2. Push secrets into the Space ───────────────────────────────────────
    secrets = {
        "DATABASE_URL": os.environ.get("DATABASE_URL") or local_env.get("DATABASE_URL"),
        "ADMIN_SECRET": os.environ.get("ADMIN_SECRET") or local_env.get("ADMIN_SECRET"),
        "HF_TOKEN":     hf_token,
    }
    for key, value in secrets.items():
        if value:
            try:
                api.add_space_secret(repo_id=SPACE_ID, key=key, value=value)
                print(f"  Secret set: {key}")
            except Exception as exc:
                print(f"  Warning: could not set secret {key}: {exc}")

    # ── 3. Upload Space files ────────────────────────────────────────────────
    print(f"→ Uploading files to {SPACE_ID} ...")
    with tempfile.TemporaryDirectory() as _tmp:
        tmp = Path(_tmp)

        # Space metadata README (YAML frontmatter required by HF Spaces)
        shutil.copy(REPO_ROOT / "space_readme.md", tmp / "README.md")

        # Docker & startup
        shutil.copy(REPO_ROOT / "Dockerfile", tmp / "Dockerfile")
        shutil.copy(REPO_ROOT / "start.sh",   tmp / "start.sh")

        # Python inference sidecar
        shutil.copy(REPO_ROOT / "infer.py", tmp / "infer.py")

        # Rust build files
        shutil.copy(REPO_ROOT / "Cargo.toml", tmp / "Cargo.toml")
        shutil.copy(REPO_ROOT / "Cargo.lock", tmp / "Cargo.lock")

        # Rust source tree
        shutil.copytree(REPO_ROOT / "src", tmp / "src")

        api.upload_folder(
            folder_path=str(tmp),
            repo_id=SPACE_ID,
            repo_type="space",
            commit_message="Deploy AvaGen",
        )

    space_url = f"https://huggingface.co/spaces/{SPACE_ID}"
    api_url   = "https://jamg-avagen.hf.space"
    print(f"\n✓ Deployed!")
    print(f"  Space:  {space_url}")
    print(f"  API:    {api_url}")
    print(f"\nThe Space is now building (Rust compile + ~5 min).")
    print(f"Monitor: {space_url}\n")
    print("When the build finishes, run the integration tests:")
    print(f"  BASE={api_url} ADMIN_SECRET=<your-secret> ./test.sh")


if __name__ == "__main__":
    main()
