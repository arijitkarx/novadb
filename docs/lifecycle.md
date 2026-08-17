# Lifecycle: Entry, Data Flow, and Exit

How a process moves through NovaDB: from the CLI entry point, through
`Database::open` and crash recovery, down the WAL-first write path, out
through queries, and into snapshots on shutdown.

```text
args ──► DbConfig ──► open (snapshot load + WAL replay)
                          │
       ┌──────────────────┼──────────────────────┐
       ▼                  ▼                      ▼
   write flow         read flow               shutdown
   (WAL-first)     (filter → flat scan)   (snapshot → truncate)
       │                  │                      │
       └──────────────────┴──────────────────────┘
                    on-disk files
```

## 1. Entry point — the CLI (`cli/src/main.rs`)

- `main()` at `cli/src/main.rs:89` — clap parses argv into `Cli` (flags
  `--db`, `--no-sync`, `--batch`) and a `Command` enum (`Create`, `Insert`,
  `Query`, … defined at `main.rs:40-87`).
- It maps CLI flags to `SyncMode` (`main.rs:92-96`): `Every` (fsync each
  write, default), `Batch(n)` (fsync every `n` writes), `Never`.
- `run()` at `main.rs:111` is the actual dispatch: it opens the database
  once, matches the subcommand, produces a `serde_json::Value`, then calls
  `db.close()` at `main.rs:216` and pretty-prints JSON.
- Exit: `ExitCode::SUCCESS`/`FAILURE` at `main.rs:99-108` — JSON goes to
  stdout, `error: {err}` to stderr. Every CLI command is one
  open → op → close cycle.

The library entry point is `Database::open` itself (`src/lib.rs:37-48`
re-exports `Database`, `DbConfig`, `NovaError`, `Record`).

## 2. Open flow — `Database::open` (`src/engine/database.rs:59`)

1. `create_dir_all` for the data dir and `wal/` (`database.rs:60-61`).
2. `load_snapshots()` (`database.rs:80`) — scans `*.nova` files. Each
   snapshot is deserialized by `Collection::load_snapshot`
   (`src/storage/collection.rs:159`): a 32-byte header (magic
   `NOVA1\x00\x00\x00`, version, dim, next id, record count — see
   `docs/format.md`) followed by length-prefixed, CRC32-checksummed record
   blocks. Vector norms are recomputed on load.
3. `replay_wal()` (`database.rs:142`) — the crash-recovery core:
   - `WalLog::read_frames` (`src/wal/log.rs:118`) decodes frame after frame;
     `decode_frame` (`src/wal/frame.rs:68`) checks magic `0xAD`, version,
     length, and CRC. A partial frame is a `TornTail` — parsing stops there.
   - If parsing stopped short of the file length, the torn tail is truncated
     off (`database.rs:174-177`). A file that is entirely unrecognizable is
     a hard `Corrupt` error; a file that begins with frame magic but parsed
     nothing is a torn *first* frame (nothing durable) and gets reset
     (`database.rs:150-173`).
   - Each frame is replayed through `apply_frame` (`database.rs:197`):
     `Create` materializes the collection, `Put` inserts a record, `Delete`
     removes one. Ops are idempotent, which makes snapshot-then-truncate a
     safe checkpoint.
   - The frame counter and op counter are reset so a freshly opened database
     does not immediately auto-snapshot (`database.rs:191-193`).
4. `load_wal_only_collections()` (`database.rs:107`) — collections created
   but never checkpointed exist only as WAL segments; their `Create` frame
   brings them back.

## 3. Write flow — WAL-first

Take `db.add(...)` (`database.rs:301`):

1. `check_vector` validates dimensionality (`collection.rs:139`).
2. `alloc_id` bumps `next_id` (`collection.rs:117`).
3. `wal.append_put` (`src/wal/log.rs:60`) → `append` (`wal/log.rs:76`):
   serializes the op with bincode, writes
   `magic | version | seq | len | crc32 | payload` to the append-only log,
   then fsyncs according to `SyncMode`.
4. **Only then** `collection.apply_put` (`collection.rs:78`) mutates memory:
   insert, replace-in-place, or swap-remove on delete (`apply_delete`,
   `collection.rs:98` — the tail-swap keeps the id→index map consistent).
   The L2 norm is recomputed.
5. `maybe_auto_snapshot` (`database.rs:378`) — if ops since the last
   snapshot reach the threshold (default 10,000), the collection is
   compacted.

`create_collection` (`database.rs:248`) writes a `Create` frame first;
`delete` (`database.rs:322`) is WAL-first too. This ordering is the
durability contract: a mutation is durable (or at least log-appended) before
it touches memory, so a crash can always rebuild state as
snapshot + WAL replay.

## 4. Read flow — `Query::run` (`src/engine/query.rs:60`)

1. Fetch the `Collection`, validate the probe vector's dimensionality.
2. `filter_predicate` (`database.rs:432`) turns `Option<Filter>` into a
   closure over metadata; `Filter::matches` (`src/metadata/filter.rs:32`)
   evaluates the AST (`Compare` / `And`). See `docs/query.md` for the DSL.
3. `search_top_k` (`src/vector/flat.rs:52`) — brute-force scan with a
   `BinaryHeap<Reverse<Scored>>` of capacity `k + 1`: each record that
   passes the predicate is cosine-scored (`flat.rs:94`, using precomputed
   norms, so one dot product per record), pushed, and the worst candidate
   evicted. Results come back sorted by score descending, id ascending.

## 5. Exit flows — shutdown and durability

- Explicit: `close()` (`database.rs:373`) → `snapshot_all` →
  `snapshot(name)` (`database.rs:354`): `write_snapshot_atomic`
  (`collection.rs:225`) writes `<name>.nova.tmp`, fsyncs, renames over the
  live file; then `wal.truncate()` (`wal/log.rs:96`) zeroes the log. A
  crash between rename and truncate is harmless — replaying an empty or
  stale WAL over a fresh snapshot is idempotent.
- Implicit: `impl Drop for Database` (`database.rs:407`) performs a
  best-effort `snapshot_all` when the handle is dropped without `close()`
  (e.g. during panic unwinding).
- Auto: `maybe_auto_snapshot` (`database.rs:378`) checkpoints mid-run when
  the WAL grows past the threshold.

## File layout on disk

```text
<dir>/
├── <collection>.nova        # snapshot: header + record blocks
└── wal/
    └── <collection>.log     # WAL: append-only frames
```

Details of both formats live in `docs/format.md` and `docs/wal.md`.
