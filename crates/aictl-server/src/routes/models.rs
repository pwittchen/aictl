use axum::Json;

use crate::openai::{ModelsList, list_models};

pub async fn list() -> Json<ModelsList> {
    Json(list_models().await)
}
