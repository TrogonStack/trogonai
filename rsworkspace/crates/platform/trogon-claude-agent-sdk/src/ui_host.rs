//! TypeScript's `uiHost`, a host that draws the CLI's prompt through the `ui_*` control requests.
//!
//! Python 0.2.181 has no such host and answers `Unsupported control request subtype`, so none of this applies to
//! [`crate::ArgvStyle::Python`].

use std::{future::Future, pin::Pin, sync::Arc};

use serde_json::Value;

use crate::{CallbackError, CancellationToken, JsonObject};

/// What a `uiHost` method receives: the request without its `subtype`, and the request's signal.
#[derive(Clone, Debug)]
pub struct UiRequest {
    pub payload: JsonObject,
    pub signal: CancellationToken,
}

pub type UiFuture<'a> = Pin<Box<dyn Future<Output = Result<Option<Value>, CallbackError>> + Send + 'a>>;

/// One `uiHost` method. Its value is the response; `None` is `undefined`, which leaves `response` out of the
/// frame.
pub trait UiHandler: Send + Sync {
    fn call(&self, request: UiRequest) -> UiFuture<'_>;
}

impl<F, Fut> UiHandler for F
where
    F: Fn(UiRequest) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Option<Value>, CallbackError>> + Send + 'static,
{
    fn call(&self, request: UiRequest) -> UiFuture<'_> {
        Box::pin(self(request))
    }
}

impl std::fmt::Debug for dyn UiHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("UiHandler").finish_non_exhaustive()
    }
}

/// TypeScript `uiHost`. A subtype whose method is absent falls through to the unsupported request error.
#[derive(Clone, Default)]
pub struct UiHost {
    pub copy: Option<Arc<dyn UiHandler>>,
    pub prompt_read: Option<Arc<dyn UiHandler>>,
    pub prompt_fill: Option<Arc<dyn UiHandler>>,
    pub prompt_paint: Option<Arc<dyn UiHandler>>,
    pub prompt_suggest: Option<Arc<dyn UiHandler>>,
    pub read_selection: Option<Arc<dyn UiHandler>>,
}

impl UiHost {
    /// The method that answers `subtype`, when the subtype is a `ui_*` one this host implements.
    pub(crate) fn handler(&self, subtype: &str) -> Option<Arc<dyn UiHandler>> {
        match subtype {
            "ui_copy" => self.copy.clone(),
            "ui_prompt_read" => self.prompt_read.clone(),
            "ui_prompt_fill" => self.prompt_fill.clone(),
            "ui_prompt_paint" => self.prompt_paint.clone(),
            "ui_prompt_suggest" => self.prompt_suggest.clone(),
            "ui_read_selection" => self.read_selection.clone(),
            _ => None,
        }
    }
}

impl std::fmt::Debug for UiHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UiHost")
            .field("copy", &self.copy.is_some())
            .field("prompt_read", &self.prompt_read.is_some())
            .field("prompt_fill", &self.prompt_fill.is_some())
            .field("prompt_paint", &self.prompt_paint.is_some())
            .field("prompt_suggest", &self.prompt_suggest.is_some())
            .field("read_selection", &self.read_selection.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests;
