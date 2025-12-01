use crate::models::TransferEvent;
use crate::scanner::ChainScanner;
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{info, warn};

/// Конфигурация параллельного сканера
#[derive(Debug, Clone)]
pub struct ParallelScanConfig {
    /// Максимальное количество одновременных запросов
    pub max_concurrent_requests: usize,
    /// Размер батча блоков
    pub batch_size: u64,
    /// Таймаут для одного батча (секунды)
    pub batch_timeout_secs: u64,
}

impl Default for ParallelScanConfig {
    fn default() -> Self {
        Self {
            max_concurrent_requests: 5,
            batch_size: 50,
            batch_timeout_secs: 30,
        }
    }
}

/// Параллельный сканер блоков
pub struct ParallelScanner<T: ChainScanner> {
    scanner: Arc<T>,
    config: ParallelScanConfig,
    semaphore: Arc<Semaphore>,
}

impl<T: ChainScanner + Send + Sync + 'static> ParallelScanner<T> {
    pub fn new(scanner: T, config: ParallelScanConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_requests));
        Self {
            scanner: Arc::new(scanner),
            config,
            semaphore,
        }
    }

    /// Сканировать диапазон блоков параллельно
    ///
    /// # Arguments
    ///
    /// * `from_block` - Начальный блок (включительно)
    /// * `to_block` - Конечный блок (включительно)
    /// * `wallets` - Список кошельков для фильтрации
    ///
    /// # Returns
    ///
    /// Вектор событий, отсортированных по номеру блока
    pub async fn scan_range_parallel(
        &self,
        from_block: u64,
        to_block: u64,
        wallets: &[String],
    ) -> Result<Vec<TransferEvent>, anyhow::Error> {
        if from_block > to_block {
            return Ok(Vec::new());
        }

        let wallets = wallets.to_vec();
        let total_blocks = to_block - from_block + 1;

        info!(
            "Starting parallel scan: blocks {}..{} ({} blocks), batch_size={}, concurrency={}",
            from_block,
            to_block,
            total_blocks,
            self.config.batch_size,
            self.config.max_concurrent_requests
        );

        // Разбиваем диапазон на батчи
        let batches: Vec<(u64, u64)> = self.split_into_batches(from_block, to_block);
        let batch_count = batches.len();

        info!("Split into {} batches", batch_count);

        // Обрабатываем батчи параллельно с ограничением concurrency
        let results = stream::iter(batches)
            .map(|(batch_start, batch_end)| {
                let scanner = Arc::clone(&self.scanner);
                let wallets = wallets.clone();
                let semaphore = Arc::clone(&self.semaphore);
                let timeout = self.config.batch_timeout_secs;

                async move {
                    // Ждем разрешения от семафора (ограничение concurrency)
                    let _permit = semaphore.acquire().await.unwrap();

                    info!("Processing batch {}..{}", batch_start, batch_end);

                    // Сканируем с таймаутом
                    let result = tokio::time::timeout(
                        tokio::time::Duration::from_secs(timeout),
                        scanner.scan_block_range(batch_start, batch_end, &wallets),
                    )
                    .await;

                    match result {
                        Ok(Ok(events)) => {
                            info!(
                                "Batch {}..{} completed: {} events",
                                batch_start,
                                batch_end,
                                events.len()
                            );
                            Ok(events)
                        }
                        Ok(Err(e)) => {
                            warn!("Batch {}..{} failed: {}", batch_start, batch_end, e);
                            Err(e)
                        }
                        Err(_) => {
                            warn!("Batch {}..{} timed out", batch_start, batch_end);
                            Err(anyhow::anyhow!("Batch timeout"))
                        }
                    }
                }
            })
            .buffer_unordered(self.config.max_concurrent_requests)
            .collect::<Vec<Result<Vec<TransferEvent>, anyhow::Error>>>()
            .await;

        // Собираем результаты
        let mut all_events = Vec::new();
        let mut errors = Vec::new();

        for (idx, result) in results.into_iter().enumerate() {
            match result {
                Ok(events) => all_events.extend(events),
                Err(e) => {
                    errors.push((idx, e));
                }
            }
        }

        if !errors.is_empty() {
            warn!(
                "Completed with {} errors out of {} batches",
                errors.len(),
                batch_count
            );
            // Можно вернуть частичный результат или ошибку в зависимости от требований
            // Здесь возвращаем частичный результат
        }

        // Сортируем события по блоку для консистентности
        all_events.sort_by_key(|e| e.block);

        info!(
            "Parallel scan completed: {} total events, {} errors",
            all_events.len(),
            errors.len()
        );

        Ok(all_events)
    }

    /// Разбить диапазон блоков на батчи
    fn split_into_batches(&self, from_block: u64, to_block: u64) -> Vec<(u64, u64)> {
        let mut batches = Vec::new();
        let mut current = from_block;

        while current <= to_block {
            let batch_end = std::cmp::min(current + self.config.batch_size - 1, to_block);
            batches.push((current, batch_end));
            current = batch_end + 1;
        }

        batches
    }

    /// Сканировать с автоматическим retry при ошибках
    pub async fn scan_range_with_retry(
        &self,
        from_block: u64,
        to_block: u64,
        wallets: &[String],
        max_retries: u32,
    ) -> Result<Vec<TransferEvent>, anyhow::Error> {
        let mut attempt = 0;
        let mut last_error = None;

        while attempt < max_retries {
            match self
                .scan_range_parallel(from_block, to_block, wallets)
                .await
            {
                Ok(events) => return Ok(events),
                Err(e) => {
                    attempt += 1;
                    last_error = Some(e);
                    if attempt < max_retries {
                        let delay = std::time::Duration::from_secs(2u64.pow(attempt));
                        warn!(
                            "Scan failed (attempt {}/{}), retrying in {:?}...",
                            attempt, max_retries, delay
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Max retries exceeded")))
    }
}
