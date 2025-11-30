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

const USDT_CONTRACT: &str = "0xa614f803B6FD780986A42c78Ec9c7f77e6DeD13C";

pub struct TronScanner {
    provider: Arc<Provider<Http>>,
}

impl TronScanner {
    pub fn new() -> Self {
        let rpc_url = env::var("TRON_RPC_URL").expect("TRON_RPC_URL must be set");
        let provider =
            Arc::new(Provider::<Http>::try_from(rpc_url).expect("Failed to create Tron provider"));
        Self { provider }
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
        let block_number: U64 = self.provider.get_block_number().await?;
        Ok(block_number.as_u64())
    }

    async fn get_block_timestamp(&self, block: u64) -> Result<DateTime<Utc>, anyhow::Error> {
        #[derive(Debug, Deserialize, Serialize)]
        struct TronBlockLight {
            timestamp: U256,
        }

        let block_hex = format!("0x{:x}", block);
        let raw_block: TronBlockLight = self
            .provider
            .request("eth_getBlockByNumber", (block_hex, false))
            .await?;

        let ts = raw_block.timestamp.as_u64() as i64;
        Ok(DateTime::from_timestamp(ts, 0).unwrap())
    }

    async fn scan_block(
        &self,
        block: u64,
        wallets: &[String],
    ) -> Result<Vec<TransferEvent>, anyhow::Error> {
        let mut events = Vec::new();

        let usdt_addr: Address = Address::from_str(USDT_CONTRACT)?;
        let transfer_sig = H256::from_slice(&keccak256(b"Transfer(address,address,uint256)"));

        // Логи Transfer USDT за конкретный блок
        let filter = Filter::new()
            .address(usdt_addr)
            .from_block(block)
            .to_block(block)
            .topic0(transfer_sig);

        let logs = self.provider.get_logs(&filter).await?;
        let timestamp = self.get_block_timestamp(block).await?;

        for log in logs {
            if log.topics.len() < 3 {
                continue;
            }
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
                block,
                timestamp,
                tx_hash,
                from,
                to,
                // USDT на Tron имеет 6 знаков после запятой
                amount: (amount / U256::from(1_000_000u64)).to_string(),
                // direction / wallet выставятся в scan_chain
                direction: Direction::In,
                wallet: String::new(),
            });
        }

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
}
