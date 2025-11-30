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
    pub ethereum_rpc_url: Option<String>,
    pub kafka_brokers: Option<String>,
    pub kafka_topic: Option<String>,
    pub log_level: String,
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
            ethereum_rpc_url: None,
            kafka_brokers: None,
            kafka_topic: None,
            log_level: "info".to_string(),
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
