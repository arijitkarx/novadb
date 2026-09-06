# NovaDB BYOS v0.1 contract

The first deployment milestone runs one NovaDB server process with one writer
and one configured database (`default` unless `NOVADB_DATABASE` is set). The
embedded Rust API remains supported. This release is not distributed, does not
support replication, and must not have two processes writing the same data
directory or object prefix.

## Data and API limits

- Collection names match `[A-Za-z0-9_-]{1,64}` and cannot be `.` or `..`.
- A collection has a fixed, non-zero dimension. The practical dimension limit
  is governed by the 4 MiB request limit; clients should use 1–4096 dimensions.
- Cosine similarity is the only distance metric. Search remains exact.
- Metadata is any JSON value. Filters support equality, numeric comparisons,
  and boolean `AND` as documented in [query.md](query.md).
- Request bodies default to 4 MiB and batches to 1,000 vectors. Configure these
  with `NOVADB_MAX_BODY_BYTES` and `NOVADB_MAX_BATCH_SIZE`.

## Local quick start

```console
export NOVADB_API_KEY='replace-with-a-long-random-secret'
docker compose up --build
curl http://localhost:8080/health
curl -H "Authorization: Bearer $NOVADB_API_KEY" \
  -H 'content-type: application/json' \
  -d '{"name":"products","dimension":3}' \
  http://localhost:8080/v1/databases/default/collections
curl -H "Authorization: Bearer $NOVADB_API_KEY" \
  -H 'content-type: application/json' \
  -d '{"vectors":[{"vector":[0.1,0.2,0.3],"metadata":{"kind":"demo"}}]}' \
  http://localhost:8080/v1/databases/default/collections/products/vectors
```

Only `/health` is unauthenticated. All other endpoints require
`Authorization: Bearer <NOVADB_API_KEY>`. Secrets are accepted only through the
environment and are never included in logs. Put TLS and rate limiting at a
reverse proxy until native support lands.

The server currently persists to a mounted local filesystem. The new
`StorageBackend` contract and its local/in-memory implementations establish
conditional manifest commits, but S3 and GCS synchronization are not yet wired
to the database lifecycle. Do not treat the local milestone as remote BYOS.

On SIGINT the server stops accepting requests and drops the database, which
performs a best-effort snapshot. On startup, readiness is exposed only after
snapshot loading and WAL recovery complete.
