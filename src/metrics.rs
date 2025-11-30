use axum::Router;
use prometheus::Registry;
use std::sync::Arc;
use tracing::info;

pub struct Metrics {
    registry: Registry,
    events_processed: prometheus::IntCounter,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let events_processed = prometheus::register_int_counter_with_registry!(
            "events_processed",
            "Total events processed",
            registry
        )
        .unwrap();
        Self {
            registry,
            events_processed,
        }
    }

    pub fn increment_events(&self) {
        self.events_processed.inc();
    }

    pub fn export(&self) -> String {
        let encoder = prometheus::TextEncoder::new();
        let mut buffer = String::new();
        let metric_families = prometheus::gather();
        encoder.encode_utf8(&metric_families, &mut buffer).unwrap();
        buffer
    }
}

pub fn start_metrics_server(metrics: Arc<Metrics>) {
    tokio::spawn(async move {
        let app = Router::new().route(
            "/metrics",
            axum::routing::get(move || async move { metrics.export() }),
        );
        let listener = tokio::net::TcpListener::bind("0.0.0.0:9090").await.unwrap();
        axum::serve(listener, app).await.unwrap();
        info!("Metrics server on 9090");
    });
}
