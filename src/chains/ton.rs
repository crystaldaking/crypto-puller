use crate::models::{Chain, Direction, TransferEvent};
use crate::scanner::ChainScanner;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};

const USDT_JETTON_MASTER: &str = "EQCxE6mUtQJKFnGfaROTKOt1lZbDiiX1kCixRv7Nw2Id_sDs";

pub struct TonScanner {
    client: Client,
    api_key: Option<String>,
    base_url: String,
}

impl TonScanner {
    pub fn new(base_url: String, api_key: Option<String>) -> Self {
        let client = reqwest::ClientBuilder::new()
            // Some providers (incl. QuickNode) may have issues negotiating h2 with rustls
            // Force HTTP/1.1 to avoid TLS/ALPN incompatibilities
            .http1_only()
            .build()
            .expect("failed to build reqwest client");
        Self {
            client,
            api_key,
            base_url,
        }
    }

    // Generic JSON-RPC 2.0 request to toncenter public endpoint
    async fn rpc_request<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, anyhow::Error> {
        #[derive(Deserialize)]
        struct RpcEnvelope<T> {
            #[allow(dead_code)]
            jsonrpc: Option<String>,
            #[allow(dead_code)]
            id: Option<u64>,
            result: T,
        }

        let mut req = self.client.post(&self.base_url).json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        }));

        if let Some(key) = &self.api_key {
            req = req.header("X-API-Key", key);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!("RPC request failed: {}", resp.status()));
        }

        // If server returns an error envelope, surface it
        let text = resp.text().await?;
        // Try to parse as success first
        if let Ok(parsed) = serde_json::from_str::<RpcEnvelope<T>>(&text) {
            return Ok(parsed.result);
        }
        // Try to parse error envelope to give a better message
        if let Ok(err_envelope) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(err) = err_envelope.get("error") {
                return Err(anyhow::anyhow!(
                    "RPC error: {}",
                    err.get("message").and_then(|m| m.as_str()).unwrap_or(&text)
                ));
            }
        }
        Err(anyhow::anyhow!("Unexpected RPC response: {}", text))
    }
}

#[derive(Deserialize)]
struct MasterchainInfoResponse {
    last: MasterchainInfo,
}

#[derive(Deserialize)]
struct MasterchainInfo {
    seqno: u64,
    shard: String,
    workchain: i32,
}

#[derive(Deserialize)]
struct BlockHeaderResponse {
    id: BlockId,
    status: u32,
    global_id: i32,
    version: u32,
    after_merge: bool,
    after_split: bool,
    before_split: bool,
    want_merge: bool,
    want_split: bool,
    validator_list_hash_short: u32,
    catchain_seqno: u32,
    min_ref_mc_seqno: u64,
    is_key_block: bool,
    prev_key_block_seqno: u64,
    start_lt: String,
    end_lt: String,
    gen_utime: u64,
    vert_seqno: u32,
}

#[derive(Deserialize)]
struct BlockId {
    workchain: i32,
    shard: String,
    seqno: u64,
    root_hash: String,
    file_hash: String,
}

#[derive(Deserialize)]
struct BlockTransactionsResponse {
    ids: Vec<String>,
    incomplete: bool,
}

#[derive(Deserialize)]
struct TransactionResponse {
    utime: u64,
    hash: String,
    lt: String,
    account: String,
    out_msgs: Vec<Message>,
    in_msg: Option<Message>,
    jetton_transfers: Vec<JettonTransfer>,
}

#[derive(Deserialize)]
struct Message {
    source: Option<String>,
    destination: Option<String>,
    value: Option<String>,
    msg_data: Option<MsgData>,
}

#[derive(Deserialize)]
struct MsgData {
    #[serde(rename = "@type")]
    type_: String,
    text: Option<String>,
    body: Option<String>,
}

#[derive(Deserialize)]
struct JettonTransfer {
    query_id: String,
    amount: String,
    source: Option<String>,
    destination: String,
    source_wallet: String,
    jetton_master: String,
    transaction_hash: String,
}

impl ChainScanner for TonScanner {
    async fn get_latest_block(&self) -> Result<u64, anyhow::Error> {
        // JSON-RPC: method getMasterchainInfo, no params
        let resp: MasterchainInfoResponse =
            self.rpc_request("getMasterchainInfo", json!({})).await?;
        Ok(resp.last.seqno)
    }

    async fn get_block_timestamp(&self, block: u64) -> Result<DateTime<Utc>, anyhow::Error> {
        // JSON-RPC: method getBlockHeader, params: { seqno }
        let resp: BlockHeaderResponse = self
            .rpc_request("getBlockHeader", json!({ "seqno": block }))
            .await?;
        Ok(DateTime::from_timestamp(resp.gen_utime as i64, 0).unwrap())
    }

    async fn scan_block(
        &self,
        block: u64,
        wallets: &[String],
    ) -> Result<Vec<TransferEvent>, anyhow::Error> {
        let mut events = Vec::new();
        // First get block header for specific workchain (0) to ensure consistency
        let header_resp: BlockHeaderResponse = self
            .rpc_request("getBlockHeader", json!({ "seqno": block, "workchain": 0 }))
            .await?;
        let _block_id = format!("({}, {})", header_resp.id.workchain, header_resp.id.shard); // But API uses seqno directly? Wait, for getBlockTransactions, it's by block_id which is (workchain,shard,seqno,root_hash,file_hash)

        // For simplicity, query transactions by time window via JSON-RPC getTransactions equivalent
        let block_timestamp = self.get_block_timestamp(block).await?;
        let next_block_ts = if block < self.get_latest_block().await? {
            self.get_block_timestamp(block + 1).await?
        } else {
            block_timestamp + chrono::Duration::seconds(60) // Approximate 1 min
        };
        let start_ts = block_timestamp.timestamp() as u64;
        let end_ts = next_block_ts.timestamp() as u64;

        // NOTE: Toncenter JSON-RPC supports similar parameters as REST for getTransactions
        // Limit to 100 for minimal pagination
        let txs: Vec<TransactionResponse> = self
            .rpc_request(
                "getTransactions",
                json!({
                    "workchain": 0,
                    "start_utime": start_ts,
                    "end_utime": end_ts,
                    "limit": 100
                }),
            )
            .await?;
        for tx in txs {
            for jt in &tx.jetton_transfers {
                if jt.jetton_master == USDT_JETTON_MASTER {
                    events.push(TransferEvent {
                        chain: Chain::Ton,
                        block,
                        timestamp: DateTime::from_timestamp(tx.utime as i64, 0).unwrap(),
                        tx_hash: tx.hash.clone(),
                        from: jt.source.clone().unwrap_or_default(),
                        to: jt.destination.clone(),
                        amount: (jt.amount.parse::<u128>()? / 1_000_000).to_string(), // Jetton USDT has 6 decimals
                        direction: Direction::In,                                     // Set later
                        wallet: String::new(),
                    });
                }
            }
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
