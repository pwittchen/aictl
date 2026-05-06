# syntax=docker/dockerfile:1.7
#
# aictl-server image — multi-stage Rust build → Debian slim runtime.
#
# Build from the repo root so the build context contains the workspace:
#   docker build -f docker/server.Dockerfile -t aictl-server .
#
# Run with a persistent volume for ~/.aictl (config + provider keys +
# auto-generated AICTL_SERVER_MASTER_KEY + audit log + server log) and
# expose 7878:
#   docker run --rm -d \
#     --name aictl-server \
#     -p 7878:7878 \
#     -v aictl-data:/home/aictl/.aictl \
#     aictl-server
#
# On first launch the server generates a master key and prints it to
# stderr — capture it from `docker logs aictl-server`. Subsequent
# launches reuse the persisted key from the mounted volume.
#
# The bind defaults to 0.0.0.0:7878 inside the container so the
# published port is reachable; the security model (master key, body
# cap, concurrency cap, optional rate limit) still applies.
#
# Optional cargo features (gguf / mlx / redaction-ner) follow the same
# rules as the CLI image — off by default; pass --build-arg
# FEATURES="redaction-ner" to opt in.

ARG RUST_VERSION=1.85
ARG DEBIAN_VERSION=bookworm

# ---------- builder ----------
FROM rust:${RUST_VERSION}-slim-${DEBIAN_VERSION} AS builder

ARG FEATURES=""

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config \
      libssl-dev \
      libdbus-1-dev \
      ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    if [ -n "${FEATURES}" ]; then \
      cargo build --release --bin aictl-server --features "${FEATURES}"; \
    else \
      cargo build --release --bin aictl-server; \
    fi \
 && cp target/release/aictl-server /usr/local/bin/aictl-server

# ---------- runtime ----------
FROM debian:${DEBIAN_VERSION}-slim AS runtime

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      libssl3 \
      libdbus-1-3 \
      tini \
      curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --uid 1000 --shell /bin/bash aictl

COPY --from=builder /usr/local/bin/aictl-server /usr/local/bin/aictl-server

# Bind to all interfaces inside the container so a published port is
# reachable from the host. Override with -e AICTL_SERVER_BIND=… or
# the --bind flag.
ENV AICTL_SERVER_BIND=0.0.0.0:7878

USER aictl
WORKDIR /home/aictl

VOLUME ["/home/aictl/.aictl"]

EXPOSE 7878

# /healthz is unauthenticated and always on — perfect for liveness.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:7878/healthz || exit 1

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/aictl-server"]
