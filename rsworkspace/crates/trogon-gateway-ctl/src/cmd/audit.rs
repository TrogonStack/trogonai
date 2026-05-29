use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;
use tokio::time::timeout;

use crate::nats::connect;
use crate::output::emit_json;
use crate::settings::CtlSettings;

pub async fn run(settings: &CtlSettings, max_messages: usize, pretty: bool) -> Result<(), String> {
    let client = connect(settings, Duration::from_secs(15)).await?;
    let subject = settings.audit_subject_wildcard();
    let mut subscription = client
        .subscribe(subject.clone())
        .await
        .map_err(|error| format!("subscribe {subject}: {error}"))?;

    let mut delivered = 0usize;
    loop {
        let next = timeout(Duration::from_secs(2), subscription.next()).await;
        let message = match next {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(_) => {
                if delivered == 0 {
                    return Err(format!("timed out waiting for audit events on {subject}"));
                }
                break;
            }
        };

        let payload: Value = serde_json::from_slice(&message.payload)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&message.payload).into_owned()));
        let envelope = serde_json::json!({
            "subject": message.subject.to_string(),
            "envelope": payload,
        });
        emit_json(&envelope, pretty).map_err(|error| error.to_string())?;
        delivered += 1;
        if delivered >= max_messages {
            break;
        }
    }

    Ok(())
}
