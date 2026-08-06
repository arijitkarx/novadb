//! Insert throughput: sequential `add` calls under different WAL sync modes.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use novadb::{Database, DbConfig, SyncMode};
use rand::distributions::Uniform;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const DIM: usize = 128;

fn bench_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert");

    for mode in [SyncMode::Never, SyncMode::Batch(1024), SyncMode::Every] {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let config = DbConfig::new(dir.path())
            .with_sync_mode(mode)
            .with_auto_snapshot_after(u64::MAX);
        let mut db = Database::open(config).unwrap();
        db.create_collection("bench", DIM).unwrap();

        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let dist = Uniform::new_inclusive(-1.0f32, 1.0);

        group.bench_function(format!("sync={mode:?}"), |b| {
            b.iter(|| {
                let vector: Vec<f32> = (0..DIM).map(|_| rng.sample(dist)).collect();
                let id = db
                    .add("bench", &vector, serde_json::json!({"bench": true}))
                    .unwrap();
                black_box(id)
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_insert);
criterion_main!(benches);
