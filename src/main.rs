use crypto_puller::api::{build_router, AppState, AddWalletRequest};
use crate::config::load_config;
use crate::metrics::Metrics;
use crypto_puller::chains::ethereum::EthereumScanner;
use crypto_puller::chains::ton::TonScanner;
use crypto_puller::chains::tron::TronScanner;
use crypto_puller::models::Chain;
use crypto_puller::scanner::{add_wallet, load_wallets, scan_chain};
use crypto_puller::sink::SinkType;
use crypto_puller::wallet::wallet_service_server::{WalletService, WalletServiceServer};
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::Message;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tokio::select;
use tokio::signal;
use tokio::sync::{broadcast, mpsc, RwLock};
use tonic::transport::Server;
use tracing::{error, info, Level};
use crypto_puller::WalletsCache;
use rdkafka::config::ClientConfig;

mod config;
mod metrics;

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

    // Logging
    tracing_subscriber::fmt().json().with_level(true).init();

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
        ($chain:ident, $enable:expr, $start_from:expr, $scanner:expr) => {
            if $enable {
                let scanner = $scanner;
                let wallets = Arc::clone(&wallets_cache);
                let tx = tx.clone();
                let pool = pool.clone();
                let mut shutdown_rx = shutdown_tx.subscribe(); // ← тут mut

                tokio::spawn(async move {
                    select! {
                        _ = scan_chain(&scanner, &pool, Chain::$chain, wallets, $start_from, tx) => {},
                        _ = shutdown_rx.recv() => { info!("Shutdown $chain scanner"); }
                    }
                });
            }
    };
}

    spawn_scanner!(
        Tron,
        config.enable_tron,
        config.start_from_tron.clone(),
        TronScanner::new()
    );
    spawn_scanner!(
        Ton,
        config.enable_ton,
        config.start_from_ton.clone(),
        TonScanner::new(config.ton_rpc_url.unwrap_or_default(), config.ton_api_key)
    );
    spawn_scanner!(
        Ethereum,
        config.enable_ethereum,
        config.start_from_ethereum.clone(),
        EthereumScanner::new(None, &config.ethereum_rpc_url.unwrap_or_default()).await?
    );

    let app_state = AppState {
        pool: pool.clone(),
        wallets: Arc::clone(&wallets_cache),
    };
    let addr = format!("0.0.0.0:{}", config.http_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let http_server = axum::serve(listener, build_router(app_state));
    tokio::spawn(async move { http_server.await.unwrap() });

    let grpc_server = Server::builder()
        .add_service(WalletServiceServer::new(WalletServiceImpl {
            pool: pool.clone(),
            wallets: Arc::clone(&wallets_cache),
        }))
        .serve(format!("0.0.0.0:{}", config.grpc_port).parse().unwrap());
    tokio::spawn(grpc_server);

    // Kafka consumer
    if let (Some(brokers), Some(topic)) = (config.kafka_brokers.clone(), config.kafka_topic.clone()) {
        let mut consumer_config = ClientConfig::new();
        consumer_config
            .set("group.id", "wallet_adder")
            .set("bootstrap.servers", brokers.as_str())
            .set("enable.partition.eof", "false")
            .set("session.timeout.ms", "6000")
            .set("enable.auto.commit", "true");

        let consumer: StreamConsumer = consumer_config.create()?;
        consumer.subscribe(&[&topic])?;

        let pool = pool.clone();
        let wallets = Arc::clone(&wallets_cache);
        let metrics_kafka = Arc::clone(&metrics); // см. пункт 4

        tokio::spawn(async move {
            loop {
                if let Ok(msg) = consumer.recv().await {
                    if let Some(payload) = msg.payload() {
                        if let Ok(req) = serde_json::from_slice::<AddWalletRequest>(payload) {
                            let chain = match req.chain.as_str() {
                                "Tron" => Chain::Tron,
                                "Ton" => Chain::Ton,
                                "Ethereum" => Chain::Ethereum,
                                _ => {
                                    info!("Kafka: invalid chain '{}'", req.chain);
                                    continue;
                                }
                            };

                            if let Ok(added) = add_wallet(&pool, chain, req.address.clone()).await {
                                if added {
                                    wallets
                                        .write()
                                        .await
                                        .entry(chain)
                                        .or_insert_with(Vec::new)
                                        .push(req.address);
                                    info!("Added via Kafka");
                                    metrics_kafka.increment_events();
                                }
                            }
                        }
                    }
                }
            }
        });
    }


    let mut sink = if config.use_console_instead_kafka {
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
