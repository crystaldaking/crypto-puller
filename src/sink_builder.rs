use crate::config::Config;
use crypto_puller::sink::SinkType;
use tracing::{error, info};

pub fn build_sink(config: &Config) -> Result<SinkType, Box<dyn std::error::Error>> {
    if config.use_console_instead_kafka {
        return Ok(SinkType::console());
    }

    if let (Some(brokers), Some(topic)) = (config.kafka_brokers.clone(), config.kafka_topic.clone())
    {
        match SinkType::kafka(&brokers, &topic) {
            Ok(s) => Ok(s),
            Err(e) => {
                error!("Failed to create Kafka sink (fallback to console): {}", e);
                Ok(SinkType::console())
            }
        }
    } else {
        info!("Kafka not configured, using console sink");
        Ok(SinkType::console())
    }
}
