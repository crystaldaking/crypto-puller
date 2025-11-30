use crate::config::Config;
use crate::models::Chain;
use crate::scanner::add_wallet;
use crate::WalletsCache;
use axum::{extract::State, http::StatusCode, Json, Router};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use validator::Validate;

#[derive(Deserialize, Validate)]
struct AddWalletRequest {
    #[validate(length(min = 1))]
    chain: String,
    #[validate(length(min = 1))]
    address: String,
}

#[derive(Clone)]
struct AppState {
    pool: sqlx::PgPool,
    wallets: WalletsCache,
}

async fn add_wallet_handler(
    State(state): State<AppState>,
    Json(mut payload): Json<AddWalletRequest>,
) -> (StatusCode, &'static str) {
    if payload.validate().is_err() {
        return (StatusCode::BAD_REQUEST, "Invalid request");
    }
    let chain = match payload.chain.as_str() {
        "Tron" => Chain::Tron,
        "Ton" => Chain::Ton,
        "Ethereum" => Chain::Ethereum,
        _ => return (StatusCode::BAD_REQUEST, "Invalid chain"),
    };
    match add_wallet(&state.pool, chain, payload.address).await {
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
