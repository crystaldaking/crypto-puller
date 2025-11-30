use crate::models::Chain;
use crate::scanner::add_wallet;
use crate::WalletsCache;
use axum::{extract::State, http::StatusCode, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

#[derive(serde::Deserialize, Debug, Clone)]
pub struct AddWalletRequest {
    pub chain: String,
    pub address: String,
}

#[derive(Serialize)]
pub struct JsonResponse {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonResponse {
    pub fn ok(message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            success: true,
            message: message.into(),
            data,
        }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            data: None,
        }
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            data: None,
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            data: None,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub wallets: WalletsCache,
}

async fn add_wallet_handler(
    State(state): State<AppState>,
    Json(payload): Json<AddWalletRequest>,
) -> (StatusCode, Json<JsonResponse>) {
    if payload.chain.is_empty() || payload.address.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(JsonResponse::bad_request("Invalid request")),
        );
    }
    let chain = match payload.chain.as_str() {
        "Tron" => Chain::Tron,
        "Ton" => Chain::Ton,
        "Ethereum" => Chain::Ethereum,
        _ => return (
            StatusCode::BAD_REQUEST,
            Json(JsonResponse::bad_request("Invalid chain")),
        ),
    };
    match add_wallet(&state.pool, chain, payload.address.clone()).await {
        Ok(added) if added => {
            state
                .wallets
                .write()
                .await
                .entry(chain)
                .or_insert_with(Vec::new)
                .push(payload.address);
            (StatusCode::OK, Json(JsonResponse::ok("Added", None)))
        }
        Ok(_) => (StatusCode::CONFLICT, Json(JsonResponse::conflict("Already exists"))),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(JsonResponse::error("Error"))),
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/wallets", axum::routing::post(add_wallet_handler))
        .layer(RequestBodyLimitLayer::new(1024)) // limit body size
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
