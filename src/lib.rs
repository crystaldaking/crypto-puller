pub mod api;
pub mod chains;
pub mod config;
pub mod models;
pub mod scanner;
pub mod sink;
pub mod wallet {
    include!(concat!(env!("OUT_DIR"), "/wallet.rs"));
}

pub type WalletsCache =
    std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<models::Chain, Vec<String>>>>;
