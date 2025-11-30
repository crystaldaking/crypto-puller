```
# Crypto-Puller: Multi-Chain USDT Poller

A Rust application that monitors confirmed USDT transfers across TRON, TON, and Ethereum blockchains, saves progress to PostgreSQL, and publishes events to Kafka.

## Features

- **Multi-Chain Support**: Polls TRON (TRC-20 USDT), TON (Jetton USDT), and Ethereum (ERC-20 USDT)
- **Confirmed Transactions**: Only scans confirmed blocks/transactions
- **Wallet Filtering**: Supports multiple wallets per chain, determines IN/OUT direction
- **Progress Persistence**: Saves last scanned block and timestamp to PostgreSQL
- **Fallback by Date**: Start scanning from a specific timestamp using resolve_block_by_timestamp
- **Graceful Shutdown**: Handles Ctrl+C for clean exit
- **Event Sinks**: Console output (JSON) and Kafka producer
- **Docker Compose**: Ready-to-run environment with PostgreSQL, Kafka, and Zookeeper

## Prerequisites

- Rust 1.70+
- Docker and Docker Compose

## Setup

1. Clone the repository:
   ```bash
   git clone <your-repo-url>
   cd crypto-puller
   ```

2. Copy the example environment file:
   ```bash
   cp .env.example .env
   ```

3. Edit `.env` with your API keys and wallet addresses:
   - Obtain API keys for TRON Grid (Trongrid), TON (QuickNode or Toncenter), and Ethereum RPC (Infura/Alchemy)
   - List your wallet addresses for each chain (comma-separated)
   - Set ENABLE_CHAIN flags to choose which blockchains to monitor
   - Set USE_CONSOLE_INSTEAD_KAFKA to choose output method

4. Start the infrastructure with Docker Compose:
   ```bash
   docker compose up -d
   ```
   This starts PostgreSQL, Kafka, and Zookeeper.

5. Run database migrations (optional, auto-run on startup):
   ```bash
   cargo sqlx migrate run
   ```

6. Build and run the application:
   ```bash
   cargo build --release
   cargo run --release
   ```

## Configuration

Environment variables in `.env`:

- `DATABASE_URL`: PostgreSQL connection string
- `USE_CONSOLE_INSTEAD_KAFKA`: If true, outputs events to console only; if false, sends to Kafka (default false)
- `KAFKA_BROKERS`: Kafka brokers (e.g., localhost:9092) - required if USE_CONSOLE_INSTEAD_KAFKA=false
- `KAFKA_TOPIC`: Kafka topic for events - required if USE_CONSOLE_INSTEAD_KAFKA=false
- `ENABLE_TRON`: Enable TRON scanning (default true)
- `ENABLE_TON`: Enable TON scanning (default true)
- `ENABLE_ETHEREUM`: Enable Ethereum scanning (default true)
- `TRON_RPC_KEY`: Trongrid API key (optional)
- `TON_API_KEY`: Provider API key for TON (optional). For Toncenter this is sent via `X-API-Key` header. QuickNode usually embeds the token in the URL and doesn't require this header.
- `TON_RPC_URL`: Full TON JSON-RPC endpoint URL. You can use either:
  - QuickNode: e.g. `https://<your-subdomain>.ton-mainnet.quiknode.pro/<token>`
  - Toncenter: `https://toncenter.com/api/v2/jsonRPC`
  The client forces HTTP/1.1 to avoid TLS/ALPN issues with some providers, and the build uses native-tls for broader TLS compatibility.
- `ETHEREUM_RPC_URL`: Ethereum RPC URL (HTTPS). You can use providers like GetBlock (recommended in the issue): e.g. `https://go.getblock.io/<your-project-id>` or Infura/Alchemy.
- `WALLETS_TRON`: Comma-separated TRON wallet addresses
- `WALLETS_TON`: Comma-separated TON wallet addresses
- `WALLETS_ETHEREUM`: Comma-separated Ethereum wallet addresses
- `START_FROM_TRON`: ISO 8601 timestamp to start scanning TRON from (optional). If not set, starts from the latest block.
- `START_FROM_TON`: ISO 8601 timestamp for TON (optional). If not set, starts from the latest block.
- `START_FROM_ETHEREUM`: ISO 8601 timestamp for Ethereum (optional). If not set, starts from the latest block.

## Usage

- Run `cargo run --release` to start polling.
- Events will be sent to the selected sink (console or Kafka based on USE_CONSOLE_INSTEAD_KAFKA).
- Use Ctrl+C to shut down gracefully, progress is saved automatically.

## Architecture

- `src/main.rs`: Application entry point, sets up scanners and sinks
- `src/models.rs`: Data models (Chain, TransferEvent, etc.)
- `src/scanner.rs`: Universal scanning logic and progress management
- `src/sink.rs`: Event sinks (Console, Kafka)
- `src/chains/`: Chain-specific scanners (TRON, TON, Ethereum)

## Dependencies

Key crates:
- `tokio`: Async runtime
- `sqlx`: PostgreSQL integration
- `rdkafka`: Kafka producer
- `reqwest`: HTTP client for APIs
- `ethers`: Ethereum RPC client
- `serde`: Serialization
- `chrono`: Date/time handling
- `tracing`: Logging

## Troubleshooting

- Ensure all API keys and URLs are correctly set in `.env`
- For Ethereum over GetBlock, set `ETHEREUM_RPC_URL` to your GetBlock HTTPS endpoint. The app uses ethers-rs Provider over HTTP JSON-RPC.
- For TON over QuickNode, set `TON_RPC_URL` to your QuickNode endpoint. If you previously saw TLS/SSL errors, the app now enforces HTTP/1.1 on the TON client which typically resolves this.
- If TLS issues persist with certain providers, note that the project now links reqwest with native-tls (OpenSSL/SChannel) for maximum compatibility.
- Check Docker containers are running: `docker compose ps`
- Logs are via `tracing`, set `RUST_LOG=info` for more details
- For production, adjust database connection pool and polling intervals

## License

[Your License Here]
```
