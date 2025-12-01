use crate::models::{Chain, Direction, TransferEvent};
use crate::scanner::ChainScanner;
use chrono::{DateTime, Utc};
use ethers::types::U256;
use ethers::utils::keccak256;
use hex;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

const USDT_CONTRACT: &str = "0xdAC17F958D2ee523a2206206994597C13D831ec7";
const MAX_BLOCKS_PER_BATCH: u64 = 100; // Рекомендуемый размер батча для большинства RPC

pub struct EthereumScanner {
    client: Client,
    base_url: String,
    batch_size: u64,
}

impl EthereumScanner {
    pub async fn new(_ws_url: Option<String>, http_url: &str) -> Result<Self, anyhow::Error> {
        // Build a reqwest client with HTTP/1.1 to avoid potential ALPN issues with some RPCs
        let client = reqwest::ClientBuilder::new().http1_only().build()?;
        Ok(Self {
            client,
            base_url: http_url.to_string(),
            batch_size: MAX_BLOCKS_PER_BATCH,
        })
    }

    pub fn with_batch_size(mut self, size: u64) -> Self {
        self.batch_size = size;
        self
    }

    // Generic JSON-RPC request helper
    async fn rpc_request<R: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<R, anyhow::Error> {
        #[derive(Deserialize)]
        struct RpcEnvelope<T> {
            #[allow(dead_code)]
            jsonrpc: Option<String>,
            #[allow(dead_code)]
            id: Option<u64>,
            result: Option<T>,
            error: Option<serde_json::Value>,
        }

        let resp = self
            .client
            .post(&self.base_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            }))
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!("RPC request failed: {} - {}", status, text));
        }
        let env: RpcEnvelope<R> = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Deserialization Error: {}. Response: {}", e, text))?;
        if let Some(err) = env.error {
            return Err(anyhow::anyhow!("RPC error: {}", err));
        }
        env.result
            .ok_or_else(|| anyhow::anyhow!("Empty result in RPC response: {}", text))
    }
}

impl ChainScanner for EthereumScanner {
    async fn get_latest_block(&self) -> Result<u64, anyhow::Error> {
        // eth_blockNumber returns hex string
        let hex_str: String = self.rpc_request("eth_blockNumber", json!([])).await?;
        let value = u64::from_str_radix(hex_str.trim_start_matches("0x"), 16)?;
        Ok(value)
    }

    async fn get_block_timestamp(&self, block: u64) -> Result<DateTime<Utc>, anyhow::Error> {
        #[derive(Deserialize)]
        struct BlockLite {
            timestamp: String,
        }
        #[derive(Deserialize)]
        struct BlockResp {
            timestamp: String,
        }

        let block_hex = format!("0x{:x}", block);
        let block_obj: BlockResp = self
            .rpc_request("eth_getBlockByNumber", json!([block_hex, false]))
            .await?;
        let ts = u64::from_str_radix(block_obj.timestamp.trim_start_matches("0x"), 16)? as i64;
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
        #[derive(Deserialize)]
        struct LogEntry {
            address: String,
            topics: Vec<String>,
            data: String,
            #[serde(rename = "transactionHash")]
            transaction_hash: Option<String>,
            #[serde(rename = "blockNumber")]
            block_number: String,
        }

        let mut events = Vec::new();
        if wallets.is_empty() {
            return Ok(events);
        }

        // ERC-20 Transfer signature keccak
        let transfer_sig = keccak256(b"Transfer(address,address,uint256)");
        let topic0 = format!("0x{}", hex::encode(transfer_sig));
        let from_block_hex = format!("0x{:x}", from_block);
        let to_block_hex = format!("0x{:x}", to_block);

        let mut padded_wallets = Vec::new();
        for wallet in wallets {
            let clean = wallet.strip_prefix("0x").unwrap_or(wallet).to_lowercase();
            let padded = format!("0x{:0>64}", clean);
            padded_wallets.push(padded);
        }

        // Кешируем timestamps для блоков в диапазоне
        let mut block_timestamps = std::collections::HashMap::new();

        // From logs - одним запросом для всего диапазона
        let from_logs: Vec<LogEntry> = self
            .rpc_request(
                "eth_getLogs",
                json!([{
                    "fromBlock": from_block_hex,
                    "toBlock": to_block_hex,
                    "address": USDT_CONTRACT,
                    "topics": [topic0.clone(), padded_wallets.clone(), serde_json::Value::Null]
                }]),
            )
            .await?;

        for log in from_logs {
            if log.topics.len() < 3 {
                continue;
            }

            let block_num = u64::from_str_radix(log.block_number.trim_start_matches("0x"), 16)
                .unwrap_or(from_block);

            // Получаем timestamp блока (кешируем)
            let timestamp = if let Some(&ts) = block_timestamps.get(&block_num) {
                ts
            } else {
                let ts = self.get_block_timestamp(block_num).await?;
                block_timestamps.insert(block_num, ts);
                ts
            };

            let from_hex = &log.topics[1];
            let to_hex = &log.topics[2];
            let from_addr =
                format!("0x{}", &from_hex.trim_start_matches("0x")[24..]).to_lowercase();
            let to_addr = format!("0x{}", &to_hex.trim_start_matches("0x")[24..]).to_lowercase();

            let data_bytes = hex::decode(log.data.trim_start_matches("0x")).unwrap_or_default();
            let amount_whole = if data_bytes.is_empty() {
                0u128
            } else {
                let v = U256::from_big_endian(&data_bytes);
                let q = v / U256::from(1_000_000u64); // 1e6
                q.as_u128()
            };

            events.push(TransferEvent {
                chain: Chain::Ethereum,
                block: block_num,
                timestamp,
                tx_hash: log.transaction_hash.unwrap_or_default(),
                from: from_addr.clone(),
                to: to_addr,
                amount: amount_whole.to_string(),
                direction: Direction::Out,
                wallet: from_addr,
            });
        }

        // To logs - одним запросом для всего диапазона
        let to_logs: Vec<LogEntry> = self
            .rpc_request(
                "eth_getLogs",
                json!([{
                    "fromBlock": from_block_hex,
                    "toBlock": to_block_hex,
                    "address": USDT_CONTRACT,
                    "topics": [topic0, serde_json::Value::Null, padded_wallets]
                }]),
            )
            .await?;

        for log in to_logs {
            if log.topics.len() < 3 {
                continue;
            }

            let block_num = u64::from_str_radix(log.block_number.trim_start_matches("0x"), 16)
                .unwrap_or(from_block);

            // Получаем timestamp блока (кешируем)
            let timestamp = if let Some(&ts) = block_timestamps.get(&block_num) {
                ts
            } else {
                let ts = self.get_block_timestamp(block_num).await?;
                block_timestamps.insert(block_num, ts);
                ts
            };

            let from_hex = &log.topics[1];
            let to_hex = &log.topics[2];
            let from_addr =
                format!("0x{}", &from_hex.trim_start_matches("0x")[24..]).to_lowercase();
            let to_addr = format!("0x{}", &to_hex.trim_start_matches("0x")[24..]).to_lowercase();

            let data_bytes = hex::decode(log.data.trim_start_matches("0x")).unwrap_or_default();
            let amount_whole = if data_bytes.is_empty() {
                0u128
            } else {
                let v = U256::from_big_endian(&data_bytes);
                let q = v / U256::from(1_000_000u64); // 1e6
                q.as_u128()
            };

            events.push(TransferEvent {
                chain: Chain::Ethereum,
                block: block_num,
                timestamp,
                tx_hash: log.transaction_hash.unwrap_or_default(),
                from: from_addr,
                to: to_addr.clone(),
                amount: amount_whole.to_string(),
                direction: Direction::In,
                wallet: to_addr,
            });
        }

        Ok(events)
    }

    fn supports_batch(&self) -> bool {
        true
    }

    fn recommended_batch_size(&self) -> u64 {
        self.batch_size
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
