# NovaDB Python SDK (planned)

The v0.1 scope ships the Rust library, CLI, benchmarks, and documentation.
The Python SDK is the first item on the post-v0.1 roadmap.

## Plan

- **Bindings:** PyO3 + maturin, exposing the `Database` API (create/insert/
  query/filter) as a `novadb` Python package.
- **Distribution:** `pip install novadb` building a native extension for
  CPython 3.9+ on Linux/macOS/Windows.
- **API shape:**

  ```python
  import novadb

  db = novadb.open("/tmp/novadb")
  db.create_collection("products", dim=3)
  db.add("products", [0.1, 0.2, 0.3], {"category": "electronics", "price": 99})

  hits = db.query("products", [0.1, 0.2, 0.3], top_k=5,
                  filter='category = "electronics" AND price < 1000')
  ```

- **Testing:** pytest suite mirroring the Rust integration tests, plus a
  crash-recovery test spawning the CLI.

## Why PyO3 and not a pure-Python SDK

A pure-Python reader would re-implement (and drift from) the on-disk format
and the filter semantics. PyO3 keeps a single source of truth in Rust while
giving Python near-C performance.
