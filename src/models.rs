use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
pub enum Chain {
    Tron,
    Ton,
    Ethereum,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Direction {
    In,
    Out,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferEvent {
    pub chain: Chain,
    pub block: u64,
    pub timestamp: DateTime<Utc>,
    pub tx_hash: String,
    pub from: String,
    pub to: String,
    pub amount: String, // USDT amount as string to handle decimals
    pub direction: Direction,
    pub wallet: String, // Our wallet address
}

#[derive(Debug, Clone)]
pub struct ScannerProgress {
    pub chain: Chain,
    pub last_block: u64,
    pub last_timestamp: DateTime<Utc>,
}
