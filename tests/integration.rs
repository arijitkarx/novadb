//! End-to-end integration tests: persistence, crash recovery, and retrieval.

use std::fs::OpenOptions;
use std::io::Write;
use std::mem;

use novadb::engine::query::Query;
use novadb::metadata::Filter;
use novadb::{Database, DbConfig, NovaError, SyncMode};
use serde_json::json;

fn config(dir: &std::path::Path) -> DbConfig {
    DbConfig::new(dir).with_sync_mode(SyncMode::Every)
}

fn seed(dir: &std::path::Path) {
    let mut db = Database::open(config(dir)).unwrap();
    db.create_collection("items", 2).unwrap();
    db.add("items", &[1.0, 0.0], json!({"cat": "a", "price": 10}))
        .unwrap();
    db.add("items", &[0.9, 0.1], json!({"cat": "a", "price": 20}))
        .unwrap();
    db.add("items", &[0.0, 1.0], json!({"cat": "b", "price": 30}))
        .unwrap();
    db.close().unwrap();
}

#[test]
fn crud_and_persistence_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());

    let mut db = Database::open(config(dir.path())).unwrap();
    assert_eq!(db.list_collections(), vec!["items".to_string()]);
    let stats = db.stats("items").unwrap();
    assert_eq!(stats.records, 3);
    assert_eq!(stats.dim, 2);

    // Delete then reopen; deletion must survive.
    assert!(db.delete("items", 1).unwrap());
    assert!(!db.delete("items", 1).unwrap());
    db.close().unwrap();

    let db = Database::open(config(dir.path())).unwrap();
    assert_eq!(db.stats("items").unwrap().records, 2);
    assert!(db.get("items", 1).unwrap().is_none());
    assert!(db.get("items", 2).unwrap().is_some());
}

#[test]
fn ungraceful_shutdown_recovers_from_wal() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut db = Database::open(config(dir.path())).unwrap();
        db.create_collection("c", 2).unwrap();
        for i in 0..50u64 {
            db.add("c", &[i as f32, 1.0], json!({"i": i})).unwrap();
        }
        // Simulate a crash: drop without close/checkpoint.
        mem::forget(db);
    }
    let db = Database::open(config(dir.path())).unwrap();
    assert_eq!(db.stats("c").unwrap().records, 50);
    assert_eq!(db.get("c", 49).unwrap().unwrap().id, 49);
}

#[test]
fn torn_wal_tail_is_dropped_during_recovery() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut db = Database::open(config(dir.path())).unwrap();
        db.create_collection("c", 2).unwrap();
        for i in 0..20u64 {
            db.add("c", &[i as f32, 0.0], json!({"i": i})).unwrap();
        }
        mem::forget(db); // no checkpoint
    }
    // Simulate a crash mid-write: chop the tail of the WAL.
    let wal_path = dir.path().join("wal/c.log");
    let len = std::fs::metadata(&wal_path).unwrap().len();
    let file = OpenOptions::new().write(true).open(&wal_path).unwrap();
    file.set_len(len - 7).unwrap();
    drop(file);

    let db = Database::open(config(dir.path())).unwrap();
    // All fully-written frames must be recovered; the torn one is dropped.
    let stats = db.stats("c").unwrap();
    assert!(
        stats.records >= 18 && stats.records <= 19,
        "got {}",
        stats.records
    );
}

#[test]
fn wal_only_collection_reconstructs_after_crash() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut db = Database::open(config(dir.path())).unwrap();
        db.create_collection("fresh", 3).unwrap(); // never checkpointed
        db.add("fresh", &[1.0, 2.0, 3.0], json!({"k": "v"}))
            .unwrap();
        mem::forget(db);
    }
    // No snapshot file exists yet.
    assert!(!dir.path().join("fresh.nova").exists());

    let db = Database::open(config(dir.path())).unwrap();
    assert_eq!(db.list_collections(), vec!["fresh".to_string()]);
    let stats = db.stats("fresh").unwrap();
    assert_eq!(stats.records, 1);
    assert_eq!(stats.dim, 3);
    let record = db.get("fresh", 1).unwrap().unwrap();
    assert_eq!(record.metadata, json!({"k": "v"}));
}

#[test]
fn snapshot_plus_later_wal_ops_recover_together() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut db = Database::open(config(dir.path())).unwrap();
        db.create_collection("c", 2).unwrap();
        for i in 0..10u64 {
            db.add("c", &[i as f32, 0.0], json!({"i": i})).unwrap();
        }
        db.snapshot("c").unwrap(); // checkpoint: 10 records durable in .nova
        assert_eq!(
            std::fs::metadata(dir.path().join("wal/c.log"))
                .unwrap()
                .len(),
            0
        );
        for i in 10..15u64 {
            db.add("c", &[i as f32, 0.0], json!({"i": i})).unwrap();
        }
        db.delete("c", 3).unwrap();
        mem::forget(db); // crash after the snapshot
    }
    let db = Database::open(config(dir.path())).unwrap();
    let stats = db.stats("c").unwrap();
    // 15 added, 1 deleted => 14 live records.
    assert_eq!(stats.records, 14);
    assert!(db.get("c", 3).unwrap().is_none());
    assert!(db.get("c", 14).unwrap().is_some());
}

#[test]
fn auto_snapshot_compacts_wal_during_writes() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path());
    cfg.auto_snapshot_after = 5;
    let mut db = Database::open(cfg.clone()).unwrap();
    db.create_collection("c", 2).unwrap();
    for i in 0..12u64 {
        db.add("c", &[i as f32, 0.0], json!({"i": i})).unwrap();
    }
    // After crossing the threshold the snapshot exists and the WAL is short.
    assert!(dir.path().join("c.nova").exists());
    let wal_len = std::fs::metadata(dir.path().join("wal/c.log"))
        .unwrap()
        .len();
    assert!(wal_len > 0 && wal_len < 512);
    mem::forget(db);

    let db = Database::open(cfg).unwrap();
    assert_eq!(db.stats("c").unwrap().records, 12);
}

#[test]
fn hybrid_query_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::open(config(dir.path())).unwrap();
    db.create_collection("products", 2).unwrap();
    for (i, (price, cat)) in [
        (900.0, "electronics"),
        (1200.0, "electronics"),
        (20.0, "books"),
        (1500.0, "books"),
    ]
    .iter()
    .enumerate()
    {
        let angle = i as f32 * 0.3;
        db.add(
            "products",
            &[angle.cos(), angle.sin()],
            json!({"category": cat, "price": price}),
        )
        .unwrap();
    }

    let hits = Query::new("products", &[1.0, 0.0])
        .top_k(2)
        .filter(Filter::parse(r#"category = "electronics""#).unwrap())
        .run(&db)
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits
        .iter()
        .all(|h| h.record.metadata["category"] == "electronics"));
    assert!(hits[0].score >= hits[1].score);

    // Price filter excludes the cheap electronics item.
    let hits = Query::new("products", &[1.0, 0.0])
        .top_k(10)
        .filter(Filter::parse("price < 1000").unwrap())
        .run(&db)
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits
        .iter()
        .all(|h| h.record.metadata["price"].as_f64().unwrap() < 1000.0));

    // Combining both filters leaves exactly one record.
    let hits = Query::new("products", &[1.0, 0.0])
        .top_k(10)
        .filter(Filter::parse(r#"category = "electronics" AND price < 1000"#).unwrap())
        .run(&db)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.id, 1);
}

#[test]
fn drop_collection_removes_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::open(config(dir.path())).unwrap();
    db.create_collection("gone", 2).unwrap();
    db.add("gone", &[1.0, 0.0], json!({})).unwrap();
    db.snapshot("gone").unwrap();
    assert!(dir.path().join("gone.nova").exists());

    db.drop_collection("gone").unwrap();
    assert!(!dir.path().join("gone.nova").exists());
    assert!(!dir.path().join("wal/gone.log").exists());
    assert_eq!(db.list_collections().len(), 0);

    assert!(matches!(
        db.drop_collection("gone"),
        Err(NovaError::CollectionNotFound(_))
    ));
}

#[test]
fn corrupt_snapshot_fails_open() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut db = Database::open(config(dir.path())).unwrap();
        db.create_collection("c", 2).unwrap();
        db.add("c", &[1.0, 0.0], json!({})).unwrap();
        db.close().unwrap();
    }
    // Flip a byte in the middle of the snapshot payload.
    let path = dir.path().join("c.nova");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[40] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();

    let err = match Database::open(config(dir.path())) {
        Err(e) => e,
        Ok(_) => panic!("expected corrupt snapshot to fail open"),
    };
    assert!(matches!(err, NovaError::Corrupt(_)));
}

#[test]
fn collection_name_validation() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::open(config(dir.path())).unwrap();
    assert!(db.create_collection("ok_name-2", 2).is_ok());
    for bad in ["", "..", ".", "a/b", "with space", "x".repeat(65).as_str()] {
        assert!(
            db.create_collection(bad, 2).is_err(),
            "should reject '{bad}'"
        );
    }
}

#[test]
fn insert_with_explicit_id_upserts() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::open(config(dir.path())).unwrap();
    db.create_collection("c", 2).unwrap();
    db.insert("c", 42, &[1.0, 0.0], json!({"v": 1})).unwrap();
    db.insert("c", 42, &[0.0, 1.0], json!({"v": 2})).unwrap();
    assert_eq!(db.stats("c").unwrap().records, 1);
    let record = db.get("c", 42).unwrap().unwrap();
    assert_eq!(record.vector, vec![0.0, 1.0]);
    assert_eq!(record.metadata, json!({"v": 2}));
    // Auto-ids continue after explicit ones.
    let id = db.add("c", &[1.0, 1.0], json!({})).unwrap();
    assert_eq!(id, 43);
}

#[test]
fn garbage_wal_is_rejected_loudly() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut db = Database::open(config(dir.path())).unwrap();
        db.create_collection("c", 2).unwrap();
        db.add("c", &[1.0, 0.0], json!({})).unwrap();
        db.close().unwrap();
    }
    // Replace the (now empty) WAL with unrecognizable bytes.
    let wal_path = dir.path().join("wal/c.log");
    let mut file = OpenOptions::new().write(true).open(&wal_path).unwrap();
    writeln!(file, "not a novadb wal").unwrap();
    drop(file);

    let err = match Database::open(config(dir.path())) {
        Err(e) => e,
        Ok(_) => panic!("expected garbage WAL to fail open"),
    };
    assert!(matches!(err, NovaError::Corrupt(_)));
}

#[test]
fn torn_first_frame_is_reset_and_appends_continue() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut db = Database::open(config(dir.path())).unwrap();
        db.create_collection("c", 2).unwrap();
        mem::forget(db); // only the Create frame exists
    }
    // Simulate a crash that tore the Create frame in half.
    let wal_path = dir.path().join("wal/c.log");
    let mut bytes = std::fs::read(&wal_path).unwrap();
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&wal_path, bytes).unwrap();

    // The torn Create was never durable, so the collection does not exist.
    let mut db = Database::open(config(dir.path())).unwrap();
    assert_eq!(db.list_collections(), Vec::<String>::new());

    // The reset log is clean: the database is fully usable and survives a
    // second crash.
    db.create_collection("c", 2).unwrap();
    db.add("c", &[1.0, 0.0], json!({})).unwrap();
    mem::forget(db);
    let db = Database::open(config(dir.path())).unwrap();
    assert_eq!(db.stats("c").unwrap().records, 1);
    assert!(db.get("c", 1).unwrap().is_some());
}
