use crate::models::TransferEvent;
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use serde_json;
use tracing::{info, warn};

#[derive(Clone)]
pub enum SinkType {
    Console,
    Kafka {
        producer: FutureProducer,
        topic: String,
    },
}

impl SinkType {
    pub fn console() -> Self {
        SinkType::Console
    }

    pub fn kafka(brokers: &str, topic: &str) -> Result<Self, anyhow::Error> {
        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            .set("message.timeout.ms", "5000")
            .create()?;

        Ok(SinkType::Kafka {
            producer,
            topic: topic.to_string(),
        })
    }
}

impl SinkType {
    pub async fn send(&mut self, event: TransferEvent) -> Result<(), anyhow::Error> {
        match self {
            SinkType::Console => Ok(()),
            SinkType::Kafka { producer, topic } => {
                let payload = serde_json::to_string(&event)?;
                let record = FutureRecord::to(topic)
                    .payload(&payload)
                    .key(&event.tx_hash);

                match producer
                    .send(record, std::time::Duration::from_secs(0))
                    .await
                {
                    Ok(_) => {
                        info!("Event sent to Kafka: {}", event.tx_hash);
                    }
                    Err((e, _)) => {
                        warn!("Failed to send event to Kafka: {}", e);
                        return Err(anyhow::anyhow!("Kafka send error: {}", e));
                    }
                }
                Ok(())
            }
        }
    }
}
