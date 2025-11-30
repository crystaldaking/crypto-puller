use crypto_puller::api::{build_router, AppState};
use crate::config::load_config;
use crate::metrics::Metrics;
use crypto_puller::models::Chain;
use crypto_puller::scanner::{add_wallet, load_wallets};
use crypto_puller::models::TransferEvent;
use crypto_puller::wallet::wallet_service_server::{WalletService, WalletServiceServer};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::{broadcast, mpsc, RwLock};
use tonic::transport::Server as TonicServer;
use tracing::info;
use crypto_puller::WalletsCache;

mod config;
mod metrics;
mod scanner_manager;
mod kafka_consumer;
mod sink_builder;

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

    let (tx, rx) = mpsc::channel::<TransferEvent>(100);
    let shutdown_tx = broadcast::channel(1).0;

    // Start scanners (moved to scanner_manager)
    scanner_manager::start_scanners(
        &config,
        pool.clone(),
        Arc::clone(&wallets_cache),
        tx.clone(),
        shutdown_tx.clone(),
    ).await?;

    let app_state = AppState {
        pool: pool.clone(),
        wallets: Arc::clone(&wallets_cache),
    };
    let addr = format!("0.0.0.0:{}", config.http_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let app = build_router(app_state);
    let server = axum::serve(listener, app);

    // prepare HTTP shutdown future (subscribes to broadcast)
    let mut shutdown_rx_http = shutdown_tx.subscribe();
    let http_shutdown = async move {
        let _ = shutdown_rx_http.recv().await;
    };

    // Build sink via builder (moved earlier so event handler can use it)
    let mut sink = sink_builder::build_sink(&config)?;
    // clone metrics for the event task so original `metrics` stays available
    let metrics_for_event = Arc::clone(&metrics);
    // move rx and sink into the event task
    let mut rx_for_event = rx;
    // spawn event handler as background task (already created)
    let event_handle = tokio::spawn(async move {
        while let Some(event) = rx_for_event.recv().await {
            sink.send(event).await.ok();
            metrics_for_event.increment_events();
        }
    });

    // spawn HTTP server with graceful shutdown in a task
    let http_handle = tokio::spawn(async move {
        let _ = server.with_graceful_shutdown(http_shutdown).await;
    });

    // gRPC server with graceful shutdown (spawn immediately, not after ctrl-c)
    let grpc_addr = format!("0.0.0.0:{}", config.grpc_port).parse().unwrap();
    let svc = WalletServiceServer::new(WalletServiceImpl {
        pool: pool.clone(),
        wallets: Arc::clone(&wallets_cache),
    });
    let grpc_server = TonicServer::builder().add_service(svc);
    let mut shutdown_rx_grpc = shutdown_tx.subscribe();
    let grpc_future = grpc_server.serve_with_shutdown(grpc_addr, async move {
        let _ = shutdown_rx_grpc.recv().await;
    });
    let grpc_handle = tokio::spawn(grpc_future);

    // Start Kafka consumer (moved to kafka_consumer)
    kafka_consumer::start_kafka_consumer(
        config.clone(),
        pool.clone(),
        Arc::clone(&wallets_cache),
        Arc::clone(&metrics),
        shutdown_tx.clone(),
    )?;

    // wait for ctrl-c, then send shutdown and wait for HTTP to finish
    let _ = signal::ctrl_c().await;
    info!("Received ctrl-c, initiating graceful shutdown...");
    let _ = shutdown_tx.send(());
    let _ = http_handle.await;

    // wait for other tasks with timeout (5 seconds)
    info!("Waiting for background tasks to finish...");
    let shutdown_timeout = tokio::time::Duration::from_secs(5);
    
    let _ = tokio::time::timeout(shutdown_timeout, event_handle).await;
    let _ = tokio::time::timeout(shutdown_timeout, grpc_handle).await;
    
    info!("Shutdown complete");

    Ok(())
}
