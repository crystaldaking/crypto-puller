use crate::models::{Chain, ScannerProgress, TransferEvent};
use chrono::{DateTime, Utc};
use serde_json;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::info;

pub trait ChainScanner {
    async fn get_latest_block(&self) -> Result<u64, anyhow::Error>;
    async fn get_block_timestamp(&self, block: u64) -> Result<DateTime<Utc>, anyhow::Error>;
    async fn scan_block(
        &self,
        block: u64,
        wallets: &[String],
    ) -> Result<Vec<TransferEvent>, anyhow::Error>;
    async fn resolve_block_by_timestamp(
        &self,
        timestamp: DateTime<Utc>,
    ) -> Result<u64, anyhow::Error>;
}

pub async fn load_wallets(pool: &PgPool) -> Result<HashMap<Chain, Vec<String>>, anyhow::Error> {
    let rows = sqlx::query("SELECT chain, address FROM wallets")
        .fetch_all(pool)
        .await?;

    let mut map: HashMap<Chain, Vec<String>> = HashMap::new();
    for row in rows {
        let chain_str: String = row.try_get("chain")?;
        let address: String = row.try_get("address")?;
        let chain: Chain = serde_json::from_str(&chain_str)?;
        map.entry(chain).or_insert_with(Vec::new).push(address);
    }
    Ok(map)
}

pub async fn add_wallet(
    pool: &PgPool,
    chain: Chain,
    address: String,
) -> Result<bool, anyhow::Error> {
    let chain_str = serde_json::to_string(&chain)?;
    let existing = sqlx::query("SELECT 1 FROM wallets WHERE chain = $1 AND address = $2")
        .bind(&chain_str)
        .bind(&address)
        .fetch_optional(pool)
        .await?;
    if existing.is_some() {
        Ok(false)
    } else {
        sqlx::query("INSERT INTO wallets (chain, address) VALUES ($1, $2)")
            .bind(&chain_str)
            .bind(&address)
            .execute(pool)
            .await?;
        Ok(true)
    }
}

pub async fn load_progress(
    pool: &PgPool,
    chain: &Chain,
) -> Result<Option<ScannerProgress>, anyhow::Error> {
    let chain_str = serde_json::to_string(chain)?;
    let row =
        sqlx::query("SELECT last_block, last_timestamp FROM scanner_progress WHERE chain = $1")
            .bind(&chain_str)
            .fetch_optional(pool)
            .await?;

    if let Some(row) = row {
        let last_block: i64 = row.try_get(0)?;
        let last_timestamp: DateTime<Utc> = row.try_get(1)?;
        Ok(Some(ScannerProgress {
            chain: chain.clone(),
            last_block: last_block as u64,
            last_timestamp,
        }))
    } else {
        Ok(None)
    }
}

pub async fn save_progress(pool: &PgPool, progress: &ScannerProgress) -> Result<(), anyhow::Error> {
    let chain_str = serde_json::to_string(&progress.chain)?;
    sqlx::query(
        "INSERT INTO scanner_progress (chain, last_block, last_timestamp) VALUES ($1, $2, $3)
         ON CONFLICT (chain) DO UPDATE SET last_block = EXCLUDED.last_block, last_timestamp = EXCLUDED.last_timestamp"
    )
    .bind(&chain_str)
    .bind(progress.last_block as i64)
    .bind(progress.last_timestamp)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn scan_chain<T: ChainScanner>(
    scanner: &T,
    pool: &PgPool,
    chain: Chain,
    wallets: Arc<RwLock<HashMap<Chain, Vec<String>>>>,
    start_from: Option<String>,
    tx: mpsc::Sender<TransferEvent>,
) -> Result<(), anyhow::Error> {
    let start_block = if let Some(start_from) = start_from {
        let timestamp: DateTime<Utc> = start_from
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid timestamp format"))?;
        scanner.resolve_block_by_timestamp(timestamp).await?
    } else {
        if let Some(progress) = load_progress(pool, &chain).await? {
            progress.last_block
        } else {
            scanner.get_latest_block().await?
        }
    };

    let mut current_block = start_block;
    let mut progress = ScannerProgress {
        chain: chain.clone(),
        last_block: start_block,
        last_timestamp: scanner.get_block_timestamp(start_block).await?,
    };
    save_progress(pool, &progress).await?;

    loop {
        let latest_block = scanner.get_latest_block().await?;
        while current_block <= latest_block {
            let our_wallets = wallets
                .read()
                .await
                .get(&chain)
                .cloned()
                .unwrap_or_default();
            let events = scanner.scan_block(current_block, &our_wallets).await?;
            info!(
                "Processing block {} for chain {:?}, found {} events",
                current_block,
                chain,
                events.len()
            );
            for event in events {
                info!(
                    "Event for our wallet: wallet={}, direction={:?}, amount={}, timestamp={:?}",
                    event.wallet, event.direction, event.amount, event.timestamp
                );
                info!("{}", serde_json::to_string_pretty(&event).unwrap());
                tx.send(event).await?;
            }
            progress.last_block = current_block;
            progress.last_timestamp = scanner.get_block_timestamp(current_block).await?;
            save_progress(pool, &progress).await?;
            current_block += 1;
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await; // Poll every 30 seconds
    }
}
