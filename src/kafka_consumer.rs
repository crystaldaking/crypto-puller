use rdkafka::config::ClientConfig;
use rdkafka::consumer::StreamConsumer;
use rdkafka::Message;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{error, info};

use crate::config::Config;
use crypto_puller::WalletsCache;
use crate::metrics::Metrics;
use sqlx::PgPool;
use rdkafka::consumer::Consumer;
use serde_json;

use crypto_puller::api::AddWalletRequest;
use crypto_puller::scanner::add_wallet;
use crypto_puller::models::Chain;

pub fn start_kafka_consumer(
    config: Config,
    pool: PgPool,
    wallets: WalletsCache,
    metrics: Arc<Metrics>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<(), Box<dyn std::error::Error>> {
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

        let pool_clone = pool.clone();
        let wallets_clone = Arc::clone(&wallets);
        let metrics_clone = Arc::clone(&metrics);
        let mut shutdown_rx = shutdown_tx.subscribe();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("Kafka consumer shutting down");
                        let _ = consumer.unsubscribe();
                        break;
                    }
                    msg = consumer.recv() => {
                        match msg {
                            Ok(msg) => {
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

                                        if let Ok(added) = add_wallet(&pool_clone, chain, req.address.clone()).await {
                                            if added {
                                                wallets_clone
                                                    .write()
                                                    .await
                                                    .entry(chain)
                                                    .or_insert_with(Vec::new)
                                                    .push(req.address);
                                                info!("Added via Kafka");
                                                metrics_clone.increment_events();
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Kafka recv error: {}", e);
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            }
                        }
                    }
                }
            }
        });
    }
    Ok(())
}
