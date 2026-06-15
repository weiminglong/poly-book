# --- Stage 1: Build the web SPA ---
FROM node:22-slim AS web-builder

WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --ignore-scripts
COPY web/ ./
RUN npx vite build

# --- Stage 2: Build the Rust binary ---
# Must be >= the workspace MSRV (rust-version = "1.94"); rust:1.93 fails to build.
FROM rust:1.94-slim AS builder

# pb-grpc's build script needs protoc; native-tls (WS feed) needs OpenSSL dev
# headers + pkg-config to compile.
RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
# `tests/integration` is a workspace member, so its manifest must be present for
# the workspace to resolve even when building only the binary.
COPY tests/ tests/

RUN cargo build --release --bin poly-book

# --- Stage 3: Final runtime image ---
FROM debian:bookworm-slim

# libssl3 is required at runtime because the binary links OpenSSL via native-tls.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/poly poly

COPY --from=builder /app/target/release/poly-book /usr/local/bin/poly-book
COPY --from=web-builder /web/dist /var/lib/poly-book/web
COPY config/default.toml /etc/poly-book/default.toml

# Durable data volume, owned by the non-root runtime user.
RUN mkdir -p /data && chown -R poly:poly /data /var/lib/poly-book

ENV PB__STORAGE__PARQUET_BASE_PATH=/data
# Bind to all interfaces inside the container; the trust boundary is enforced at
# the orchestrator / security-group / reverse-proxy level, not in-process.
ENV PB__METRICS__LISTEN_ADDR=0.0.0.0:9090
ENV PB__API__LISTEN_ADDR=0.0.0.0:3000

EXPOSE 3000 9090

VOLUME ["/data"]

# Run as the non-root user (audit finding A.158).
USER poly

ENTRYPOINT ["poly-book"]
CMD ["--config", "/etc/poly-book/default.toml", "--help"]
