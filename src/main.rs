use crate::api::build_router;
use crate::config::load_config;
use crate::metrics::Metrics;
use crypto_puller::chains::ethereum::EthereumScanner;
use crypto_puller::chains::ton::TonScanner;
use crypto_puller::chains::tron::TronScanner;
use crypto_puller::models::Chain;
use crypto_puller::scanner::{add_wallet, load_wallets, scan_chain};
use crypto_puller::sink::SinkType;
use crypto_puller::wallet::wallet_service_server::{WalletService, WalletServiceServer};
use opentelemetry::global;
use opentelemetry_otlp::WithExportConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::Message;
use sqlx::postgres::PgPoolOptions;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::{broadcast, mpsc, RwLock};
use tonic::transport::Server;
use tracing::{error, info, Level};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::prelude::*;

mod api;
mod config;
mod metrics;

type WalletsCache = Arc<RwLock<HashMap<Chain, Vec<String>>>>;

#[derive(Clone)]
struct WalletServiceImpl {
    pool: sqlx::PgPool,
    wallets: WalletsCache,
}

#[tonic::async_trait]
impl WalletService for WalletServiceImpl {
    async fn add_wallet(
        &self,
        request: tonic::Request<crypto_puller::wallet::WalletRequest>,
    ) -> Result<tonic::Response<crypto_puller::wallet::Empty>, tonic::Status> {
        let req = request.into_inner();
        let chain = match req.chain.as_str() {
            "Tron" => Chain::Tron,
            "Ton" => Chain::Ton,
            "Ethereum" => Chain::Ethereum,
            _ => return Err(tonic::Status::invalid_argument("Invalid chain")),
        };
        let address = req.address.clone();
        match add_wallet(&self.pool, chain, address).await {
            Ok(added) if added => {
                self.wallets
                    .write()
                    .await
                    .entry(chain)
                    .or_insert_with(Vec::new)
                    .push(req.address);
                Ok(tonic::Response::new(crypto_puller::wallet::Empty {}))
            }
            Ok(_) => Err(tonic::Status::already_exists("Already exists")),
            Err(e) => Err(tonic::Status::internal(e.to_string())),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config();

    // Logging with OTEL
    global::set_text_map_propagator(opentelemetry::sdk::propagation::TraceContextPropagator::new());
    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(opentelemetry_otlp::new_exporter().tonic())
        .install_batch(opentelemetry::runtime::Tokio)?;
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().json().with_level(true))
        .with(OpenTelemetryLayer::new(tracer))
        .with(tracing_subscriber::filter::LevelFilter::from_level(
            match config.log_level.as_str() {
                "debug" => Level::DEBUG,
                "error" => Level::ERROR,
                _ => Level::INFO,
            },
        ))
        .init();

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!().run(&pool).await?;

    let wallets_cache: WalletsCache = Arc::new(RwLock::new(load_wallets(&pool).await?));

    let metrics = Arc::new(Metrics::new());
    metrics::start_metrics_server(Arc::clone(&metrics));

    let (tx, mut rx) = mpsc::channel(100);
    let shutdown_tx = broadcast::channel(1).0;

    // Spawn scanners
    macro_rules! spawn_scanner {
        ($chain:ident, $scanner:expr) => {
            if config.enable_ $chain {
                let scanner = $scanner;
                let wallets = Arc::clone(&wallets_cache);
                let tx = tx.clone();
                let pool = pool.clone();
                let shutdown_rx = shutdown_tx.subscribe();
                tokio::spawn(async move {
                    select! {
                        _ = scan_chain(&scanner, &pool, Chain:: $chain, wallets, config.start_from_ $chain.clone(), tx) => {},
                        _ = shutdown_rx.recv() => { info!("Shutdown $chain scanner"); }
                    }
                });
            }
        };
    }

    spawn_scanner!(tron, TronScanner::new());
    spawn_scanner!(
        ton,
        TonScanner::new(config.ton_rpc_url.unwrap_or_default(), config.ton_api_key)
    );
    spawn_scanner!(
        ethereum,
        EthereumScanner::new(None, &config.ethereum_rpc_url.unwrap_or_default()).await?
    );

    let app_state = AppState {
        pool: pool.clone(),
        wallets: Arc::clone(&wallets_cache),
    };
    let http_server = axum::Server::bind(
        &"0.0.0.0:".to_string() + &config.http_port.to_string().parse().unwrap(),
    )
    .serve(build_router(app_state).into_make_service());
    tokio::spawn(http_server);

    let grpc_server = Server::builder()
        .add_service(WalletServiceServer::new(WalletServiceImpl {
            pool: pool.clone(),
            wallets: Arc::clone(&wallets_cache),
        }))
        .serve("0.0.0.0:".to_string() + &config.grpc_port.to_string().parse().unwrap());
    tokio::spawn(grpc_server);

    // Kafka consumer
    if let (Some(brokers), Some(topic)) = (config.kafka_brokers, config.kafka_topic) {
        let consumer = StreamConsumer::create(brokers.as_str(), "wallet_adder")?;
        consumer.subscribe(&["add_wallet"])?;
        let pool = pool.clone();
        let wallets = Arc::clone(&wallets_cache);
        tokio::spawn(async move {
            loop {
                if let Ok(msg) = consumer.recv().await {
                    if let Some(payload) = msg.payload() {
                        if let Ok(req) = serde_json::from_slice::<AddWalletRequest>(payload) {
                            if let Ok(added) =
                                add_wallet(&pool, req.chain.parse()?, req.address).await
                            {
                                if added {
                                    wallets
                                        .write()
                                        .await
                                        .entry(req.chain.parse()?)
                                        .or_insert_with(Vec::new)
                                        .push(req.address);
                                    info!("Added via Kafka");
                                    metrics.increment_events();
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    let mut sink = if config.use_console {
        SinkType::console()
    } else {
        SinkType::kafka(&config.kafka_brokers.unwrap(), &config.kafka_topic.unwrap())?
    };

    let event_handler = async {
        while let Some(event) = rx.recv().await {
            sink.send(event).await.ok();
            metrics.increment_events();
        }
    };

    tokio::select! {
        _ = event_handler => {},
        _ = signal::ctrl_c() => { shutdown_tx.send(())?; }
    }

    Ok(())
}
