# NovaDB BYOS v0.1 Alpha

**A self-hostable bring-your-own-storage vector database with an embedded Rust library.**

NovaDB is an open-source systems project. It implements its own
storage engine, write-ahead log with crash recovery, exact flat vector search,
metadata filtering, and hybrid retrieval — no external database underneath.

The current alpha includes an authenticated HTTP server and a provider-neutral
storage contract. Local persistent volumes are usable today. Native S3 and GCS
synchronization is planned but **not implemented yet**.

> **Project status:** NovaDB is open-source alpha software. It is suitable for
> evaluation, internal tools, prototypes, and small single-node workloads where
> downtime is acceptable. Do not present the current release as a distributed,
> highly available, or remote-object-storage-backed database.

## Features

| Area | What's included |
| --- | --- |
| Storage | Custom binary snapshot format (`*.nova`), collections, full CRUD |
| Durability | Write-ahead log (`wal/*.log`), crash recovery, torn-tail handling, atomic periodic snapshots, 3 sync modes |
| Retrieval | Exact flat top-K search, cosine similarity, precomputed norms |
| Filtering | Equality, numeric comparisons, boolean AND over JSON metadata |
| Hybrid | Vector similarity + metadata filter in one query (filter-then-score) |
| DX | HTTP/JSON server, JSON CLI, fluent Rust API, Criterion benchmarks |

**Intentionally out of scope (v0.1):** multi-writer operation, SQL,
HNSW/IVF/PQ, replication, Raft, full-text search, distributed execution.

## Self-hosted quick start

```console
export NOVADB_API_KEY='replace-with-a-long-random-secret'
docker compose up --build
curl http://localhost:8080/health
curl -H "Authorization: Bearer $NOVADB_API_KEY" http://localhost:8080/v1/info
```

See [`docs/byos.md`](docs/byos.md) for the deployment contract, limits, and API
examples. One process owns one database and data directory; v0.1 is
single-writer and non-distributed.

## Basic production deployment

The current release can be deployed as a single-node service backed by a
durable local or cloud block-storage volume:

1. Clone the repository and generate a strong API key.
2. Mount a durable SSD-backed host path or block volume at `/data`.
3. Start one NovaDB container for that data directory.
4. Keep port `8080` private and place Caddy, Nginx, Traefik, or a cloud load
   balancer in front of it for TLS, rate limiting, and request timeouts.
5. Back up the complete data directory and regularly test restoration.
6. Monitor `/health`, `/ready`, container restarts, disk usage, and logs.

For example, change the service volume in `compose.yml` to a host path:

```yaml
services:
  novadb:
    volumes:
      - /srv/novadb/data:/data
```

Store `NOVADB_API_KEY` in a deployment secret or protected environment file,
not in source control. Stop the container gracefully so NovaDB can checkpoint
its state. Never run two NovaDB processes against the same data directory.

### S3 and S3-compatible storage

NovaDB cannot currently use S3 as its live durable backend. The
`StorageBackend` trait is the integration foundation, but the database lifecycle
is still connected to the local filesystem. Mounting a bucket with `s3fs` or a
similar FUSE adapter is not recommended because object stores do not provide the
filesystem durability and atomicity assumptions used by the local engine.

Today, use a persistent block volume for live data and copy verified backups to
S3. This makes S3 a backup destination, not the database's source of truth.

The planned native S3 implementation will:

- keep local disk as a rebuildable cache and WAL working area;
- upload completed WAL segments and snapshots as immutable objects;
- commit generations through a versioned `manifest.json` using conditional
  writes/ETags;
- recover from the latest snapshot plus subsequent WAL segments at startup; and
- require one writer for each database prefix.

The intended configuration shape is:

```env
NOVADB_STORAGE_PROVIDER=s3
NOVADB_S3_BUCKET=my-private-bucket
NOVADB_S3_PREFIX=production/novadb
NOVADB_S3_REGION=us-east-1
NOVADB_DATA_DIR=/var/cache/novadb
NOVADB_API_KEY=replace-with-a-secret
```

These S3 variables are documentation of the planned interface and are **not
accepted by the current server**. IAM roles will be preferred over static access
keys when the backend is implemented.

## Quick start

```console
$ cargo build --release

$ ./target/release/novadb --db ./demo create products --dim 3
$ ./target/release/novadb --db ./demo insert products \
    --vector "[0.1, 0.2, 0.3]" --metadata '{"category": "electronics", "price": 999}'
$ ./target/release/novadb --db ./demo insert products \
    --vector "[0.9, 0.8, 0.7]" --metadata '{"category": "books", "price": 25}'
$ ./target/release/novadb --db ./demo query products \
    --vector "[0.1, 0.2, 0.3]" --top-k 5 \
    --filter 'category = "electronics" AND price < 2000'
```

Results are JSON, so the CLI composes with `jq`.

## Using the library

```rust
use novadb::engine::query::Query;
use novadb::metadata::Filter;
use novadb::{Database, DbConfig};

fn main() -> novadb::Result<()> {
    let mut db = Database::open(DbConfig::new("/tmp/novadb"))?;

    db.create_collection("products", 3)?;
    db.add("products", &[0.1, 0.2, 0.3],
           serde_json::json!({"category": "electronics", "price": 99}))?;

    let hits = Query::new("products", &[0.1, 0.2, 0.3])
        .top_k(5)
        .filter(Filter::parse("price < 1000")?)
        .run(&db)?;
    db.close()?;
    Ok(())
}
```

See `examples/basic.rs` for a runnable version.

## Architecture

```text
                Client
       (HTTP / CLI / Rust API)
                  │
            Query Engine
          ┌───────┴────────┐
          │                │
   Vector Search     Metadata Filter
          │                │
          └───────┬────────┘
                  │
          Storage Engine
        (Files + WAL + Snapshot)
                  │
             Local Disk
```

All writes are **WAL-first**: an operation is appended to the collection's
log (and optionally fsynced) *before* it is applied to memory. Reopening a
database rebuilds state as `snapshot + WAL replay`. Snapshots are written
atomically (temp file + rename) and the WAL is truncated after — a crash
between the two steps is safe because replay is idempotent.

### On-disk layout

```text
demo/
├── products.nova        # snapshot: magic header + checksummed records
└── wal/
    └── products.log     # WAL: framed, checksummed operations
```

Formats are documented in `docs/format.md` and `docs/wal.md`.

## Repository layout

```text
src/
├── storage/    binary format, collections, CRUD
├── wal/        frame encoding, durable log, recovery
├── vector/     cosine similarity, flat top-K search
├── metadata/   filter AST + query DSL parser
└── engine/     Database lifecycle, hybrid queries
server/         authenticated HTTP/JSON server
cli/            JSON command-line interface
benchmarks/     Criterion: search latency & insert throughput
tests/          integration tests incl. crash recovery
docs/           format, WAL protocol, query DSL
examples/       runnable library examples
sdk/            Python SDK (planned, see README)
```

## Configuration

`DbConfig` supports three WAL sync policies:

| Mode | Durability | Cost |
| --- | --- | --- |
| `SyncMode::Every` (default) | fsync after every write; crash loses nothing acknowledged | ~11 ms/insert |
| `SyncMode::Batch(n)` | fsync every *n* writes; crash may lose the last *n* | ~29 µs/insert |
| `SyncMode::Never` | no fsync; OS flush decides | ~14 µs/insert |

Set `auto_snapshot_after` to compact the WAL into a snapshot during writes
(default 10,000 ops).

## Benchmarks (summary)

Hardware-dependent; see `benchmarks/` for the full harness:

```text
search_top_k/unfiltered/100000  ~25 ms   (dim 128, flat scan)
search_top_k/filtered/100000    ~15 ms   (early filter rejection)
insert/sync=Never               ~14 µs
insert/sync=Batch(1024)         ~29 µs
insert/sync=Every               ~11 ms   (dominated by fsync)
```

## Documentation

- [`docs/overview.md`](docs/overview.md) — the codebase map: entry points, what the WAL is, storage, lifecycle
- [`docs/format.md`](docs/format.md) — the `*.nova` snapshot format
- [`docs/wal.md`](docs/wal.md) — the write-ahead log protocol and recovery
- [`docs/query.md`](docs/query.md) — the metadata filter DSL
- [`docs/byos.md`](docs/byos.md) — server configuration, deployment contract, limits, and API examples

## Roadmap (not in v0.1)

- Native S3-compatible and Google Cloud Storage durability and recovery
- Backup, restore, verify, and index-rebuild commands
- Python SDK and OpenAPI documentation
- Metrics, operational alerts, and process-level single-writer locking
- HNSW approximate indexing after remote durability is stable
- SQL-like and full-text interfaces as later work

See [`novadb_byos_plan.md`](novadb_byos_plan.md) for the complete plan. Issues
and focused pull requests are welcome, particularly around object-storage
contracts, failure testing, recovery, API compatibility, and documentation.

## License

MIT
