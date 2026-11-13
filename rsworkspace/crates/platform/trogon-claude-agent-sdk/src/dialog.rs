use crate::{CallbackError, CancellationToken, ControlRequestId, DialogKind, JsonObject, McpServerName, ToolUseId};
use serde::Serialize;
use serde_json::Value;
use std::{future::Future, pin::Pin};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ElicitationMode {
    Form,
    Url,
    /// A mode this SDK does not name, passed on as TypeScript passes it.
    Other(String),
}

impl ElicitationMode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Form => "form",
            Self::Url => "url",
            Self::Other(mode) => mode,
        }
    }
}

impl From<String> for ElicitationMode {
    fn from(mode: String) -> Self {
        match mode.as_str() {
            "form" => Self::Form,
            "url" => Self::Url,
            _ => Self::Other(mode),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ElicitationRequest {
    pub server_name: McpServerName,
    pub message: String,
    /// Absent when the CLI sent no mode; TypeScript hands the handler `undefined` then.
    pub mode: Option<ElicitationMode>,
    pub url: Option<String>,
    pub elicitation_id: Option<String>,
    pub requested_schema: Option<JsonObject>,
    pub title: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub request_id: ControlRequestId,
    pub signal: CancellationToken,
}

#[derive(Clone, Debug)]
pub enum ElicitationResponse {
    Accept {
        content: Option<JsonObject>,
    },
    Decline,
    Cancel,
    /// TypeScript `onElicitation` resolving to `null`: the host answered out of band, so no response is
    /// written.
    AnsweredOutOfBand,
}

#[derive(Serialize)]
#[serde(tag = "action", rename_all = "lowercase")]
enum ElicitationResponseWire<'a> {
    Accept {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<&'a JsonObject>,
    },
    Decline,
    Cancel,
}

impl ElicitationResponse {
    /// The handler result as JSON; [`Self::AnsweredOutOfBand`] is `null`, the value the TypeScript handler
    /// returns.
    pub fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        let wire = match self {
            Self::Accept { content } => ElicitationResponseWire::Accept {
                content: content.as_ref(),
            },
            Self::Decline => ElicitationResponseWire::Decline,
            Self::Cancel => ElicitationResponseWire::Cancel,
            Self::AnsweredOutOfBand => return Ok(Value::Null),
        };
        serde_json::to_value(wire)
    }
}

pub type ElicitationFuture<'a> = Pin<Box<dyn Future<Output = Result<ElicitationResponse, CallbackError>> + Send + 'a>>;
pub trait ElicitationHandler: Send + Sync {
    fn call(&self, request: ElicitationRequest) -> ElicitationFuture<'_>;
}
impl<F, Fut> ElicitationHandler for F
where
    F: Fn(ElicitationRequest) -> Fut + Send + Sync,
    Fut: Future<Output = Result<ElicitationResponse, CallbackError>> + Send + 'static,
{
    fn call(&self, request: ElicitationRequest) -> ElicitationFuture<'_> {
        Box::pin(self(request))
    }
}

#[derive(Clone, Debug)]
pub struct UserDialogRequest {
    pub dialog_kind: DialogKind,
    pub payload: JsonObject,
    pub tool_use_id: Option<ToolUseId>,
    pub request_id: ControlRequestId,
    pub signal: CancellationToken,
}

#[derive(Clone, Debug)]
pub enum UserDialogResponse {
    Completed {
        result: Value,
    },
    Cancelled,
    /// TypeScript `onUserDialog` resolving to `null`: the host answered out of band, so no response is written.
    AnsweredOutOfBand,
}

#[derive(Serialize)]
#[serde(tag = "behavior", rename_all = "lowercase")]
enum UserDialogResponseWire<'a> {
    Completed { result: &'a Value },
    Cancelled,
}

impl UserDialogResponse {
    /// The handler result as JSON; [`Self::AnsweredOutOfBand`] is `null`, the value the TypeScript handler
    /// returns.
    pub fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        let wire = match self {
            Self::Completed { result } => UserDialogResponseWire::Completed { result },
            Self::Cancelled => UserDialogResponseWire::Cancelled,
            Self::AnsweredOutOfBand => return Ok(Value::Null),
        };
        serde_json::to_value(wire)
    }
}

pub type UserDialogFuture<'a> = Pin<Box<dyn Future<Output = Result<UserDialogResponse, CallbackError>> + Send + 'a>>;
pub trait UserDialogHandler: Send + Sync {
    fn call(&self, request: UserDialogRequest) -> UserDialogFuture<'_>;
}
impl<F, Fut> UserDialogHandler for F
where
    F: Fn(UserDialogRequest) -> Fut + Send + Sync,
    Fut: Future<Output = Result<UserDialogResponse, CallbackError>> + Send + 'static,
{
    fn call(&self, request: UserDialogRequest) -> UserDialogFuture<'_> {
        Box::pin(self(request))
    }
}

impl std::fmt::Debug for dyn ElicitationHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ElicitationHandler").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for dyn UserDialogHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("UserDialogHandler").finish_non_exhaustive()
    }
}
