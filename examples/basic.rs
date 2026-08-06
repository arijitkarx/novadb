//! Basic NovaDB usage: create a collection, add records, run a hybrid query.

use novadb::engine::query::Query;
use novadb::metadata::Filter;
use novadb::{Database, DbConfig};

fn main() -> novadb::Result<()> {
    let dir = std::env::temp_dir().join("novadb-example");
    let mut db = Database::open(DbConfig::new(&dir))?;

    // 1. A collection of 2-dimensional vectors.
    db.create_collection("products", 2)?;

    // 2. Add records with metadata (auto-assigned ids).
    db.add(
        "products",
        &[1.0, 0.0],
        serde_json::json!({
            "name": "speaker",
            "category": "electronics",
            "price": 899,
            "rating": 4.5,
        }),
    )?;
    db.add(
        "products",
        &[0.9, 0.1],
        serde_json::json!({
            "name": "headphones",
            "category": "electronics",
            "price": 1299,
            "rating": 4.8,
        }),
    )?;
    db.add(
        "products",
        &[0.0, 1.0],
        serde_json::json!({
            "name": "novel",
            "category": "books",
            "price": 19,
            "rating": 4.0,
        }),
    )?;

    // 3. Hybrid query: nearest to the probe among electronics under $1000.
    let hits = Query::new("products", &[1.0, 0.0])
        .top_k(5)
        .filter(Filter::parse(
            r#"category = "electronics" AND price < 1000"#,
        )?)
        .run(&db)?;

    println!("{} hit(s):", hits.len());
    for hit in &hits {
        println!(
            "  id={} score={:.4} name={} price={}",
            hit.id,
            hit.score,
            hit.record.metadata["name"].as_str().unwrap_or("?"),
            hit.record.metadata["price"],
        );
    }

    // 4. Point lookups and deletes.
    if let Some(record) = db.get("products", 1)? {
        println!(
            "record 1 -> {}",
            serde_json::to_string(&record.metadata).expect("metadata is valid JSON")
        );
    }
    db.delete("products", 3)?;
    println!("after delete: {} records", db.stats("products")?.records);

    // 5. Durability: snapshots + WAL. A clean close checkpoints everything.
    db.close()?;
    println!("database closed cleanly (snapshot written, WAL truncated)");

    // Reopen: everything is recovered.
    let db = Database::open(DbConfig::new(&dir))?;
    println!("reopened: {} records", db.stats("products")?.records);

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
