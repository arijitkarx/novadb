//! Exact (flat) top-K similarity search.
//!
//! A brute-force scan over every record, scoring with cosine similarity.
//! Cheap, exact, and the right baseline for a v0.1 database; approximate
//! indexes (HNSW, IVF) are explicitly out of scope.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use crate::storage::collection::{l2_norm, Collection};
use crate::storage::format::Record;
use crate::vector::cosine::dot_product;

/// One similarity search result.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: u64,
    /// Cosine similarity in `[-1.0, 1.0]`.
    pub score: f32,
    pub record: Record,
}

/// An element of the top-K candidate heap. `Ord` compares by score
/// (NaN-safe via `total_cmp`), then by id as a stable tie-break.
#[derive(Debug, PartialEq, Clone)]
struct Scored {
    score: f32,
    id: u64,
}

impl Eq for Scored {}

impl Ord for Scored {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.id.cmp(&self.id))
    }
}

impl PartialOrd for Scored {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Search a collection for the `k` records most similar to `query`.
///
/// Only records for which `predicate` returns `true` are candidates — this is
/// the hybrid filter-and-score path. The query vector's dimensionality must
/// already have been validated against the collection.
pub fn search_top_k<F>(
    collection: &Collection,
    query: &[f32],
    k: usize,
    predicate: F,
) -> Vec<SearchHit>
where
    F: Fn(&Record) -> bool,
{
    if k == 0 || collection.is_empty() {
        return Vec::new();
    }
    let query_norm = l2_norm(query);
    let mut heap: BinaryHeap<Reverse<Scored>> = BinaryHeap::with_capacity(k + 1);

    for (idx, record) in collection.iter().enumerate() {
        if !predicate(record) {
            continue;
        }
        let score = score_cosine(query, query_norm, &record.vector, collection.norm(idx));
        heap.push(Reverse(Scored {
            score,
            id: record.id,
        }));
        if heap.len() > k {
            heap.pop(); // evict the current worst candidate
        }
    }

    let mut hits: Vec<SearchHit> = heap
        .into_iter()
        .map(|Reverse(Scored { score, id })| {
            let record = collection.get(id).cloned().expect("heap id always live");
            SearchHit { id, score, record }
        })
        .collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    hits
}

/// Cosine similarity using the query's precomputed norm and the record's
/// precomputed norm, so each record costs one dot product.
fn score_cosine(query: &[f32], query_norm: f32, vector: &[f32], vector_norm: f32) -> f32 {
    let denom = query_norm * vector_norm;
    if denom == 0.0 {
        0.0
    } else {
        dot_product(query, vector) / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn collection_with(vectors: &[&[f32]]) -> Collection {
        let mut c = Collection::new("t", vectors[0].len());
        for (i, v) in vectors.iter().enumerate() {
            c.apply_put(Record {
                id: i as u64,
                vector: v.to_vec(),
                metadata: json!({"i": i, "even": i % 2 == 0}),
            })
            .unwrap();
        }
        c
    }

    #[test]
    fn returns_top_k_in_score_order() {
        let c = collection_with(&[&[1.0, 0.0], &[0.0, 1.0], &[0.9, 0.1], &[-1.0, 0.0]]);
        let hits = search_top_k(&c, &[1.0, 0.0], 2, |_| true);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, 0); // identical
        assert_eq!(hits[1].id, 2); // near-identical
        assert!(hits[0].score >= hits[1].score);
    }

    #[test]
    fn k_larger_than_collection_returns_all() {
        let c = collection_with(&[&[1.0, 0.0], &[0.0, 1.0]]);
        let hits = search_top_k(&c, &[1.0, 0.0], 100, |_| true);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn zero_k_returns_nothing() {
        let c = collection_with(&[&[1.0, 0.0]]);
        assert!(search_top_k(&c, &[1.0, 0.0], 0, |_| true).is_empty());
    }

    #[test]
    fn predicate_filters_candidates() {
        let c = collection_with(&[&[1.0, 0.0], &[0.5, 0.5]]);
        let hits = search_top_k(&c, &[1.0, 0.0], 10, |r| r.metadata["i"].as_u64() == Some(0));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, 0);
    }

    #[test]
    fn zero_vector_scores_zero() {
        let c = collection_with(&[&[0.0, 0.0], &[1.0, 0.0]]);
        let hits = search_top_k(&c, &[0.0, 0.0], 10, |_| true);
        for hit in &hits {
            assert_eq!(hit.score, 0.0);
        }
    }
}
