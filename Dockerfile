# Build stage
FROM rust:1.97-slim AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p novadb-cli --bin novadb

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /data
COPY --from=builder /build/target/release/novadb /usr/local/bin/novadb
ENTRYPOINT ["novadb"]
CMD ["--db", "/data"]
