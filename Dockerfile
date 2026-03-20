# ── Stage 1: Build the Rust binary ─────────────────────────────────────────
# Pinned to bookworm so the binary links against glibc 2.36, matching the
# runtime image. rust:1-slim recently moved to Ubuntu 24.04 (glibc 2.39).
FROM rust:1-slim-bookworm AS builder
WORKDIR /build

RUN apt-get update && apt-get install -y pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Cache dependency compilation separately from source changes.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs && \
    cargo build --release 2>/dev/null; \
    rm -f target/release/deps/avagen*

# Compile with the real source
COPY src ./src
RUN cargo build --release

# ── Stage 2: Minimal runtime image ──────────────────────────────────────────
FROM debian:bookworm-slim
WORKDIR /app

# ca-certificates — needed for TLS certificate verification (DiceBear API, NeonDB)
# libssl3         — runtime OpenSSL libs (linked by sqlx / reqwest)
# python3 + venv  — needed by huggingface_hub for HF bucket uploads
RUN apt-get update && apt-get install -y \
    ca-certificates libssl3 python3 python3-venv && \
    python3 -m venv /opt/hfvenv && \
    /opt/hfvenv/bin/pip install --quiet "huggingface_hub>=1.5.0" && \
    rm -rf /var/lib/apt/lists/* /root/.cache/pip

COPY --from=builder /build/target/release/avagen ./avagen
RUN chmod +x ./avagen

COPY start.sh ./start.sh
RUN chmod +x ./start.sh

# ── Runtime environment ──────────────────────────────────────────────────────
ENV PORT=7860 \
    RUST_LOG=avagen=info,tower_http=info \
    SKIP_VIDEO_PIPELINE=1

EXPOSE 7860

CMD ["./start.sh"]
