#!/usr/bin/env bash
# Startup script for HuggingFace Spaces (and any Docker environment).
# Launches the Python inference sidecar, then the Rust Axum server.
set -e

echo "[start] Starting AvaGen inference sidecar (port 8001)..."
python3 /app/infer.py &
SIDECAR_PID=$!

echo "[start] Starting AvaGen Rust server (port ${PORT:-7860})..."
exec /app/avagen
