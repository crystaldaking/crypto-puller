use std::sync::Arc;
use axum::Router;
use prometheus::Registry;
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
        prometheus::gather()
            .iter()
            .map(|m| prometheus::TextEncoder::new().format(m))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn start_metrics_server(metrics: Arc<Metrics>) {
    tokio::spawn(async move {
        let app = Router::new().route(
            "/metrics",
            axum::routing::get(move || async move { metrics.export() }),
        );
        axum::Server::bind(&"0.0.0.0:9090".parse().unwrap())
            .serve(app.into_make_service())
            .await
            .unwrap();
        info!("Metrics server on 9090");
    });
}
