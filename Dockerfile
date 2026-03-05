FROM rust:1.93-slim AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# Build release binary
RUN cargo build --release --bin poly-book

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/poly-book /usr/local/bin/poly-book
COPY config/default.toml /etc/poly-book/default.toml

ENV PB__STORAGE__PARQUET_BASE_PATH=/data
ENV PB__METRICS__LISTEN_ADDR=0.0.0.0:9090

EXPOSE 9090

VOLUME ["/data"]

ENTRYPOINT ["poly-book"]
CMD ["--config", "/etc/poly-book/default.toml", "--help"]
