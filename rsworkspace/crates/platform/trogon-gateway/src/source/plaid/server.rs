use std::time::Duration;

use tracing::info;
use trogon_nats::jetstream::{JetStreamContext, ProvisionedStreamField};

use super::PlaidConfig;
use super::dedup_window::DedupWindow;

pub async fn provision<C: JetStreamContext>(js: &C, config: &PlaidConfig) -> Result<(), C::Error> {
    // A token verifies for the tolerance on either side of its `iat`, so replays
    // of one delivery can arrive up to twice the tolerance apart.
    let dedup_window = DedupWindow::spanning(config.timestamp_tolerance, config.stream_max_age);
    js.create_or_reconcile_stream(
        async_nats::jetstream::stream::Config {
            name: config.stream_name.as_str().to_owned(),
            subjects: vec![format!("{}.>", config.subject_prefix)],
            duplicate_window: dedup_window.into(),
            max_age: config.stream_max_age.into(),
            ..Default::default()
        },
        &[
            ProvisionedStreamField::Subjects,
            ProvisionedStreamField::DuplicateWindow,
            ProvisionedStreamField::MaxAge,
        ],
    )
    .await?;

    let max_age_secs = Duration::from(config.stream_max_age).as_secs();
    let stream_name = config.stream_name.as_str();
    let dedup_window_secs = Duration::from(dedup_window).as_secs();
    info!(
        stream = stream_name,
        max_age_secs, dedup_window_secs, "JetStream stream ready"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
