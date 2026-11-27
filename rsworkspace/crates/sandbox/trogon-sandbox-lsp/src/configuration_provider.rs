use std::sync::Arc;

use serde_json::Value;

use crate::configuration_item::ConfigurationItem;

/// Answers `workspace/configuration` items. The default answers `null` for every item.
pub type ConfigurationProvider = Arc<dyn Fn(&ConfigurationItem) -> Value + Send + Sync>;
