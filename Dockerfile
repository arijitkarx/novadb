# Build stage
FROM rust:1.97-slim AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p novadb-server --bin novadb-server \
    && cargo build --release -p novadb-cli --bin novadb

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /data
COPY --from=builder /build/target/release/novadb /usr/local/bin/novadb
COPY --from=builder /build/target/release/novadb-server /usr/local/bin/novadb-server
ENV PORT=8080 NOVADB_DATA_DIR=/data
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=2s --retries=6 CMD curl --fail --silent http://localhost:8080/health || exit 1
ENTRYPOINT ["novadb-server"]
