use crate::models::Chain;
use crate::scanner::add_wallet;
use crate::WalletsCache;
use axum::{extract::State, http::StatusCode, Json, Router};
use serde::Deserialize;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

#[derive(serde::Deserialize, Debug, Clone)]
pub struct AddWalletRequest {
    pub chain: String,
    pub address: String,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub wallets: WalletsCache,
}

async fn add_wallet_handler(
    State(state): State<AppState>,
    Json(payload): Json<AddWalletRequest>,
) -> (StatusCode, &'static str) {
    if payload.chain.is_empty() || payload.address.is_empty() {
        return (StatusCode::BAD_REQUEST, "Invalid request");
    }
    let chain = match payload.chain.as_str() {
        "Tron" => Chain::Tron,
        "Ton" => Chain::Ton,
        "Ethereum" => Chain::Ethereum,
        _ => return (StatusCode::BAD_REQUEST, "Invalid chain"),
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
            (StatusCode::OK, "Added")
        }
        Ok(_) => (StatusCode::CONFLICT, "Already exists"),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Error"),
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/wallets", axum::routing::post(add_wallet_handler))
        .layer(RequestBodyLimitLayer::new(1024)) // limit body size
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
