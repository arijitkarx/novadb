# NovaDB Write-Ahead Log (`wal/*.log`)

The WAL is the **source of truth for durability**. Every mutation is framed
and appended to the log before it is applied to memory; on open, the database
rebuilds state as `snapshot + WAL replay`.

## File layout

A log is a sequence of frames:

```text
┌─────────────────────┐
│ Frame 0             │
├─────────────────────┤
│ Frame 1             │
├─────────────────────┤
│ ...                 │
└─────────────────────┘
```

## Frame format

```text
┌───────┬─────────┬───────────┬──────────┬──────────┬──────────────┐
│ magic │ version │ seq u64   │ len u32  │ crc32    │ payload      │
│ 1 B   │ 2 B     │  (LE)     │ (LE)     │ (LE)     │ (len bytes)  │
└───────┴─────────┴───────────┴──────────┴──────────┴──────────────┘
```

- `magic` — `0xAD`.
- `version` — `1`.
- `seq` — monotonically increasing frame number, 0-based within the file.
- `len` — payload length.
- `crc32` — IEEE CRC-32 of the payload.
- `payload` — bincode-serialized `WalOp`.

### Operations (`WalOp`)

| Variant | Payload | Meaning |
| --- | --- | --- |
| `Create(dim)` | `u32` | Create a collection with this dimensionality |
| `Put(record)` | `PersistedRecord` | Insert or replace a record |
| `Delete(id)` | `u64` | Delete a record |

Every collection's log **begins with a `Create` frame**. This lets a
collection that was created and written but never checkpointed be fully
reconstructed after a crash.

## Sync modes

| Mode | Behavior |
| --- | --- |
| `SyncMode::Every` | fsync after every append. Acknowledged writes survive any crash. |
| `SyncMode::Batch(n)` | fsync every n appends. A crash may lose the most recent n. |
| `SyncMode::Never` | no fsync. Durability depends on OS write-back. |

## Recovery procedure

On `Database::open`:

1. Load every snapshot (`*.nova`); open its WAL.
2. Scan the WAL frame by frame:
   - a full frame with a valid magic, version, and checksum is replayed;
   - a **partial frame** (header or payload truncated) is a *torn tail* —
     the crash cut a write short. Everything before it is durable; the tail
     is truncated away so later appends don't get stranded behind it;
   - a full frame with a bad checksum is corruption — open fails loudly.
3. A log that starts with the frame magic but parses nothing is a torn first
   frame: it is reset to empty (nothing was ever durable).
4. A log with unrecognizable bytes at its start is corruption — open fails
   loudly rather than silently discarding data.
5. Replayed operations are idempotent (`Put` replaces, `Delete` is a no-op on
   a missing id), so the checkpoint sequence is crash-safe.

## Checkpointing

A snapshot is written atomically (temp + rename), then the WAL is truncated
to zero. If a crash lands between the two steps, the next open replays the
old frames on top of the new snapshot — harmless because replay is
idempotent. Checkpoints happen on `close()`, on explicit `snapshot`/
`checkpoint` commands, and automatically when a collection's `op_count`
crosses `auto_snapshot_after`.

## Failure semantics

Writes follow the WAL-first ordering: `append frame` → `fsync (optional)` →
`apply to memory`. If the append or fsync fails, the operation errors and
memory is untouched. If the OS wrote bytes that were never acknowledged
(batch/never modes), recovery applies them — the database converges to the
last durable state, never to a state that was never in the log.
