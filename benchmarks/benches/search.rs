//! Flat top-K search latency at various collection sizes, with and without
//! metadata filters.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use novadb::engine::query::Query;
use novadb::metadata::Filter;
use novadb::{Database, DbConfig, SyncMode};
use rand::distributions::Uniform;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const DIM: usize = 128;

/// A deterministic PRNG makes benchmark data reproducible.
fn seeded_rng() -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(0xC0FFEE)
}

/// Build an in-memory DB with `n` random unit-ish vectors plus metadata.
/// The temp dir and DB live for the process's lifetime (leaked on purpose).
fn build_db(n: usize) -> Database {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let config = DbConfig::new(dir.path())
        .with_sync_mode(SyncMode::Never)
        .with_auto_snapshot_after(u64::MAX);
    let mut db = Database::open(config).unwrap();
    db.create_collection("bench", DIM).unwrap();
    let mut rng = seeded_rng();
    let dist = Uniform::new_inclusive(-1.0f32, 1.0);
    for i in 0..n {
        let vector: Vec<f32> = (0..DIM).map(|_| rng.sample(dist)).collect();
        let category = if i % 3 == 0 { "a" } else { "b" };
        let price = rng.sample(Uniform::new_inclusive(0.0f32, 10_000.0));
        db.add(
            "bench",
            &vector,
            serde_json::json!({"cat": category, "price": price}),
        )
        .unwrap();
    }
    // The temp dir and DB live for the whole benchmark run.
    db
}

fn bench_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("search_top_k");
    group.sample_size(50);

    for n in [10_000, 50_000, 100_000] {
        let db = build_db(n);
        let probe: Vec<f32> = (0..DIM).map(|i| (i as f32) / DIM as f32).collect();

        group.bench_with_input(BenchmarkId::new("unfiltered", n), &n, |b, _| {
            b.iter(|| {
                let hits = Query::new("bench", &probe)
                    .top_k(10)
                    .run(black_box(&db))
                    .unwrap();
                black_box(hits.len())
            });
        });

        let filter = Filter::parse(r#"cat = "a" AND price < 5000"#).unwrap();
        group.bench_with_input(BenchmarkId::new("filtered", n), &n, |b, _| {
            b.iter(|| {
                let query = Query::new("bench", &probe).top_k(10).filter(filter.clone());
                let hits = query.run(black_box(&db)).unwrap();
                black_box(hits.len())
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_search);
criterion_main!(benches);
