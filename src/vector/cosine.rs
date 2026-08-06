//! Cosine similarity over `f32` vectors.

/// Cosine similarity between two vectors.
///
/// Returns a value in `[-1.0, 1.0]`. The zero vector has no direction, so
/// any pair involving it scores `0.0`. Callers must ensure both vectors have
/// the same length.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (&x, &y) in a.iter().zip(b) {
        dot += x as f64 * y as f64;
        na += (x as f64) * (x as f64);
        nb += (y as f64) * (y as f64);
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        (dot / denom) as f32
    }
}

/// Dot product of two vectors.
pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(&x, &y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors_score_one() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors_score_zero() {
        let a = [1.0, 0.0];
        let b = [0.0, 1.0];
        assert!((cosine_similarity(&a, &b)).abs() < 1e-7);
    }

    #[test]
    fn opposite_vectors_score_negative_one() {
        let a = [1.0, 2.0];
        let b = [-1.0, -2.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn scale_invariant() {
        let a = [1.0, 2.0, 3.0];
        let b = [0.5, 1.0, 1.5]; // b = 0.5 * a
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zero_vectors_score_zero() {
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[0.0, 0.0]), 0.0);
    }
}
