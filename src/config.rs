use figment::{
    providers::{Env, Serialized},
    Figment,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct Config {
    pub database_url: String,
    pub http_port: u16,
    pub grpc_port: u16,
    pub enable_tron: bool,
    pub enable_ton: bool,
    pub enable_ethereum: bool,
    pub use_console_instead_kafka: bool,
    pub start_from_tron: Option<String>,
    pub start_from_ton: Option<String>,
    pub start_from_ethereum: Option<String>,
    pub ton_rpc_url: Option<String>,
    pub ton_api_key: Option<String>,
    pub tron_rpc_url: Option<String>,
    pub ethereum_rpc_url: Option<String>,
    pub kafka_brokers: Option<String>,
    pub kafka_topic: Option<String>,
    pub log_level: String,
    // Optimization parameters
    pub ethereum_batch_size: Option<u64>,
    pub tron_batch_size: Option<u64>,
    pub ton_batch_size: Option<u64>,
    pub max_concurrent_requests: Option<usize>,
    pub batch_timeout_secs: Option<u64>,
}

// Дефолты, если какого-то env нет
impl Default for Config {
    fn default() -> Self {
        Self {
            database_url: "postgresql://user:password@localhost/crypto_puller".to_string(),
            http_port: 3000,
            grpc_port: 50051,
            enable_tron: true,
            enable_ton: true,
            enable_ethereum: true,
            // по умолчанию — пишем в консоль
            use_console_instead_kafka: true,
            start_from_tron: None,
            start_from_ton: None,
            start_from_ethereum: None,
            ton_rpc_url: None,
            ton_api_key: None,
            tron_rpc_url: None,
            ethereum_rpc_url: None,
            kafka_brokers: None,
            kafka_topic: None,
            log_level: "info".to_string(),
            // Optimization defaults
            ethereum_batch_size: Some(100),
            tron_batch_size: Some(5), // QuickNode Discover plan limit: 5 blocks max
            ton_batch_size: Some(10),
            max_concurrent_requests: Some(5),
            batch_timeout_secs: Some(30),
        }
    }
}

pub(crate) fn load_config() -> Config {
    let _ = dotenv::dotenv();

    Figment::from(Serialized::defaults(Config::default()))
        .merge(Env::raw())
        .extract()
        .expect("Failed to load config")
}
