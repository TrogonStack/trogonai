use std::time::Duration;

use async_nats::ConnectOptions;

use crate::settings::CtlSettings;

pub async fn connect(settings: &CtlSettings, timeout: Duration) -> Result<async_nats::Client, String> {
    let mut options = ConnectOptions::new().connection_timeout(timeout);
    if let Some(creds) = &settings.nats_creds {
        options = options
            .credentials_file(creds)
            .await
            .map_err(|error| format!("load NATS creds {}: {error}", creds.display()))?;
    }

    let servers: Vec<String> = settings
        .nats_url
        .split(',')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            if segment.contains("://") {
                segment.to_string()
            } else {
                format!("nats://{segment}")
            }
        })
        .collect();

    if servers.is_empty() {
        return Err("NATS_URL is empty".into());
    }

    async_nats::connect_with_options(servers, options)
        .await
        .map_err(|error| format!("NATS connect: {error}"))
}
