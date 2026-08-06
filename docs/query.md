# NovaDB Query DSL

Hybrid queries combine exact top-K vector search with metadata filtering.
The filter is written in a small DSL or built as an AST.

## Filter semantics

A filter is a boolean expression over a record's **top-level metadata
fields** (a JSON object). Records whose metadata is missing a field never
match a filter on that field.

### Operators

| Operator | Meaning | Works on |
| --- | --- | --- |
| `=` | equal | strings, numbers, booleans, null |
| `!=` | not equal | same as `=` |
| `<` `>` `<=` `>=` | numeric comparison | numbers only |

Numbers compare numerically across types: `1 == 1.0`, `5 > 4.9`.

### AND

`AND` combines terms; all must match. `AND` is the only combinator in v0.1.

## DSL grammar

```text
expr  := term ( "AND" term )*
term  := ident op value
op    := "=" | "!=" | "<" | ">" | "<=" | ">="
value := number | "true" | "false" | "null" | string
ident := [a-zA-Z_][a-zA-Z0-9_]*
```

Strings are double-quoted with `\" \\ \n \t \r` escapes. The keyword `AND`
is case-insensitive.

## Examples

```text
category = "electronics"                                  # string equality
price < 1000                                              # numeric comparison
rating >= 4
in_stock = true
category = "electronics" AND price < 1000 AND rating >= 4 # boolean AND
tags != null
```

## Query execution: filter-then-score

The engine applies the predicate before any similarity math:

1. Scan every live record; discard those failing the filter.
2. Score survivors with cosine similarity (precomputed norms, one dot
   product each).
3. Keep the top-K in a bounded heap; return results sorted by score,
   ties broken by ascending id.

Filtering first means selective filters make hybrid queries *faster*, not
slower — the benchmark suite shows ~40% speedup at 100k records with a
selective filter.

## Programmatic API

Filters can also be built as an AST instead of parsed:

```rust
use novadb::metadata::Filter;

let f = Filter::and(vec![
    Filter::eq("category", "electronics"),
    Filter::lt("price", 1000),
    Filter::gte("rating", 4),
]);
```
