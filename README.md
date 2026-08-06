# NovaDB v0.1

**A lightweight embedded vector database built from scratch in Rust.**

NovaDB is an educational, open-source systems project. It implements its own
storage engine, write-ahead log with crash recovery, exact flat vector search,
metadata filtering, and hybrid retrieval — no external database underneath.

> Built to demonstrate systems engineering fundamentals: storage engine
> design, binary file formats, write-ahead logging, crash recovery, and API
> design — not to compete with production databases.

## Features

| Area | What's included |
| --- | --- |
| Storage | Custom binary snapshot format (`*.nova`), collections, full CRUD |
| Durability | Write-ahead log (`wal/*.log`), crash recovery, torn-tail handling, atomic periodic snapshots, 3 sync modes |
| Retrieval | Exact flat top-K search, cosine similarity, precomputed norms |
| Filtering | Equality, numeric comparisons, boolean AND over JSON metadata |
| Hybrid | Vector similarity + metadata filter in one query (filter-then-score) |
| DX | JSON CLI, fluent Rust query API, Criterion benchmarks |

**Intentionally out of scope (v0.1):** SQL, HNSW/IVF/PQ, replication, Raft,
full-text search, distributed execution.

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
          (CLI / Rust API)
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

- [`docs/format.md`](docs/format.md) — the `*.nova` snapshot format
- [`docs/wal.md`](docs/wal.md) — the write-ahead log protocol and recovery
- [`docs/query.md`](docs/query.md) — the metadata filter DSL

## Roadmap (not in v0.1)

Python SDK (PyO3), HNSW/IVF approximate indexes, SQL-ish interface, columnar
storage, REST server. See `task.md` for the full project brief.

## License

MIT
