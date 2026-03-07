# --- Stage 1: Build the web SPA ---
FROM node:22-slim AS web-builder

WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --ignore-scripts
COPY web/ ./
RUN npx vite build

# --- Stage 2: Build the Rust binary ---
FROM rust:1.93-slim AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release --bin poly-book

# --- Stage 3: Final runtime image ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/poly-book /usr/local/bin/poly-book
COPY --from=web-builder /web/dist /var/lib/poly-book/web
COPY config/default.toml /etc/poly-book/default.toml

ENV PB__STORAGE__PARQUET_BASE_PATH=/data
ENV PB__METRICS__LISTEN_ADDR=0.0.0.0:9090
ENV PB__API__LISTEN_ADDR=0.0.0.0:3000

EXPOSE 3000 9090

VOLUME ["/data"]

ENTRYPOINT ["poly-book"]
CMD ["--config", "/etc/poly-book/default.toml", "--help"]
