use crate::models::{Chain, Direction, TransferEvent};
use crate::scanner::ChainScanner;
use chrono::{DateTime, Utc};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Address, Filter, H256, U256, U64};
use ethers::utils::keccak256;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, info, warn};

const USDT_CONTRACT: &str = "0xa614f803B6FD780986A42c78Ec9c7f77e6DeD13C";
const MAX_BLOCKS_PER_BATCH: u64 = 5; // QuickNode Discover plan limit: 5 blocks max

pub struct TronScanner {
    provider: Arc<Provider<Http>>,
    batch_size: u64,
}

impl TronScanner {
    pub fn new() -> Self {
        let rpc_url = env::var("TRON_RPC_URL").expect("TRON_RPC_URL must be set");
        let provider =
            Arc::new(Provider::<Http>::try_from(rpc_url).expect("Failed to create Tron provider"));
        Self {
            provider,
            batch_size: MAX_BLOCKS_PER_BATCH,
        }
    }

    pub fn with_batch_size(mut self, size: u64) -> Self {
        self.batch_size = size;
        self
    }
}

/// Конвертация 20‑байтового EVM‑адреса в Tron base58 (T...).
fn tron_address20_to_base58(addr20: &[u8]) -> String {
    assert_eq!(addr20.len(), 20);

    // payload = 0x41 + 20 байт адреса
    let mut payload = Vec::with_capacity(21);
    payload.push(0x41);
    payload.extend_from_slice(addr20);

    let hash1 = Sha256::digest(&payload);
    let hash2 = Sha256::digest(&hash1);
    let checksum = &hash2[0..4];

    let mut full = Vec::with_capacity(25);
    full.extend_from_slice(&payload);
    full.extend_from_slice(checksum);

    bs58::encode(full).into_string()
}

impl ChainScanner for TronScanner {
    async fn get_latest_block(&self) -> Result<u64, anyhow::Error> {
        let block_number: U64 = tokio::time::timeout(
            tokio::time::Duration::from_secs(10),
            self.provider.get_block_number(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("TRON get_block_number timeout"))?
        .map_err(|e| anyhow::anyhow!("TRON get_block_number error: {}", e))?;
        Ok(block_number.as_u64())
    }

    async fn get_block_timestamp(&self, block: u64) -> Result<DateTime<Utc>, anyhow::Error> {
        #[derive(Debug, Deserialize, Serialize)]
        struct TronBlockLight {
            timestamp: U256,
        }

        let block_hex = format!("0x{:x}", block);
        let raw_block: TronBlockLight = tokio::time::timeout(
            tokio::time::Duration::from_secs(10),
            self.provider
                .request("eth_getBlockByNumber", (block_hex, false)),
        )
        .await
        .map_err(|_| anyhow::anyhow!("TRON block request timeout"))?
        .map_err(|e| anyhow::anyhow!("TRON block request error: {}", e))?;

        let ts = raw_block.timestamp.as_u64() as i64;
        Ok(DateTime::from_timestamp(ts, 0).unwrap())
    }

    async fn scan_block(
        &self,
        block: u64,
        wallets: &[String],
    ) -> Result<Vec<TransferEvent>, anyhow::Error> {
        // Используем оптимизированный метод для одного блока
        self.scan_block_range(block, block, wallets).await
    }

    async fn scan_block_range(
        &self,
        from_block: u64,
        to_block: u64,
        wallets: &[String],
    ) -> Result<Vec<TransferEvent>, anyhow::Error> {
        let mut events = Vec::new();

        let usdt_addr: Address = Address::from_str(USDT_CONTRACT)?;
        let transfer_sig = H256::from_slice(&keccak256(b"Transfer(address,address,uint256)"));

        // Логи Transfer USDT за диапазон блоков
        let filter = Filter::new()
            .address(usdt_addr)
            .from_block(from_block)
            .to_block(to_block)
            .topic0(transfer_sig);

        debug!(
            "TRON: Requesting logs for blocks {}..{}",
            from_block, to_block
        );

        // Добавляем timeout для защиты от зависания
        let logs = tokio::time::timeout(
            tokio::time::Duration::from_secs(60), // Увеличено до 60 секунд
            self.provider.get_logs(&filter),
        )
        .await
        .map_err(|_| {
            warn!(
                "TRON RPC timeout after 60 seconds for blocks {}..{}",
                from_block, to_block
            );
            anyhow::anyhow!("TRON RPC timeout after 60 seconds")
        })?
        .map_err(|e| {
            warn!(
                "TRON RPC error for blocks {}..{}: {}",
                from_block, to_block, e
            );
            anyhow::anyhow!("TRON RPC error: {}", e)
        })?;

        info!(
            "TRON: Received {} logs for blocks {}..{}",
            logs.len(),
            from_block,
            to_block
        );

        // Кешируем timestamps для блоков в диапазоне
        let mut block_timestamps = std::collections::HashMap::new();

        debug!("TRON: Processing {} logs", logs.len());

        for log in logs {
            if log.topics.len() < 3 {
                continue;
            }

            let block_num = log.block_number.map(|n| n.as_u64()).unwrap_or(from_block);

            // Получаем timestamp блока (кешируем)
            let timestamp = if let Some(&ts) = block_timestamps.get(&block_num) {
                ts
            } else {
                let ts = self.get_block_timestamp(block_num).await?;
                block_timestamps.insert(block_num, ts);
                ts
            };

            let from_bytes = &log.topics[1].0[12..32];
            let to_bytes = &log.topics[2].0[12..32];

            // Convert to base58
            let from = tron_address20_to_base58(from_bytes);
            let to = tron_address20_to_base58(to_bytes);

            // data = uint256 amount (big-endian)
            let amount = U256::from_big_endian(&log.data);

            let tx_hash_h256 = log.transaction_hash.unwrap_or_default();
            let tx_hash = format!("0x{}", hex::encode(tx_hash_h256.0));

            events.push(TransferEvent {
                chain: Chain::Tron,
                block: block_num,
                timestamp,
                tx_hash,
                from,
                to,
                // USDT на Tron имеет 6 знаков после запятой
                amount: (amount / U256::from(1_000_000u64)).to_string(),
                // direction / wallet выставятся позже
                direction: Direction::In,
                wallet: String::new(),
            });
        }

        debug!(
            "TRON: Found {} total events, filtering by wallets",
            events.len()
        );

        let mut filtered_events = Vec::new();
        for event in events {
            if wallets.contains(&event.from) {
                filtered_events.push(TransferEvent {
                    direction: Direction::Out,
                    wallet: event.from.clone(),
                    ..event
                });
            } else if wallets.contains(&event.to) {
                filtered_events.push(TransferEvent {
                    direction: Direction::In,
                    wallet: event.to.clone(),
                    ..event
                });
            }
        }

        info!(
            "TRON: Filtered to {} events for our wallets",
            filtered_events.len()
        );
        Ok(filtered_events)
    }

    async fn resolve_block_by_timestamp(
        &self,
        timestamp: DateTime<Utc>,
    ) -> Result<u64, anyhow::Error> {
        let target = timestamp.timestamp() as u64;
        let mut low = 1u64;
        let mut high = self.get_latest_block().await?;

        while low < high {
            let mid = low + (high - low) / 2;
            let mid_ts = self.get_block_timestamp(mid).await?.timestamp() as u64;

            if mid_ts < target {
                low = mid + 1;
            } else {
                high = mid;
            }
        }

        Ok(low)
    }

    fn supports_batch(&self) -> bool {
        true
    }

    fn recommended_batch_size(&self) -> u64 {
        self.batch_size
    }
}
