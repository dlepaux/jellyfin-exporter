# Stage 1: Build
FROM rust:1.85-bookworm AS builder

# Build-time metadata. Forwarded to build.rs via env vars below so the
# `jellyfin_exporter_build_info` metric reports accurate values for the
# image being built. CI passes real values; local `docker build` defaults
# to placeholders.
ARG VERSION=dev
ARG GIT_SHA=unknown
ARG BUILD_DATE=unknown

ENV BUILD_GIT_SHA=$GIT_SHA \
    BUILD_DATE=$BUILD_DATE

WORKDIR /build

# Copy manifests for dependency caching
COPY Cargo.toml Cargo.lock build.rs ./

# Create dummy source to cache dependency compilation
RUN mkdir -p src && echo "fn main(){}" > src/main.rs && echo "" > src/lib.rs
RUN cargo build --release 2>/dev/null || true

# Copy real source code
COPY src/ src/

# Invalidate cached builds for real source
RUN touch src/main.rs src/lib.rs
RUN cargo build --release --bin jellyfin-exporter

# Stage 2: Runtime
FROM debian:bookworm-slim

# Build metadata, baked into OCI labels by CI (defaults are placeholders for
# local builds where these args are not passed).
ARG VERSION=dev
ARG GIT_SHA=unknown
ARG BUILD_DATE=unknown

LABEL org.opencontainers.image.source=https://github.com/dlepaux/jellyfin-exporter
LABEL org.opencontainers.image.licenses=MIT
LABEL org.opencontainers.image.title="jellyfin-exporter"
LABEL org.opencontainers.image.description="Prometheus exporter for Jellyfin — sessions, transcoding, and library metrics"
LABEL org.opencontainers.image.version=$VERSION
LABEL org.opencontainers.image.revision=$GIT_SHA
LABEL org.opencontainers.image.created=$BUILD_DATE

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --system --gid 1001 exporter \
    && useradd --system --uid 1001 --gid exporter exporter

COPY --from=builder /build/target/release/jellyfin-exporter /usr/local/bin/jellyfin-exporter

USER exporter

EXPOSE 9711

# No HEALTHCHECK directive: the orchestrator (Docker Compose / k8s) probes
# /health and /ready directly. Avoiding wget/curl in the runtime image keeps
# the attack surface minimal — the only executable is the exporter binary.

ENTRYPOINT ["jellyfin-exporter"]
