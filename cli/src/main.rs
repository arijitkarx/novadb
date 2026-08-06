//! The NovaDB command-line interface.
//!
//! Commands mirror the library API: create/drop collections, insert records,
//! run hybrid queries, and checkpoint the WAL into snapshots.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use novadb::engine::query::Query;
use novadb::metadata::Filter;
use novadb::{Database, DbConfig, NovaError};
use serde_json::{json, Value};

#[derive(Parser)]
#[command(
    name = "novadb",
    version,
    about = "A lightweight embedded vector database",
    long_about = "NovaDB v0.1: an embedded vector database with WAL, crash recovery,\nexact flat vector search, metadata filtering, and hybrid queries.\n\nData lives in <dir>/*.nova snapshots with a write-ahead log in <dir>/wal/.\nQueries and results are JSON, so the CLI composes well with jq."
)]
struct Cli {
    /// Directory holding the database (snapshots + wal/).
    #[arg(short, long, default_value = "./novadb-data")]
    db: PathBuf,

    /// Never fsync the WAL (fast but unsafe).
    #[arg(long, conflicts_with = "batch")]
    no_sync: bool,

    /// fsync the WAL every N writes.
    #[arg(long, conflicts_with = "no_sync")]
    batch: Option<u64>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a collection with the given vector dimensionality.
    Create {
        name: String,
        #[arg(long)]
        dim: usize,
    },
    /// Drop a collection and delete its files.
    Drop { name: String },
    /// List collections.
    List,
    /// Show collection stats.
    Info { name: String },
    /// Insert a record (auto id unless --id is given).
    Insert {
        name: String,
        /// Embedding, e.g. --vector "[0.1, 0.2, 0.3]"
        #[arg(long)]
        vector: String,
        /// Metadata JSON object.
        #[arg(long, default_value = "{}")]
        metadata: String,
        /// Explicit record id (upsert semantics).
        #[arg(long)]
        id: Option<u64>,
    },
    /// Fetch a record by id.
    Get { name: String, id: u64 },
    /// Delete a record by id.
    Delete { name: String, id: u64 },
    /// List all records of a collection.
    Records { name: String },
    /// Hybrid similarity query.
    ///
    /// --filter uses the NovaDB DSL, e.g.
    /// --filter 'category = "electronics" AND price < 1000'
    Query {
        name: String,
        #[arg(long)]
        vector: String,
        #[arg(short, long, default_value_t = 10)]
        top_k: usize,
        #[arg(long)]
        filter: Option<String>,
    },
    /// Write snapshots now (checkpoint).
    Checkpoint { name: Option<String> },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut config = DbConfig::new(&cli.db);
    if cli.no_sync {
        config = config.with_sync_mode(novadb::SyncMode::Never);
    } else if let Some(n) = cli.batch {
        config = config.with_sync_mode(novadb::SyncMode::Batch(n));
    }

    let result = run(cli.command, config);
    match result {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command, config: DbConfig) -> Result<String, NovaError> {
    let mut db = Database::open(config)?;
    let output = match command {
        Command::Create { name, dim } => {
            db.create_collection(&name, dim)?;
            json!({"ok": true, "collection": name, "dim": dim})
        }
        Command::Drop { name } => {
            db.drop_collection(&name)?;
            json!({"ok": true, "dropped": name})
        }
        Command::List => {
            let names = db.list_collections();
            let mut items = Vec::new();
            for name in &names {
                let s = db.stats(name)?;
                items.push(json!({
                    "name": s.name,
                    "dim": s.dim,
                    "records": s.records,
                    "wal_frames": s.wal_frames,
                    "ops_since_snapshot": s.ops_since_snapshot,
                }));
            }
            json!({"collections": items})
        }
        Command::Info { name } => {
            let s = db.stats(&name)?;
            json!({
                "name": s.name,
                "dim": s.dim,
                "records": s.records,
                "next_id": s.next_id,
                "wal_frames": s.wal_frames,
                "ops_since_snapshot": s.ops_since_snapshot,
            })
        }
        Command::Insert {
            name,
            vector,
            metadata,
            id,
        } => {
            let vector = parse_vector(&vector)?;
            let metadata = parse_metadata(&metadata)?;
            let id = match id {
                Some(id) => {
                    db.insert(&name, id, &vector, metadata)?;
                    id
                }
                None => db.add(&name, &vector, metadata)?,
            };
            json!({"ok": true, "collection": name, "id": id})
        }
        Command::Get { name, id } => match db.get(&name, id)? {
            Some(record) => json!({"ok": true, "record": record_to_json(&record)}),
            None => json!({"ok": false, "reason": "not found"}),
        },
        Command::Delete { name, id } => {
            let deleted = db.delete(&name, id)?;
            json!({"ok": true, "deleted": deleted})
        }
        Command::Records { name } => {
            let records: Vec<Value> = db.all_records(&name)?.iter().map(record_to_json).collect();
            json!({"records": records})
        }
        Command::Query {
            name,
            vector,
            top_k,
            filter,
        } => {
            let vector = parse_vector(&vector)?;
            let filter = match filter {
                Some(dsl) => Some(Filter::parse(&dsl)?),
                None => None,
            };
            let mut query = Query::new(&name, &vector).top_k(top_k);
            if let Some(f) = filter {
                query = query.filter(f);
            }
            let hits = db.query(&query)?;
            let results: Vec<Value> = hits
                .iter()
                .map(|h| {
                    json!({
                        "id": h.id,
                        "score": h.score,
                        "record": record_to_json(&h.record),
                    })
                })
                .collect();
            json!({"collection": name, "hits": results, "count": results.len()})
        }
        Command::Checkpoint { name } => match name {
            Some(name) => {
                db.snapshot(&name)?;
                json!({"ok": true, "snapshotted": name})
            }
            None => {
                db.snapshot_all()?;
                json!({"ok": true, "snapshotted": "all"})
            }
        },
    };
    db.close()?;
    Ok(serde_json::to_string_pretty(&output).unwrap())
}

/// Parse `"[0.1, 0.2, 0.3]"` (or bare `0.1,0.2,0.3`) into a vector.
fn parse_vector(text: &str) -> Result<Vec<f32>, NovaError> {
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    let mut out = Vec::new();
    for part in inner.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        out.push(
            part.parse::<f32>()
                .map_err(|_| NovaError::Parse(format!("invalid vector component '{part}'")))?,
        );
    }
    if out.is_empty() {
        return Err(NovaError::EmptyVector);
    }
    Ok(out)
}

fn parse_metadata(text: &str) -> Result<Value, NovaError> {
    if text.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_str(text).map_err(|e| NovaError::Parse(format!("invalid metadata JSON: {e}")))
}

fn record_to_json(record: &novadb::Record) -> Value {
    json!({
        "id": record.id,
        "vector": record.vector,
        "metadata": record.metadata,
    })
}
