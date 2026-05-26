use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::push_notification_config_id::PushNotificationConfigId;
use crate::task_id::A2aTaskId;

/// Bridge-held extension for `tasks/pushNotificationConfig/*` payloads until upstream `TaskPushNotificationConfig` carries semantics.
#[derive(Clone, Default)]
pub struct PushDeliverySemanticsRegistry {
    pairs: Arc<Mutex<HashMap<(A2aTaskId, PushNotificationConfigId), DeliverySemantics>>>,
}

impl PushDeliverySemanticsRegistry {
    pub fn set(&self, task_id: A2aTaskId, cfg_id: PushNotificationConfigId, semantics: DeliverySemantics) {
        let mut pairs = self.pairs.lock().unwrap();
        pairs.insert((task_id, cfg_id), semantics);
    }

    pub fn remove(&self, task_id: &A2aTaskId, cfg_id: &PushNotificationConfigId) {
        self.pairs.lock().unwrap().remove(&(task_id.clone(), cfg_id.clone()));
    }

    pub fn get(&self, task_id: &A2aTaskId, cfg_id: &PushNotificationConfigId) -> DeliverySemantics {
        self.pairs
            .lock()
            .unwrap()
            .get(&(task_id.clone(), cfg_id.clone()))
            .cloned()
            .unwrap_or_default()
    }
}
