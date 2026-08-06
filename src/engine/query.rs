//! The hybrid query API: vector similarity combined with metadata filtering.
//!
//! A query is built fluently and executed against a [`Database`]:
//!
//! ```no_run
//! use novadb::{Database, DbConfig};
//! use novadb::engine::query::Query;
//!
//! # fn main() -> novadb::Result<()> {
//! let mut db = Database::open(DbConfig::new("/tmp/novadb-example"))?;
//! let hits = Query::new("products", &[0.1, 0.2, 0.3])
//!     .top_k(5)
//!     .filter(novadb::metadata::Filter::parse("price < 1000")?)
//!     .run(&db)?;
//! # Ok(())
//! # }
//! ```
//!
//! The execution strategy is **filter-then-score**: records that fail the
//! metadata predicate are excluded before any similarity math runs, which
//! keeps hybrid search exact and cheap for selective filters.

use crate::error::Result;
use crate::metadata::filter::Filter;
use crate::vector::flat::{search_top_k, SearchHit};
use crate::Database;

/// A hybrid similarity query.
pub struct Query<'a> {
    collection: &'a str,
    vector: &'a [f32],
    top_k: usize,
    filter: Option<Filter>,
}

impl<'a> Query<'a> {
    /// Start a query against `collection` using `vector` as the probe.
    pub fn new(collection: &'a str, vector: &'a [f32]) -> Self {
        Query {
            collection,
            vector,
            top_k: 10,
            filter: None,
        }
    }

    /// Number of results to return (default 10).
    pub fn top_k(mut self, k: usize) -> Self {
        self.top_k = k;
        self
    }

    /// Restrict candidates to records matching `filter`.
    pub fn filter(mut self, filter: Filter) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Execute the query.
    pub fn run(&self, db: &Database) -> Result<Vec<SearchHit>> {
        let collection = db.collection(self.collection)?;
        collection.check_vector(self.vector)?;
        let predicate = crate::engine::database::filter_predicate(self.filter.as_ref());
        Ok(search_top_k(collection, self.vector, self.top_k, predicate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DbConfig;
    use serde_json::json;

    fn db_with_records() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::open(DbConfig::new(dir.path())).unwrap();
        db.create_collection("items", 2).unwrap();
        db.add(
            "items",
            &[1.0, 0.0],
            json!({"category": "electronics", "price": 900}),
        )
        .unwrap();
        db.add(
            "items",
            &[0.9, 0.1],
            json!({"category": "electronics", "price": 1200}),
        )
        .unwrap();
        db.add(
            "items",
            &[0.0, 1.0],
            json!({"category": "books", "price": 20}),
        )
        .unwrap();
        db.close().unwrap();
        dir
    }

    #[test]
    fn hybrid_query_filters_before_scoring() {
        let dir = db_with_records();
        let db = Database::open(DbConfig::new(dir.path())).unwrap();

        let unfiltered = Query::new("items", &[1.0, 0.0]).top_k(10).run(&db).unwrap();
        assert_eq!(unfiltered.len(), 3);
        assert_eq!(unfiltered[0].id, 1);

        let filtered = Query::new("items", &[1.0, 0.0])
            .top_k(10)
            .filter(Filter::parse(r#"price < 1000"#).unwrap())
            .run(&db)
            .unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered
            .iter()
            .all(|h| h.record.metadata["price"].as_f64() < Some(1000.0)));
        assert_eq!(filtered[0].id, 1);

        let none = Query::new("items", &[1.0, 0.0])
            .top_k(10)
            .filter(Filter::parse(r#"category = "garden""#).unwrap())
            .run(&db)
            .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn query_rejects_bad_dimension() {
        let dir = db_with_records();
        let db = Database::open(DbConfig::new(dir.path())).unwrap();
        let err = Query::new("items", &[1.0]).top_k(1).run(&db).unwrap_err();
        assert!(matches!(err, crate::error::NovaError::DimMismatch { .. }));
    }

    #[test]
    fn query_missing_collection() {
        let dir = db_with_records();
        let db = Database::open(DbConfig::new(dir.path())).unwrap();
        let err = Query::new("nope", &[1.0, 2.0]).run(&db).unwrap_err();
        assert!(matches!(
            err,
            crate::error::NovaError::CollectionNotFound(_)
        ));
    }
}
