use clap::Parser;
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::Serialize;
use validator::Validate;

#[derive(Parser, Serialize, Validate)]
#[clap(author, version, about)]
struct Config {
    #[clap(env, long)]
    #[validate(length(min = 1))]
    database_url: String,
    #[clap(env, long, default_value = "3000")]
    http_port: u16,
    #[clap(env, long, default_value = "50051")]
    grpc_port: u16,
    #[clap(env, long, default_value = "false")]
    enable_tron: bool,
    #[clap(env, long, default_value = "false")]
    enable_ton: bool,
    #[clap(env, long, default_value = "true")]
    enable_ethereum: bool,
    #[clap(env, long, default_value = "true")]
    use_console: bool,
    #[clap(env, long)]
    ton_rpc_url: Option<String>,
    #[clap(env, long)]
    ton_api_key: Option<String>,
    #[clap(env, long)]
    ethereum_rpc_url: Option<String>,
    #[clap(env, long)]
    kafka_brokers: Option<String>,
    #[clap(env, long)]
    kafka_topic: Option<String>,
    #[clap(env, long, default_value = "info")]
    log_level: String,
}

pub(crate) fn load_config() -> Config {
    let figment = Figment::new()
        .merge(Toml::file("config.toml"))
        .merge(Env::prefixed("CP_"));
    let config: Config = figment.extract().expect("Failed to load config");
    config.validate().expect("Invalid config");
    tracing::debug!("Config loaded: {:#?}", config);
    config
}
