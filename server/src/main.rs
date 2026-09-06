use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use novadb::engine::query::Query;
use novadb::metadata::Filter;
use novadb::{Database, DbConfig, NovaError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower_http::trace::TraceLayer;

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Database>>,
    api_key: Arc<str>,
    database_name: Arc<str>,
    request_sequence: Arc<AtomicU64>,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: String,
}

struct ApiError(StatusCode, &'static str, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(ErrorBody {
                error: ErrorDetail {
                    code: self.1,
                    message: self.2,
                },
            }),
        )
            .into_response()
    }
}

impl From<NovaError> for ApiError {
    fn from(error: NovaError) -> Self {
        let message = error.to_string();
        let (status, code) = match &error {
            NovaError::CollectionNotFound(_) | NovaError::RecordNotFound(_) => {
                (StatusCode::NOT_FOUND, "not_found")
            }
            NovaError::CollectionExists(_) | NovaError::Conflict(_) => {
                (StatusCode::CONFLICT, "conflict")
            }
            NovaError::DimMismatch { .. } | NovaError::EmptyVector | NovaError::Parse(_) => {
                (StatusCode::BAD_REQUEST, "invalid_request")
            }
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };
        Self(status, code, message)
    }
}

#[derive(Deserialize)]
struct CreateCollection {
    name: String,
    dimension: usize,
}

#[derive(Deserialize)]
struct VectorInput {
    id: Option<u64>,
    vector: Vec<f32>,
    #[serde(default = "empty_object")]
    metadata: Value,
}

#[derive(Deserialize)]
struct BatchUpsert {
    vectors: Vec<VectorInput>,
}

#[derive(Deserialize)]
struct QueryInput {
    vector: Vec<f32>,
    #[serde(default = "default_top_k")]
    top_k: usize,
    filter: Option<String>,
}

fn empty_object() -> Value {
    json!({})
}
fn default_top_k() -> usize {
    10
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let data_dir = std::env::var("NOVADB_DATA_DIR").unwrap_or_else(|_| "/data".into());
    let api_key = std::env::var("NOVADB_API_KEY").unwrap_or_default();
    if api_key.is_empty() {
        eprintln!("NOVADB_API_KEY is required");
        std::process::exit(2);
    }
    let max_body = env_usize("NOVADB_MAX_BODY_BYTES", 4 * 1024 * 1024);
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let database_name = std::env::var("NOVADB_DATABASE").unwrap_or_else(|_| "default".into());
    let db = Database::open(DbConfig::new(data_dir)).unwrap_or_else(|error| {
        eprintln!("database startup failed: {error}");
        std::process::exit(1);
    });
    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        api_key: api_key.into(),
        database_name: database_name.into(),
        request_sequence: Arc::new(AtomicU64::new(1)),
    };
    let app = app(state).layer(DefaultBodyLimit::max(max_body));
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("server address must bind");
    tracing::info!(port = %port, "NovaDB server ready");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .expect("server failed");
}

fn app(state: AppState) -> Router {
    let public = Router::new().route("/health", get(health));
    let protected = Router::new()
        .route("/ready", get(ready))
        .route("/v1/info", get(info))
        .route("/v1/databases", get(list_databases).post(create_database))
        .route(
            "/v1/databases/:db/collections",
            get(list_collections).post(create_collection),
        )
        .route(
            "/v1/databases/:db/collections/:collection",
            delete(drop_collection),
        )
        .route(
            "/v1/databases/:db/collections/:collection/vectors",
            post(batch_upsert),
        )
        .route(
            "/v1/databases/:db/collections/:collection/vectors/:id",
            put(upsert).delete(delete_vector),
        )
        .route(
            "/v1/databases/:db/collections/:collection/query",
            post(query),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    public
        .merge(protected)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn authenticate(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let expected = format!("Bearer {}", state.api_key);
    if supplied != Some(expected.as_str()) {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid bearer API key is required".into(),
        ));
    }
    let request_id = format!(
        "nova-{}",
        state.request_sequence.fetch_add(1, Ordering::Relaxed)
    );
    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    Ok(response)
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}
async fn ready(State(state): State<AppState>) -> Json<Value> {
    let collections = state.db.lock().unwrap().list_collections().len();
    Json(json!({"status": "ready", "collections": collections}))
}
async fn info(State(state): State<AppState>) -> Json<Value> {
    Json(
        json!({"name": "NovaDB", "version": env!("CARGO_PKG_VERSION"), "database": state.database_name.as_ref(), "distance_metric": "cosine", "mode": "single-writer"}),
    )
}
async fn list_databases(State(state): State<AppState>) -> Json<Value> {
    Json(json!({"databases": [state.database_name.as_ref()]}))
}
async fn create_database() -> Result<(StatusCode, Json<Value>), ApiError> {
    Err(ApiError(
        StatusCode::CONFLICT,
        "single_database",
        "this server owns one configured database".into(),
    ))
}

fn check_db(state: &AppState, db: &str) -> Result<(), ApiError> {
    if db == state.database_name.as_ref() {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::NOT_FOUND,
            "database_not_found",
            format!("database '{db}' not found"),
        ))
    }
}

async fn list_collections(
    State(state): State<AppState>,
    Path(db): Path<String>,
) -> Result<Json<Value>, ApiError> {
    check_db(&state, &db)?;
    let guard = state.db.lock().unwrap();
    let collections: Vec<_> = guard
        .list_collections()
        .into_iter()
        .map(|name| {
            let stats = guard.stats(&name).unwrap();
            json!({"name": name, "dimension": stats.dim, "vectors": stats.records})
        })
        .collect();
    Ok(Json(json!({"collections": collections})))
}
async fn create_collection(
    State(state): State<AppState>,
    Path(db): Path<String>,
    Json(input): Json<CreateCollection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    check_db(&state, &db)?;
    state
        .db
        .lock()
        .unwrap()
        .create_collection(&input.name, input.dimension)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"name": input.name, "dimension": input.dimension})),
    ))
}
async fn drop_collection(
    State(state): State<AppState>,
    Path((db, collection)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    check_db(&state, &db)?;
    state.db.lock().unwrap().drop_collection(&collection)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn batch_upsert(
    State(state): State<AppState>,
    Path((db, collection)): Path<(String, String)>,
    Json(input): Json<BatchUpsert>,
) -> Result<Json<Value>, ApiError> {
    check_db(&state, &db)?;
    if input.vectors.is_empty() || input.vectors.len() > env_usize("NOVADB_MAX_BATCH_SIZE", 1000) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "invalid_batch",
            "batch must contain 1..NOVADB_MAX_BATCH_SIZE vectors".into(),
        ));
    }
    let mut guard = state.db.lock().unwrap();
    let expected = guard.stats(&collection)?.dim;
    if let Some(item) = input
        .vectors
        .iter()
        .find(|item| item.vector.len() != expected)
    {
        return Err(NovaError::DimMismatch {
            expected,
            got: item.vector.len(),
        }
        .into());
    }
    let mut ids = Vec::with_capacity(input.vectors.len());
    for item in input.vectors {
        let id = match item.id {
            Some(id) => {
                guard.insert(&collection, id, &item.vector, item.metadata)?;
                id
            }
            None => guard.add(&collection, &item.vector, item.metadata)?,
        };
        ids.push(id);
    }
    Ok(Json(json!({"ids": ids, "count": ids.len()})))
}
async fn upsert(
    State(state): State<AppState>,
    Path((db, collection, id)): Path<(String, String, u64)>,
    Json(input): Json<VectorInput>,
) -> Result<Json<Value>, ApiError> {
    check_db(&state, &db)?;
    state
        .db
        .lock()
        .unwrap()
        .insert(&collection, id, &input.vector, input.metadata)?;
    Ok(Json(json!({"id": id})))
}
async fn delete_vector(
    State(state): State<AppState>,
    Path((db, collection, id)): Path<(String, String, u64)>,
) -> Result<StatusCode, ApiError> {
    check_db(&state, &db)?;
    if state.db.lock().unwrap().delete(&collection, id)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("vector {id} not found"),
        ))
    }
}
async fn query(
    State(state): State<AppState>,
    Path((db, collection)): Path<(String, String)>,
    Json(input): Json<QueryInput>,
) -> Result<Json<Value>, ApiError> {
    check_db(&state, &db)?;
    let filter = input.filter.as_deref().map(Filter::parse).transpose()?;
    let guard = state.db.lock().unwrap();
    let mut query = Query::new(&collection, &input.vector).top_k(input.top_k);
    if let Some(filter) = filter {
        query = query.filter(filter);
    }
    let hits: Vec<_> = guard.query(&query)?.into_iter().map(|hit| json!({"id": hit.id, "score": hit.score, "vector": hit.record.vector, "metadata": hit.record.metadata})).collect();
    Ok(Json(json!({"hits": hits, "count": hits.len()})))
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("graceful shutdown requested");
}
