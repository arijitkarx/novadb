# NovaDB — v0.1

## A Lightweight Embedded Vector Database Built from Scratch

**Tagline:** _A systems engineering project exploring storage engines, write-ahead logging, and hybrid retrieval._

---

## Vision

NovaDB is an educational, open-source embedded vector database built to understand how modern AI databases work under the hood.

Instead of relying on existing databases, NovaDB implements its own storage engine, persistence layer, vector indexing, and metadata filtering in a compact, production-inspired architecture.

**Goal:** Build a **small, complete, and well-engineered system** that demonstrates database internals and systems programming—not compete with production databases.

---

## Problem

AI applications often need to:

- Store embeddings
- Persist metadata
- Perform similarity search
- Filter results using structured attributes

NovaDB explores how these capabilities can be implemented in a single lightweight embedded database.

---

## Scope (v0.1)

### Storage Engine

- Custom binary file format
- Collections
- Persistent storage
- Write-Ahead Log (WAL)
- Crash recovery
- Periodic snapshots

### Vector Search

- Flat (Exact) Index
- Cosine similarity
- Top-K search

### Metadata Filtering

- Equality (`=`)
- Numeric comparisons (`<`, `>`, `<=`, `>=`)
- Boolean AND filters

Example:

```text
category = "electronics"
price < 1000
rating >= 4
```

### Hybrid Retrieval

Combine:

- Vector similarity
- Metadata filtering

in a single query.

### Developer Experience

- CLI
- Python SDK
- Simple API

---

# Out of Scope (Intentionally)

To keep the project focused and complete, NovaDB v0.1 **does not include**:

- SQL
- Distributed systems
- Replication
- Raft
- Kubernetes
- S3 integration
- HNSW
- IVF / Product Quantization
- Query optimizer
- Full-text search

These are potential future extensions, not goals for the initial release.

---

# Architecture

```text
                Client
          (CLI / Python SDK)
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

---

# Tech Stack

| Layer         | Technology                                |
| ------------- | ----------------------------------------- |
| Language      | Rust                                      |
| Storage       | Custom binary format                      |
| Persistence   | Write-Ahead Log                           |
| Serialization | Protocol Buffers / bincode                |
| API           | Python bindings (PyO3) or REST (optional) |
| Testing       | Rust test framework                       |
| Benchmarking  | Criterion.rs                              |
| Packaging     | Docker                                    |

---

# Repository Structure

```text
novadb/

├── storage/
├── wal/
├── vector/
├── metadata/
├── engine/
├── cli/
├── sdk/
├── benchmarks/
├── tests/
├── docs/
└── examples/
```

---

# Milestones

### Milestone 1 — Storage

- Collections
- Binary file format
- CRUD
- Persistence

**Deliverable:** Embedded database with durable storage.

---

### Milestone 2 — Durability

- WAL
- Recovery
- Snapshots

**Deliverable:** Crash-safe database.

---

### Milestone 3 — Retrieval

- Exact vector search
- Metadata filtering
- Hybrid queries

**Deliverable:** Functional retrieval engine.

---

### Milestone 4 — Developer Experience

- CLI
- Python SDK
- Benchmarks
- Documentation

**Deliverable:** Public v0.1 release.

---

# Success Criteria

### Functional

- Persistent collections
- Crash recovery via WAL
- Exact Top-K vector search
- Metadata filtering
- Hybrid search
- Python SDK
- CLI

### Engineering

- Comprehensive documentation
- Benchmark suite
- Unit & integration tests
- CI pipeline
- Docker support

---

# What This Demonstrates

NovaDB showcases practical knowledge of:

- Storage engine design
- Binary file formats
- Write-Ahead Logging (WAL)
- Crash recovery
- Vector similarity search
- Metadata indexing
- Systems programming in Rust
- API and SDK design
- Performance benchmarking
- Open-source software engineering

---

# Positioning

> **NovaDB is an educational embedded vector database built in Rust to explore storage engine architecture, durability through Write-Ahead Logging, and hybrid retrieval combining vector similarity with metadata filtering.**

Its purpose is to demonstrate **systems engineering fundamentals** through a focused, complete implementation rather than reproducing the full feature set of production databases.

---

## Why this scope is strong

This version is intentionally **small enough to finish in 4–6 weeks** while still being technically impressive. It has a clear beginning and end, can be fully documented and tested, and provides a polished artifact for your resume, GitHub, and LinkedIn. Most importantly, it leaves room for future versions (e.g., HNSW, SQL, columnar storage, distributed execution) without compromising the completeness of the initial release.

Session New session - 2026-08-05T19:22:56.823Z
Continue opencode -s ses_02c9f0248ffeKVS18MqOKOgvbF
