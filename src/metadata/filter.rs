//! Metadata filters: a small boolean AST evaluated against record metadata.
//!
//! Scope (v0.1): equality, numeric comparisons, and boolean AND. The AST is
//! open-ended — `Or`/`Not` are natural extensions.

use serde_json::Value;

use crate::error::{NovaError, Result};

/// Comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ne,
    Lt,
    Gt,
    Lte,
    Gte,
}

/// A filter expression over a record's metadata document.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    /// `field <op> value`
    Compare(String, Cmp, Value),
    /// All sub-filters must match.
    And(Vec<Filter>),
}

impl Filter {
    /// Evaluate this filter against a metadata document.
    pub fn matches(&self, metadata: &Value) -> bool {
        match self {
            Filter::Compare(field, cmp, expected) => {
                let Some(actual) = metadata.get(field) else {
                    return false; // missing field never matches
                };
                match cmp {
                    Cmp::Eq => values_equal(actual, expected),
                    Cmp::Ne => !values_equal(actual, expected),
                    Cmp::Lt => {
                        numeric_cmp(actual, expected).map(|o| o == std::cmp::Ordering::Less)
                            == Some(true)
                    }
                    Cmp::Gt => {
                        numeric_cmp(actual, expected).map(|o| o == std::cmp::Ordering::Greater)
                            == Some(true)
                    }
                    Cmp::Lte => {
                        numeric_cmp(actual, expected).map(|o| o != std::cmp::Ordering::Greater)
                            == Some(true)
                    }
                    Cmp::Gte => {
                        numeric_cmp(actual, expected).map(|o| o != std::cmp::Ordering::Less)
                            == Some(true)
                    }
                }
            }
            Filter::And(filters) => filters.iter().all(|f| f.matches(metadata)),
        }
    }
}

/// Numeric comparison: integers and floats compare numerically (1 == 1.0).
/// Returns `None` when either side is not a number.
fn numeric_cmp(actual: &Value, expected: &Value) -> Option<std::cmp::Ordering> {
    match (actual.as_f64(), expected.as_f64()) {
        (Some(a), Some(b)) => a.partial_cmp(&b),
        _ => None,
    }
}

/// Equality with numeric unification: `1 == 1.0` is true.
fn values_equal(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        _ => actual == expected,
    }
}

/// Convenience constructors.
impl Filter {
    pub fn eq(field: impl Into<String>, value: impl Into<Value>) -> Self {
        Filter::Compare(field.into(), Cmp::Eq, value.into())
    }
    pub fn ne(field: impl Into<String>, value: impl Into<Value>) -> Self {
        Filter::Compare(field.into(), Cmp::Ne, value.into())
    }
    pub fn lt(field: impl Into<String>, value: impl Into<Value>) -> Self {
        Filter::Compare(field.into(), Cmp::Lt, value.into())
    }
    pub fn gt(field: impl Into<String>, value: impl Into<Value>) -> Self {
        Filter::Compare(field.into(), Cmp::Gt, value.into())
    }
    pub fn lte(field: impl Into<String>, value: impl Into<Value>) -> Self {
        Filter::Compare(field.into(), Cmp::Lte, value.into())
    }
    pub fn gte(field: impl Into<String>, value: impl Into<Value>) -> Self {
        Filter::Compare(field.into(), Cmp::Gte, value.into())
    }
    pub fn and(filters: Vec<Filter>) -> Self {
        Filter::And(filters)
    }

    /// Parse a filter from the NovaDB query DSL. See [`crate::metadata::parse`].
    pub fn parse(dsl: &str) -> Result<Self> {
        let mut parser = crate::metadata::parse::Parser::new(dsl);
        let filter = parser.parse()?;
        if let Some(rest) = parser.remaining() {
            if !rest.is_empty() {
                return Err(NovaError::Parse(format!(
                    "unexpected trailing input: '{rest}'"
                )));
            }
        }
        Ok(filter)
    }
}

impl std::fmt::Display for Filter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Filter::Compare(field, cmp, value) => {
                let op = match cmp {
                    Cmp::Eq => "=",
                    Cmp::Ne => "!=",
                    Cmp::Lt => "<",
                    Cmp::Gt => ">",
                    Cmp::Lte => "<=",
                    Cmp::Gte => ">=",
                };
                write!(f, "{field} {op} {value}")
            }
            Filter::And(filters) => {
                let parts: Vec<String> = filters.iter().map(|f| format!("{f}")).collect();
                write!(f, "{}", parts.join(" AND "))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn equality_and_numeric_unification() {
        let m = json!({"category": "electronics", "price": 1000, "rating": 4.5, "in_stock": true});
        assert!(Filter::eq("category", "electronics").matches(&m));
        assert!(!Filter::eq("category", "books").matches(&m));
        assert!(Filter::eq("price", 1000.0).matches(&m)); // int vs float
        assert!(!Filter::ne("category", "electronics").matches(&m));
    }

    #[test]
    fn numeric_comparisons() {
        let m = json!({"price": 1000, "rating": 4.0});
        assert!(Filter::lt("price", 1001).matches(&m));
        assert!(!Filter::lt("price", 1000).matches(&m));
        assert!(Filter::gt("price", 999).matches(&m));
        assert!(Filter::lte("price", 1000).matches(&m));
        assert!(Filter::gte("price", 1000).matches(&m));
        assert!(!Filter::gt("rating", 4.0).matches(&m));
        assert!(Filter::gte("rating", 4.0).matches(&m));
    }

    #[test]
    fn non_numeric_comparisons_fail() {
        let m = json!({"category": "electronics"});
        assert!(!Filter::lt("category", "zzz").matches(&m));
    }

    #[test]
    fn missing_field_never_matches() {
        let m = json!({});
        assert!(!Filter::eq("missing", 1).matches(&m));
        assert!(!Filter::ne("missing", 1).matches(&m)); // != on missing is false
    }

    #[test]
    fn and_requires_all() {
        let m = json!({"category": "electronics", "price": 500, "rating": 4});
        let f = Filter::and(vec![
            Filter::eq("category", "electronics"),
            Filter::lt("price", 1000),
            Filter::gte("rating", 4),
        ]);
        assert!(f.matches(&m));
        let f2 = Filter::and(vec![
            Filter::eq("category", "electronics"),
            Filter::gt("price", 1000),
        ]);
        assert!(!f2.matches(&m));
    }

    #[test]
    fn dsl_roundtrip() {
        let f =
            Filter::parse(r#"category = "electronics" AND price < 1000 AND rating >= 4"#).unwrap();
        let m = json!({"category": "electronics", "price": 500, "rating": 4.0});
        assert!(f.matches(&m));
        assert_eq!(
            format!("{f}"),
            r#"category = "electronics" AND price < 1000 AND rating >= 4"#
        );
    }

    #[test]
    fn display_format() {
        assert_eq!(format!("{}", Filter::eq("a", 5)), "a = 5");
    }
}
