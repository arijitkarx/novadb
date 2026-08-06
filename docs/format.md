# NovaDB Snapshot Format (`*.nova`)

A snapshot is the compact, self-contained representation of one collection.
It is written atomically (write to `*.nova.tmp`, fsync, rename over the
target) and is a full state: no other file is needed to load it.

## File layout

```text
┌──────────────────────────────────────────────┐
│ Header (32 bytes)                            │
├──────────────────────────────────────────────┤
│ Block: record 0                              │
│ Block: record 1                              │
│ ...                                          │
│ Block: record N-1                            │
└──────────────────────────────────────────────┘
```

All integers are little-endian.

## Header (32 bytes)

| Offset | Size | Field | Value |
| --- | --- | --- | --- |
| 0 | 8 | Magic | `NOVA1\x00\x00\x00` |
| 8 | 4 | Version | `1` |
| 12 | 4 | Dimension `u32` | dimensionality of every vector |
| 16 | 8 | Next id `u64` | next auto-assigned record id |
| 24 | 4 | Record count `u32` | number of blocks that follow |
| 28 | 4 | Reserved | zero |

A snapshot whose magic or version does not match is rejected at load time
with a `Corrupt` error. The record count must equal the number of blocks
actually read.

## Record block

```text
┌──────────┬──────────┬──────────────────┐
│ len u32  │ crc32    │ payload (len B)  │
└──────────┴──────────┴──────────────────┘
```

- `len` — payload length in bytes.
- `crc32` — IEEE CRC-32 of the payload.
- `payload` — a bincode-serialized `PersistedRecord`.

A block whose checksum does not match is corruption and fails the load.

## Payload (`PersistedRecord`)

Serialized with bincode:

| Field | Type |
| --- | --- |
| id | `u64` |
| vector | `Vec<f32>` |
| metadata | `String` — a JSON document |

Metadata is stored as a JSON string because bincode cannot encode
`serde_json::Value` directly (it has no `deserialize_any` support); JSON is
the interchange format for metadata at every boundary — CLI input, filter
evaluation, and disk.

## Deleting records

Snapshots never contain tombstones: a deleted record is dropped at the next
checkpoint. Live deletes happen through the WAL (`Delete` frame); the
snapshot simply reflects the resulting state.
