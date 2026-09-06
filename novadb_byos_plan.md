# NovaDB BYOS Build Plan

## Product vision

NovaDB should become a self-hostable, bring-your-own-storage vector database:

> Bring your own S3-compatible or Google Cloud Storage bucket. NovaDB provides the vector-database engine, indexing, retrieval, persistence, and recovery management.

The customer owns the bucket, credentials, infrastructure, and data. NovaDB supplies a containerized server and client interfaces that operate within the customer's environment.

## Current codebase snapshot

Repository inspected: [arijitkarx/novadb](https://github.com/arijitkarx/novadb), `main` branch.

Already present:

- Rust embedded database library.
- Custom binary snapshot format.
- WAL-first persistence and crash recovery.
- Torn-tail handling and periodic snapshots.
- Exact cosine top-K vector search.
- JSON metadata filters and hybrid vector-plus-filter queries.
- CLI, Rust API, integration tests, benchmarks, examples, and documentation.
- Dockerfile for the current local-disk CLI workflow.

Not yet present for the BYOS product:

- HTTP server/API.
- Storage backend abstraction.
- S3/GCS object-storage backend.
- Remote snapshot/WAL/segment layout.
- Authentication and tenant/database isolation.
- Server configuration and secret handling.
- Self-host deployment instructions and health checks.
- Python SDK implementation.
- Public release packaging and versioned images.

The current Dockerfile packages the CLI and uses `/data`; it is not yet a long-running HTTP service. The existing project brief also lists REST, Python SDK, S3 integration, and approximate indexes as future work.

## Target architecture

```text
Customer application
        |
        | HTTP/JSON or Python SDK
        v
NovaDB server container
        |
        +-- Query engine
        +-- Local cache and search index
        +-- Local write buffer/WAL
        +-- StorageBackend abstraction
                    |
                    +-- Local filesystem
                    +-- S3-compatible object storage
                    +-- Google Cloud Storage
```

The object store is the durable source of truth. Local disk is a rebuildable cache and working area for low-latency search. Queries should not download every vector from the bucket.

## Build priorities

### P0 — Required for the first credible BYOS release

#### 1. Define the product contract

- [ ] Rename the product description from “embedded vector database” to “self-hostable BYOS vector database” while preserving the embedded library mode.
- [ ] Document the initial deployment model: one NovaDB server process and one writer per database/prefix.
- [ ] Clearly document that v1 is not distributed or multi-writer.
- [ ] Define supported vector dimensions, distance metric, metadata types, payload limits, and collection naming rules.

#### 2. Introduce a storage abstraction

- [ ] Create a `StorageBackend` interface for reading, writing, listing, deleting, and conditionally committing objects.
- [ ] Keep the current filesystem backend working for local development and backward compatibility.
- [ ] Add an object-storage backend using a common Rust object-store interface where practical.
- [ ] Add provider configuration for `local`, `s3`, and `gcs`.
- [ ] Add a fake/in-memory backend for deterministic tests.
- [ ] Add storage contract tests that every backend must pass.

#### 3. Design the remote object layout

Proposed layout:

```text
<bucket>/<prefix>/
  databases/<database-id>/
    manifest.json
    snapshots/<generation>.nova
    wal/<sequence>-<uuid>.log
    segments/<segment-id>.nova
    indexes/<index-version>/
    metadata/config.json
```

- [ ] Use immutable names for WAL segments, snapshots, and index artifacts.
- [ ] Make the manifest identify the latest committed generation.
- [ ] Add checksums, format versions, dimensions, and creation metadata to every artifact.
- [ ] Make all writes idempotent where possible.
- [ ] Update the manifest with a compare-and-swap/conditional-write operation.
- [ ] Never append to one long-lived remote object.

#### 4. Implement remote durability and recovery

- [ ] Write operations to the local WAL before applying them to memory.
- [ ] Upload completed WAL segments to object storage.
- [ ] Periodically compact WAL segments into immutable snapshots.
- [ ] Upload a rebuilt search index or make index rebuilding deterministic from snapshots and WAL.
- [ ] On startup, load the manifest, snapshot, index, and unapplied WAL segments.
- [ ] Detect missing, corrupt, or incompatible artifacts and fail with actionable errors.
- [ ] Add `backup`, `restore`, `verify`, and `rebuild-index` commands.
- [ ] Define behavior when object storage is temporarily unavailable.

#### 5. Build the HTTP server

Add a separate server binary so the Rust library, CLI, and server remain cleanly separated.

Minimum endpoints:

```text
GET    /health
GET    /ready
GET    /v1/info
POST   /v1/databases
GET    /v1/databases
POST   /v1/databases/{db}/collections
GET    /v1/databases/{db}/collections
DELETE /v1/databases/{db}/collections/{collection}
POST   /v1/databases/{db}/collections/{collection}/vectors
PUT    /v1/databases/{db}/collections/{collection}/vectors/{id}
DELETE /v1/databases/{db}/collections/{collection}/vectors/{id}
POST   /v1/databases/{db}/collections/{collection}/query
```

- [ ] Return stable JSON error objects.
- [ ] Validate vector dimensions before touching storage.
- [ ] Support batch upsert and batch query.
- [ ] Add request IDs and structured logs.
- [ ] Make the server listen on the `PORT` environment variable.
- [ ] Add OpenAPI documentation and an interactive API page.

#### 6. Add secure customer configuration

- [ ] Support environment variables and a non-secret config file.
- [ ] Support AWS IAM roles/service accounts when available.
- [ ] Support access-key credentials only through environment variables or secret managers.
- [ ] Never log secret values.
- [ ] Allow a bucket prefix so NovaDB cannot touch unrelated customer objects.
- [ ] Add startup validation for bucket access, read/write permissions, and provider configuration.
- [ ] Add API-key authentication for the HTTP server.
- [ ] Add per-request body size, batch size, and timeout limits.
- [ ] Add basic rate limiting or document the reverse-proxy requirement.

#### 7. Package the self-hosted experience

- [ ] Replace the CLI-only Docker workflow with a server image while keeping a CLI image or CLI mode.
- [ ] Publish versioned images to GitHub Container Registry.
- [ ] Add `docker compose` examples for local disk, MinIO, S3, and GCS.
- [ ] Provide a one-command local quick start.
- [ ] Provide a production deployment example for Docker Compose.
- [ ] Provide a Cloud Run example that uses external object storage and treats local disk as cache.
- [ ] Add graceful shutdown and startup recovery behavior.
- [ ] Add readiness checks that verify the database has loaded successfully.

#### 8. Prove it with tests

- [ ] Run the same storage contract tests against local storage and MinIO.
- [ ] Add integration tests for upload, restart, recovery, compaction, and restore.
- [ ] Add failure tests for interrupted uploads, stale manifests, corrupt snapshots, and missing WAL segments.
- [ ] Add concurrent-read tests.
- [ ] Add single-writer conflict tests and return a clear error for a second writer.
- [ ] Add API tests for authentication, validation, batching, and error responses.
- [ ] Add CI for formatting, linting, unit tests, integration tests, and Docker builds.

### P1 — Required for a polished and usable release

#### 9. Developer experience

- [ ] Implement the Python SDK.
- [ ] Add official Rust and Python examples for a RAG workflow.
- [ ] Add a typed query request model and documented filter syntax.
- [ ] Add collection statistics and database status endpoints.
- [ ] Add pagination for collection/vector management operations.
- [ ] Add import/export from JSONL and Parquet where useful.
- [ ] Add a small admin CLI for setup, status, backup, restore, and repair.

#### 10. Operations and observability

- [ ] Expose Prometheus metrics: request count, latency, search count, vector count, WAL size, snapshot age, cache hits, and storage errors.
- [ ] Add structured logs with database, collection, request ID, and operation type.
- [ ] Add a `/metrics` endpoint.
- [ ] Add configurable compaction thresholds.
- [ ] Add storage retry and exponential backoff policies.
- [ ] Add a clear degraded/read-only mode when storage writes fail.
- [ ] Document bucket versioning, encryption, retention, and backup recommendations.

#### 11. Retrieval improvements

- [ ] Keep exact flat search as the correctness baseline.
- [ ] Add HNSW as the first approximate index only after remote durability is stable.
- [ ] Add index versioning and rebuild status.
- [ ] Benchmark recall versus latency for exact and approximate search.
- [ ] Add optional payload/document storage only if it supports a clear RAG use case.

#### 12. Documentation and trust

- [ ] Rewrite the README around BYOS deployment rather than only educational internals.
- [ ] Add an architecture diagram and data-flow diagram.
- [ ] Add a security model and required bucket permissions.
- [ ] Add a compatibility matrix for AWS S3, MinIO, R2, B2, Wasabi, and GCS.
- [ ] Add an explicit limitations page: one writer, no replication, no SLA, no managed hosting.
- [ ] Add a disaster-recovery guide.
- [ ] Add a migration guide from the current local `.nova` and WAL layout.
- [ ] Add release notes and semantic versioning.
- [ ] Correct the repository URL in `Cargo.toml` before publishing.

### P2 — Future product expansion

- [ ] Multi-writer coordination and distributed locking.
- [ ] Replication and high availability.
- [ ] Raft or another consensus protocol.
- [ ] Kubernetes Helm chart and operator.
- [ ] Multi-tenant hosted control plane.
- [ ] Billing, usage metering, and hosted support.
- [ ] SQL-like query interface.
- [ ] Full-text and hybrid lexical search.
- [ ] Additional cloud-native index and storage optimizations.

## Recommended first release boundary

Call the first product release `NovaDB BYOS v0.1` and limit it to:

- One server process per database.
- One writer per database.
- Local, S3-compatible, and GCS storage providers.
- REST/JSON API.
- Exact vector search.
- Metadata filtering and hybrid queries.
- WAL-backed recovery.
- Snapshot and restore.
- Docker Compose deployment.
- API-key authentication.
- Python client.
- MinIO integration tests.

Do not include replication, Kubernetes, multi-writer support, SQL, or a hosted control plane in this release.

## Definition of done

A new user should be able to:

1. Create a private bucket or bucket prefix.
2. Start NovaDB with Docker.
3. Configure the provider and credentials without modifying source code.
4. Create a collection and insert vectors through the API.
5. Run filtered similarity queries from Python or curl.
6. Stop and restart the container.
7. Observe that the database recovers from the bucket.
8. Back up, restore, and verify the database.
9. Understand the single-writer limitation from the documentation.
10. Remove the container while retaining the data in their own bucket.

## Showcase/demo scenario

Use a small RAG catalog rather than random vectors:

1. Start NovaDB with a MinIO bucket using Docker Compose.
2. Ingest product or document embeddings with metadata such as category, tenant, language, and price.
3. Run a similarity query with a metadata filter.
4. Show the object layout in the bucket.
5. Stop and restart NovaDB.
6. Show recovery and the same query returning the same result.
7. Run the same setup against an S3 or GCS bucket by changing configuration only.

The portfolio message is:

> I built a self-hostable vector database that lets teams keep their data in their own object storage while NovaDB manages indexing, filtered retrieval, persistence, and crash recovery.

## Immediate next three implementation slices

1. **Server slice:** add the HTTP server, health endpoints, collection endpoints, upsert, and query.
2. **Storage slice:** extract the storage interface, preserve local storage, add S3-compatible storage, and implement manifest-based recovery.
3. **Release slice:** add API keys, Docker Compose with MinIO, integration tests, Python client, and BYOS documentation.

