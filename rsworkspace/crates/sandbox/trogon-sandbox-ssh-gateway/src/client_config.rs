use std::sync::Arc;

use crate::constants::{KEEPALIVE_INTERVAL, KEEPALIVE_MAX};
use russh::client;

/// The client config workers connect with: keepalives so a dead session is noticed.
pub fn client_config() -> Arc<client::Config> {
    Arc::new(client::Config {
        keepalive_interval: Some(KEEPALIVE_INTERVAL),
        keepalive_max: KEEPALIVE_MAX,
        ..Default::default()
    })
}
