use std::time::Duration;

use tracing::info;
use trogon_nats::jetstream::JetStreamContext;

use super::config::GooglePubSubConfig;

pub async fn provision<C: JetStreamContext>(js: &C, config: &GooglePubSubConfig) -> Result<(), C::Error> {
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: config.stream_name.to_string(),
        subjects: vec![format!("{}.>", config.subject_prefix)],
        max_age: config.stream_max_age.into(),
        ..Default::default()
    })
    .await?;

    let max_age_secs = Duration::from(config.stream_max_age).as_secs();
    let stream_name = config.stream_name.as_str();
    info!(stream = stream_name, max_age_secs, "JetStream stream ready");
    Ok(())
}

#[cfg(test)]
mod tests;
