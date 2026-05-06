# syntax=docker/dockerfile:1.7
#
# aictl CLI image — multi-stage Rust build → Debian slim runtime.
#
# Build from the repo root so the build context contains the workspace:
#   docker build -f docker/cli.Dockerfile -t aictl .
#
# Run interactively (REPL needs a TTY); mount ~/.aictl for persistent
# config, keys, sessions, and audit:
#   docker run --rm -it \
#     -v "$HOME/.aictl:/home/aictl/.aictl" \
#     -v "$PWD:/workspace" \
#     aictl
#
# One-shot non-interactive use:
#   docker run --rm \
#     -v "$HOME/.aictl:/home/aictl/.aictl" \
#     aictl --message "hello"
#
# Optional cargo features (gguf / mlx / redaction-ner) are off by default.
# MLX is Apple-Silicon-only and never built in this image. GGUF and the
# NER redactor pull large native deps (llama.cpp, ONNX runtime); add them
# with --build-arg FEATURES="gguf" or FEATURES="redaction-ner" only when
# you need them, and expect a longer build.

ARG RUST_VERSION=1.85
ARG DEBIAN_VERSION=bookworm

# ---------- builder ----------
FROM rust:${RUST_VERSION}-slim-${DEBIAN_VERSION} AS builder

ARG FEATURES=""

# reqwest pulls native-tls by default → needs OpenSSL headers.
# keyring's sync-secret-service backend links libdbus-1.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config \
      libssl-dev \
      libdbus-1-dev \
      ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /src

# Copy the whole workspace. The .dockerignore at the repo root keeps
# target/, .git/, and the website out of the build context so this stays
# fast even though we're not doing a recipe-cached build.
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    if [ -n "${FEATURES}" ]; then \
      cargo build --release --bin aictl --features "${FEATURES}"; \
    else \
      cargo build --release --bin aictl; \
    fi \
 && cp target/release/aictl /usr/local/bin/aictl

# ---------- runtime ----------
FROM debian:${DEBIAN_VERSION}-slim AS runtime

# libssl3 + ca-certificates for HTTPS to LLM providers.
# libdbus-1-3 lets the keyring backend probe the Secret Service when
# one is present; without dbus running, keys::get_secret silently falls
# back to the plain ~/.aictl/config entry, which is the expected
# container behavior.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      libssl3 \
      libdbus-1-3 \
      tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --uid 1000 --shell /bin/bash aictl

COPY --from=builder /usr/local/bin/aictl /usr/local/bin/aictl

USER aictl
WORKDIR /workspace

# Persist config, keys, sessions, audit, agents, skills, plugins, hooks.
VOLUME ["/home/aictl/.aictl"]

# tini reaps zombies and forwards signals — important because the REPL
# spawns child processes for the shell tool.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/aictl"]
