use std::sync::Arc;

use serde_json::Value;

/// Called with the params of every notification of the method it is registered for.
pub type NotificationHandler = Arc<dyn Fn(&Value) + Send + Sync>;
