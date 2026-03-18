use axum::response::Json;
use serde_json::Value;

pub async fn handle() -> Json<Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "avagen",
    }))
}

pub async fn home() -> &'static str {
    "AvaGen — AI Avatar Generation API made with ❤️ by Jamg"
}
