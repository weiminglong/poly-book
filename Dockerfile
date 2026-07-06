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

# pb-grpc's build script needs protoc. TLS is rustls (no OpenSSL), so no
# libssl-dev/pkg-config are needed to compile.
RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler \
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

# No libssl3 at runtime: TLS is rustls with bundled webpki (Mozilla) roots, so
# the binary neither links OpenSSL nor needs the system CA store.
# ca-certificates is kept as harmless belt-and-suspenders for any tooling that
# consults the system trust store.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/poly poly

COPY --from=builder /app/target/release/poly-book /usr/local/bin/poly-book
COPY --from=web-builder /web/dist /var/lib/poly-book/web
COPY config/default.toml /etc/poly-book/default.toml

# Durable data volume, owned by the non-root runtime user.
RUN mkdir -p /data && chown -R poly:poly /data /var/lib/poly-book

ENV PB__STORAGE__PARQUET_BASE_PATH=/data
# Loopback defaults avoid accidental unauthenticated exposure. Orchestrators that
# need an external bind must set PB__API__AUTH_TOKEN as well.
ENV PB__METRICS__LISTEN_ADDR=127.0.0.1:9090
ENV PB__API__LISTEN_ADDR=127.0.0.1:3000
# Serve the bundled workstation UI from the same process: any non-API route
# falls back to the SPA, so one container is API + UI.
ENV PB__API__STATIC_ASSETS_DIR=/var/lib/poly-book/web

EXPOSE 3000 9090

VOLUME ["/data"]

# Run as the non-root user.
USER poly

ENTRYPOINT ["poly-book"]
# Default to the combined live workstation (feed + API + UI in one process)
# so `docker run -p 3000:3000 -e PB__API__LISTEN_ADDR=0.0.0.0:3000 <image>`
# is a working system, not a help screen. Other subcommands (ingest, serve,
# reconcile, ...) override this CMD; docker-compose does so explicitly.
CMD ["--config", "/etc/poly-book/default.toml", "serve-api", "--auto-rotate"]
