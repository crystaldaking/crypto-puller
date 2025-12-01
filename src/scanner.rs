use crate::models::{Chain, ScannerProgress, TransferEvent};
use chrono::{DateTime, Utc};
use serde_json;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::info;

/// Trait для сканирования блокчейна с поддержкой батчинга
pub trait ChainScanner {
    /// Получить последний блок
    async fn get_latest_block(&self) -> Result<u64, anyhow::Error>;

    /// Получить timestamp блока
    async fn get_block_timestamp(&self, block: u64) -> Result<DateTime<Utc>, anyhow::Error>;

    /// Сканировать один блок (legacy метод)
    async fn scan_block(
        &self,
        block: u64,
        wallets: &[String],
    ) -> Result<Vec<TransferEvent>, anyhow::Error>;

    /// Сканировать диапазон блоков (оптимизированный метод)
    /// По умолчанию вызывает scan_block для каждого блока
    async fn scan_block_range(
        &self,
        from_block: u64,
        to_block: u64,
        wallets: &[String],
    ) -> Result<Vec<TransferEvent>, anyhow::Error> {
        let mut all_events = Vec::new();
        for block in from_block..=to_block {
            let events = self.scan_block(block, wallets).await?;
            all_events.extend(events);
        }
        Ok(all_events)
    }

    /// Разрешить блок по timestamp (бинарный поиск)
    async fn resolve_block_by_timestamp(
        &self,
        timestamp: DateTime<Utc>,
    ) -> Result<u64, anyhow::Error>;

    /// Поддерживает ли сканер батчинг
    fn supports_batch(&self) -> bool {
        false
    }

    /// Рекомендуемый размер батча
    fn recommended_batch_size(&self) -> u64 {
        1
    }
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

    let batch_size = scanner.recommended_batch_size();
    let supports_batch = scanner.supports_batch();

    info!(
        "Starting scanner for {:?}, batch_size={}, supports_batch={}",
        chain, batch_size, supports_batch
    );

    loop {
        let latest_block = scanner.get_latest_block().await?;

        while current_block <= latest_block {
            let our_wallets = wallets
                .read()
                .await
                .get(&chain)
                .cloned()
                .unwrap_or_default();

            // Определяем диапазон для обработки
            let to_block = std::cmp::min(current_block + batch_size - 1, latest_block);

            let events = if supports_batch && batch_size > 1 {
                // Батчинг: обрабатываем диапазон блоков за раз
                info!(
                    "Batch scanning blocks {}..{} for chain {:?}",
                    current_block, to_block, chain
                );
                scanner
                    .scan_block_range(current_block, to_block, &our_wallets)
                    .await?
            } else {
                // Обычное сканирование по одному блоку (когда batch_size=1)
                scanner.scan_block(current_block, &our_wallets).await?
            };

            info!(
                "Processing blocks {}..{} for chain {:?}, found {} events",
                current_block,
                to_block,
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

            // Обновляем прогресс на последний обработанный блок
            progress.last_block = to_block;
            progress.last_timestamp = scanner.get_block_timestamp(to_block).await?;
            save_progress(pool, &progress).await?;

            current_block = to_block + 1;
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await; // Poll every 30 seconds
    }
}
