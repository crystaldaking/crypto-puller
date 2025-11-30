use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

use crate::config::Config;
use crypto_puller::WalletsCache;
use sqlx::PgPool;

use crypto_puller::chains::{tron::TronScanner, ton::TonScanner, ethereum::EthereumScanner};
use crypto_puller::scanner::scan_chain;
use crypto_puller::models::{TransferEvent, Chain};

pub async fn start_scanners(
    config: &Config,
    pool: PgPool,
    wallets_cache: WalletsCache,
    tx: mpsc::Sender<TransferEvent>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Tron
    if config.enable_tron {
        let wallets = Arc::clone(&wallets_cache);
        let tx_clone = tx.clone();
        let pool_clone = pool.clone();
        let mut shutdown_rx = shutdown_tx.subscribe();

        let start_from = config.start_from_tron.clone();
        tokio::spawn(async move {
            let scanner = TronScanner::new();
            let chain_name = "Tron";
            tokio::select! {
                _ = scan_chain(&scanner, &pool_clone, Chain::Tron, wallets, start_from, tx_clone) => {},
                _ = shutdown_rx.recv() => { info!("Shutdown {} scanner", chain_name); }
            }
        });
    }

    // Ton
    if config.enable_ton {
        let wallets = Arc::clone(&wallets_cache);
        let tx_clone = tx.clone();
        let pool_clone = pool.clone();
        let mut shutdown_rx = shutdown_tx.subscribe();
        let start_from = config.start_from_ton.clone();
        let ton_rpc = config.ton_rpc_url.clone().unwrap_or_default();
        let ton_api_key = config.ton_api_key.clone();

        tokio::spawn(async move {
            let scanner = TonScanner::new(ton_rpc, ton_api_key);
            let chain_name = "Ton";
            tokio::select! {
                _ = scan_chain(&scanner, &pool_clone, Chain::Ton, wallets, start_from, tx_clone) => {},
                _ = shutdown_rx.recv() => { info!("Shutdown {} scanner", chain_name); }
            }
        });
    }

    // Ethereum
    if config.enable_ethereum {
        let wallets = Arc::clone(&wallets_cache);
        let tx_clone = tx.clone();
        let pool_clone = pool.clone();
        let mut shutdown_rx = shutdown_tx.subscribe();
        let start_from = config.start_from_ethereum.clone();
        let eth_rpc = config.ethereum_rpc_url.clone().unwrap_or_default();

        // создаём сканер (await если нужно) и запускаем
        let scanner = EthereumScanner::new(None, &eth_rpc).await?;
        tokio::spawn(async move {
            let chain_name = "Ethereum";
            tokio::select! {
                _ = scan_chain(&scanner, &pool_clone, Chain::Ethereum, wallets, start_from, tx_clone) => {},
                _ = shutdown_rx.recv() => { info!("Shutdown {} scanner", chain_name); }
            }
        });
    }

    Ok(())
}
