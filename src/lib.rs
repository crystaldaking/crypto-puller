#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_macros)]
#![allow(non_camel_case_types)]
#![allow(unused_unsafe)]
#![allow(unreachable_pub)]
#![allow(async_fn_in_trait)]

pub mod api;
pub mod chains;
pub mod config;
pub mod models;
pub mod scanner;
pub mod scanner_service;
pub mod sink;
pub mod wallet {
    include!(concat!(env!("OUT_DIR"), "/wallet.rs"));
}

pub type WalletsCache =
    std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<models::Chain, Vec<String>>>>;
