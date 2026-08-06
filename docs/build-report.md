# NovaDB v0.1 — Build Session Report

Date: 2026-08-06

Everything below was implemented, compiled, and verified in this session.
The full v0.1 scope from `task.md` was delivered: storage engine, WAL +
crash recovery + snapshots, hybrid retrieval, CLI, tests, benchmarks,
documentation, Docker, and CI.

---

## 1. What was built

### Milestone 1 — Storage engine
- **Custom binary snapshot format** (`data/{name}.nova`): 32-byte header
  (magic `NOVA1\x00\x00\x00`, version, dim, next-id, record count) followed
  by length-prefixed, CRC-32-checksummed record blocks.
- **CRC-32 (IEEE)** implemented from scratch (table-free bitwise, verified
  against the canonical `CRC32("123456789") == 0xCBF43926`).
- **Collections**: in-memory append-ordered record list + id→index hashmap
  (swap-remove deletes keep O(1) point lookups), precomputed L2 norms,
  auto-assigned ids, dim validation.
- **CRUD**: insert (upsert with explicit id), add (auto id), get, delete,
  list, records, drop collection.
- **Metadata storage**: bincode cannot serialize `serde_json::Value`
  (no `deserialize_any`), so records persist as `PersistedRecord`
  `{id, vector, metadata: String}` with metadata as a JSON document —
  the same interchange format used at every API boundary.

### Milestone 2 — Durability
- **WAL** (`wal/{name}.log`): frames of
  `magic 0xAD | version | seq u64 | len u32 | crc32 | bincode(WalOp)`.
- **Ops**: `Create(dim)` (every log begins with one, so WAL-only collections
  survive crashes), `Put(record)`, `Delete(id)`.
- **WAL-first write ordering**: append (+ fsync) *then* apply to memory.
  A failed append errors without touching memory.
- **Three sync modes**: `Every` (fsync per write), `Batch(n)` (fsync every n),
  `Never` — exposed via `DbConfig` and the CLI (`--no-sync`, `--batch`).
- **Crash recovery**: state = snapshot + idempotent WAL replay.
  - Torn tail (partial frame) → truncated away, durable prefix kept.
  - Torn first frame (magic present, nothing parsed) → log reset to empty
    (nothing was ever durable; the collection simply doesn't exist yet).
  - Unrecognizable bytes at log start → open fails loudly with `Corrupt`,
    never silently discards data.
  - Bad checksum in a full frame → `Corrupt` error.
- **Snapshots**: atomic write (tmp + fsync + rename); then WAL truncated.
  Crash between the two steps is safe because replay is idempotent.
- **Checkpoint triggers**: explicit `snapshot()`/`close()`, and automatic
  compaction when a collection's op count crosses `auto_snapshot_after`
  (default 10,000).

### Milestone 3 — Retrieval
- **Cosine similarity**: dot / (|a||b|), zero vectors score 0.0, computed
  with precomputed record norms (one dot product per candidate).
- **Flat top-K search**: bounded binary heap with NaN-safe `total_cmp`
  ordering and id tie-breaks; results sorted by score desc, id asc.
- **Metadata filters**: AST with `=`, `!=`, `<`, `>`, `<=`, `>=` and boolean
  `AND`; numeric comparison unifies int/float (`1 == 1.0`); missing fields
  never match.
- **Query DSL**: hand-written recursive-descent parser (lexer + parser,
  string escapes, integer/float literals) supporting exactly the task.md
  syntax, e.g. `category = "electronics" AND price < 1000 AND rating >= 4`.
- **Hybrid queries**: fluent `Query` builder (`Query::new(c, v).top_k(k)
  .filter(f).run(&db)`), executing filter-then-score — selective filters
  make hybrid search *faster*, not slower.

### Milestone 4 — Developer experience
- **CLI** (`novadb`, JSON output, jq-friendly): `create`, `drop`, `list`,
  `info`, `insert` (`--id` for upsert), `get`, `delete`, `records`, `query`
  (`--vector`, `--top-k`, `--filter`), `checkpoint`, plus `--db`,
  `--no-sync`, `--batch`.
- **Tests**: 46 unit tests + 13 integration tests + 2 doctests = **61**.
  Integration suite covers: reopen persistence, ungraceful shutdown
  recovery, torn-tail recovery, WAL-only collection reconstruction,
  snapshot+WAL replay, auto-snapshot compaction, end-to-end hybrid queries,
  drop/cleanup, corrupt snapshot rejection, name validation, upsert
  semantics, garbage-WAL rejection, torn-first-frame reset.
- **Benchmarks (Criterion)** in `benchmarks/`: top-K latency at 10k/50k/100k
  records (filtered + unfiltered) and insert throughput under all three
  sync modes.
- **Docs**: `README.md`, `docs/format.md`, `docs/wal.md`, `docs/query.md`,
  `sdk/README.md` (PyO3 plan).
- **Docker**: multi-stage image (rust:1.97-slim builder → debian bookworm
  slim runtime), `cargo build --release -p novadb-cli --bin novadb`
  (workspace paths need `-p`), volume-friendly `ENTRYPOINT novadb`.
- **CI** (`.github/workflows/ci.yml`): fmt check, clippy `-D warnings`,
  full test run, release build, benchmark smoke test.
- **Example**: `examples/basic.rs` — full lifecycle demo.
- **Error handling**: typed `NovaError` (Io, Corrupt, CollectionNotFound,
  CollectionExists, RecordNotFound, DimMismatch, EmptyVector, Parse,
  Encoding) with `Display`; collection names validated
  (`[a-zA-Z0-9_-]{1,64}`) against path tricks.

---

## 2. Verification results (run 2026-08-06)

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | 61 passed, 0 failed |
| `cargo build --release --workspace` | OK |

### CLI end-to-end (13 scenarios, all passed)
create/list, duplicate-create rejection, auto + explicit ids, upsert,
hybrid query correctness, int-vs-float numeric comparison, pure vector
search, malformed input errors (dim mismatch, bad vector, bad operator,
bad JSON), idempotent delete, records listing, checkpoint + WAL compaction
to 0 bytes, checksum-corruption detection + restore, drop collection.

### Benchmark numbers (dim 128, release)
```
search_top_k/unfiltered/10000    ~2.7 ms
search_top_k/filtered/10000      ~1.5 ms
search_top_k/unfiltered/50000    ~19.7 ms
search_top_k/filtered/50000      ~8.6 ms
search_top_k/unfiltered/100000   ~27.7 ms
search_top_k/filtered/100000     ~17.5 ms
insert/sync=Never                ~60 µs
insert/sync=Batch(1024)          ~24 µs
insert/sync=Every                ~5.7 ms   (dominated by fsync)
```
Linear scaling, as expected for flat search; filters speed up queries
via early rejection.

### Docker (daemon started by user, then verified)
Image builds; create/insert/query/checkpoint/list all work in containers;
data persists across container runs via a bind-mounted volume; host shows
`products.nova` + `wal/products.log`.

---

## 3. Design decisions worth remembering

1. **WAL is the source of truth; snapshots are compacted checkpoints** —
   the classic WAL-first architecture, and the simplest one that is fully
   crash-safe.
2. **Metadata as JSON string on disk** (not bincode `Value`) — a bincode
   limitation that became a clean design point: JSON is the metadata
   interchange format at every layer.
3. **`Create` as the first WAL frame** — solves reconstructing collections
   that never reached a snapshot.
4. **Strict corruption policy**: torn tails recover silently (they are
   expected after a crash), but unrecognizable content fails loud.
5. **CLI checkpoints on every close** — each invocation is durable; crash
   recovery matters at the library level (covered by integration tests
   using `mem::forget` to skip the checkpoint).

---

## 4. Not done / deferred (intentionally)

- **Python SDK (PyO3)** — deferred by decision; plan in `sdk/README.md`.
- Approximate indexes (HNSW/IVF/PQ), SQL, replication, full-text search —
  out of scope for v0.1 per `task.md`.
- No git repository initialized yet (directory is not a repo); first commit
  still pending.

---

## 5. Repository layout (final)

```text
Cargo.toml              # workspace + lib package
src/
├── lib.rs
├── error.rs
├── config.rs           # DbConfig, SyncMode
├── storage/            # format.rs (binary format + CRC32), collection.rs
├── wal/                # frame.rs (encoding), log.rs (append/replay/truncate)
├── vector/             # cosine.rs, flat.rs (top-K)
├── metadata/           # filter.rs (AST), parse.rs (DSL parser)
└── engine/             # database.rs (lifecycle/WAL orchestration), query.rs
cli/src/main.rs         # JSON CLI
benchmarks/benches/     # search.rs, insert.rs (Criterion)
tests/integration.rs    # 13 end-to-end tests incl. crash recovery
examples/basic.rs
docs/                   # format.md, wal.md, query.md
sdk/README.md           # Python SDK plan
Dockerfile
.github/workflows/ci.yml
```

Final dependency footprint (deliberately minimal): `bincode`, `serde`,
`serde_json`, `clap` (CLI), `criterion`/`rand`/`tempfile` (dev/bench).
