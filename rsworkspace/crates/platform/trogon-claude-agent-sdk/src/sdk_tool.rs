use std::{future::Future, pin::Pin, sync::Arc};

use serde::Serialize;
use serde_json::Value;

use crate::{CallbackError, CancellationToken, JsonObject, McpToolInputSchema, McpToolName, ToolResultStatus};

#[derive(Clone, Debug)]
pub struct McpToolRequest {
    pub arguments: JsonObject,
    pub signal: CancellationToken,
}

#[derive(Clone, Debug)]
pub struct McpToolOutput {
    pub content: Vec<JsonObject>,
    pub structured_content: Option<JsonObject>,
    pub status: ToolResultStatus,
    /// The result exactly as the handler produced it, when it is not one of the typed fields above.
    pub(crate) result: Option<Value>,
}

impl McpToolOutput {
    /// A handler result passed through untouched: any `_meta`, task fields and unknown keys survive in
    /// their order, and a value that is not a valid `CallToolResult` is reported the way the MCP SDK
    /// reports it. The typed fields stay empty.
    pub fn from_result(result: Value) -> Self {
        Self {
            content: Vec::new(),
            structured_content: None,
            status: ToolResultStatus::NotReported,
            result: Some(result),
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        let mut content = JsonObject::new();
        content.insert("type".into(), "text".into());
        content.insert("text".into(), text.into().into());
        Self {
            content: vec![content],
            structured_content: None,
            status: ToolResultStatus::Success,
            result: None,
        }
    }

    pub(crate) fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        if let Some(result) = &self.result {
            return Ok(result.clone());
        }
        serde_json::to_value(ToolOutputWire {
            content: &self.content,
            structured_content: self.structured_content.as_ref(),
            is_error: match self.status {
                ToolResultStatus::NotReported => None,
                ToolResultStatus::Success => Some(false),
                ToolResultStatus::Error => Some(true),
            },
        })
    }
}

pub type McpToolFuture<'a> = Pin<Box<dyn Future<Output = Result<McpToolOutput, CallbackError>> + Send + 'a>>;

pub trait McpToolHandler: Send + Sync {
    fn call(&self, request: McpToolRequest) -> McpToolFuture<'_>;
}

impl<F, Fut> McpToolHandler for F
where
    F: Fn(McpToolRequest) -> Fut + Send + Sync,
    Fut: Future<Output = Result<McpToolOutput, CallbackError>> + Send + 'static,
{
    fn call(&self, request: McpToolRequest) -> McpToolFuture<'_> {
        Box::pin(self(request))
    }
}

#[derive(Clone)]
pub struct SdkToolOptions {
    pub name: McpToolName,
    pub description: String,
    pub input_schema: McpToolInputSchema,
    pub handler: Arc<dyn McpToolHandler>,
    /// MCP tool annotations, listed as `annotations`. The Python dialect lists only the five standard hints.
    pub annotations: Option<JsonObject>,
    /// TypeScript `searchHint`, listed as `_meta["anthropic/searchHint"]` when non-empty.
    pub search_hint: Option<String>,
    /// TypeScript `alwaysLoad`, listed as `_meta["anthropic/alwaysLoad"]`.
    pub always_load: bool,
}

impl std::fmt::Debug for SdkToolOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SdkToolOptions")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct SdkTool {
    name: McpToolName,
    description: String,
    input_schema: McpToolInputSchema,
    handler: Arc<dyn McpToolHandler>,
    annotations: Option<JsonObject>,
    search_hint: Option<String>,
    always_load: bool,
}

impl From<SdkToolOptions> for SdkTool {
    fn from(options: SdkToolOptions) -> Self {
        let SdkToolOptions {
            name,
            description,
            input_schema,
            handler,
            annotations,
            search_hint,
            always_load,
        } = options;
        Self {
            name,
            description,
            input_schema,
            handler,
            annotations,
            search_hint,
            always_load,
        }
    }
}

impl SdkTool {
    pub fn name(&self) -> &McpToolName {
        &self.name
    }

    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    pub(crate) fn input_schema(&self) -> &McpToolInputSchema {
        &self.input_schema
    }

    pub(crate) fn annotations(&self) -> Option<&JsonObject> {
        self.annotations.as_ref()
    }

    pub(crate) fn search_hint(&self) -> Option<&str> {
        self.search_hint.as_deref()
    }

    pub(crate) fn always_load(&self) -> bool {
        self.always_load
    }

    pub(crate) fn call(&self, request: McpToolRequest) -> McpToolFuture<'_> {
        self.handler.call(request)
    }
}

impl std::fmt::Debug for SdkTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SdkTool")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolOutputWire<'a> {
    content: &'a [JsonObject],
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_content: Option<&'a JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_error: Option<bool>,
}
