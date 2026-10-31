use axum::response::Json;
use serde_json::{Value, json};

/// Unauthenticated liveness probe. Deliberately reveals nothing beyond "up".
pub(super) async fn health_check() -> Json<Value> {
    Json(json!({ "ok": true }))
}
