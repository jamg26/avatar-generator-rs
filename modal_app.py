"""
Modal deployment for AvaGen — AI Avatar Generation API.

Deploy:
    modal deploy modal_app.py

Serve (ephemeral / test):
    modal serve modal_app.py

The Rust/Axum binary is baked into the image at deploy time.
All secrets are loaded from .env via Modal's secret mechanism.
"""

import modal

app = modal.App("avagen")

# ---------------------------------------------------------------------------
# Image: Debian slim + OpenSSL runtime (binary links against libssl3) + binary
# ---------------------------------------------------------------------------
image = (
    modal.Image.debian_slim()
    .apt_install("libssl3", "ca-certificates")
    # Bake the pre-built release binary into the image layer
    .add_local_file(
        local_path="target/release/avagen",
        remote_path="/app/avagen",
        copy=True,
    )
    .run_commands("chmod +x /app/avagen")
    .workdir("/app")
)


# ---------------------------------------------------------------------------
# Web server function — routes all HTTP traffic to port 8080 on the container
# ---------------------------------------------------------------------------
@app.function(
    image=image,
    # Load all env vars from .env file (DATABASE_URL, HF_TOKEN, ADMIN_SECRET, …)
    secrets=[modal.Secret.from_dotenv()],
    # Keep 1 container always warm to avoid cold-start latency
    min_containers=1,
    # Image generation can take up to ~60s; give plenty of room
    timeout=600,
)
@modal.web_server(8080, startup_timeout=30.0)
def serve():
    """Start the Axum server. Modal keeps the container alive and routes traffic to port 8080."""
    import subprocess
    subprocess.Popen(["/app/avagen"])
