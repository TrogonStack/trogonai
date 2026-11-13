//! The state a `ClientHandle` shares with its driver.

use std::sync::{Arc, Mutex, PoisonError};

use crate::UiHost;

/// What a `ClientHandle` and its driver share: the host that draws the CLI's prompt.
#[derive(Clone, Default)]
pub(crate) struct HostState(Arc<Mutex<Option<UiHost>>>);

impl HostState {
    pub(crate) fn ui_host(&self) -> Option<UiHost> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(crate) fn set_ui_host(&self, host: Option<UiHost>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = host;
    }
}
