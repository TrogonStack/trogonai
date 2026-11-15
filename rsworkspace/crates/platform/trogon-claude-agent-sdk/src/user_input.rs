use crate::{JsonObject, MessageId, MessageOrigin, Prompt, SessionId, ToolUseId, UserInputPriority};
use serde_json::Value;

/// One structured content block, forwarded to the CLI verbatim.
///
/// Both pinned upstream SDKs write user content blocks without validating
/// them; the CLI and the Messages API own block validation.
#[derive(Clone, Debug)]
pub struct InputBlock(JsonObject);

impl InputBlock {
    pub fn from_json(value: JsonObject) -> Self {
        Self(value)
    }

    pub fn text(text: impl Into<String>) -> Self {
        let mut object = JsonObject::new();
        object.insert("type".into(), "text".into());
        object.insert("text".into(), text.into().into());
        Self(object)
    }

    pub fn as_json(&self) -> &JsonObject {
        &self.0
    }
}

impl From<JsonObject> for InputBlock {
    fn from(value: JsonObject) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug)]
enum InputContent {
    Text(Prompt),
    Blocks(Vec<InputBlock>),
    /// A whole `SDKUserMessage` object, written as given.
    Message(JsonObject),
}

/// How a [`UserInput`] goes on the wire.
#[derive(Clone, Copy, Debug)]
enum InputShape {
    /// As a caller builds an `SDKUserMessage`: `type`, `message`, `parent_tool_use_id`, then each set field in the
    /// order the type declares it. A text prompt stays a string.
    Caller,
    /// As a one-shot `query()` writes a string prompt: `type`, `session_id`, `message`, `parent_tool_use_id`.
    /// `sdk.mjs` wraps the text in a single text block; Python writes it as a string.
    QueryPrompt { text_block: bool },
}

/// One `SDKUserMessage` / Python user message dict written to the CLI.
///
/// Both pinned upstream SDKs write a streamed user message as the caller built it, so every field the type
/// declares is here, and `additional_fields` carries keys this crate does not model yet. A field left `None` is not
/// written. [`UserInput::from_json`] keeps a whole message object, key order included.
#[derive(Clone, Debug)]
pub struct UserInput {
    content: InputContent,
    shape: InputShape,
    pub parent_tool_use_id: Option<ToolUseId>,
    pub is_synthetic: Option<bool>,
    pub tool_use_result: Option<Value>,
    pub priority: Option<UserInputPriority>,
    pub origin: Option<MessageOrigin>,
    pub should_query: Option<bool>,
    pub timestamp: Option<String>,
    pub client_composed: Option<bool>,
    pub uuid: Option<MessageId>,
    pub pasted_content: Option<Vec<Value>>,
    pub inline_pastes: Option<Vec<String>>,
    pub session_id: Option<SessionId>,
    pub agent_id: Option<String>,
    pub subagent_type: Option<String>,
    pub task_description: Option<String>,
    /// Keys written after every typed field, unless the message already has them.
    pub additional_fields: JsonObject,
    /// The session Python `ClaudeSDKClient.query` appends to a streamed message that names none.
    trailing_session: Option<SessionId>,
}

impl From<Prompt> for UserInput {
    fn from(value: Prompt) -> Self {
        Self::new(InputContent::Text(value))
    }
}

impl UserInput {
    fn new(content: InputContent) -> Self {
        Self {
            content,
            shape: InputShape::Caller,
            parent_tool_use_id: None,
            is_synthetic: None,
            tool_use_result: None,
            priority: None,
            origin: None,
            should_query: None,
            timestamp: None,
            client_composed: None,
            uuid: None,
            pasted_content: None,
            inline_pastes: None,
            session_id: None,
            agent_id: None,
            subagent_type: None,
            task_description: None,
            additional_fields: JsonObject::new(),
            trailing_session: None,
        }
    }

    pub fn blocks(blocks: Vec<InputBlock>) -> Self {
        Self::new(InputContent::Blocks(blocks))
    }

    /// A whole user message object, written as given, key order and unknown keys included, as both upstream SDKs
    /// write a streamed message. A typed field set afterwards replaces that key in place, or is added at the end.
    pub fn from_json(message: JsonObject) -> Self {
        Self::new(InputContent::Message(message))
    }

    /// This input as a one-shot `query()` writes its prompt: `sdk.mjs` when `text_block` is set, Python otherwise.
    /// Only a text prompt changes shape; any other input is a streamed message, which both SDKs write as given.
    pub(crate) fn into_query_prompt(mut self, text_block: bool) -> Self {
        if let InputContent::Text(_) = self.content {
            self.shape = InputShape::QueryPrompt { text_block };
        }
        self
    }

    /// Python `ClaudeSDKClient.query`: `if "session_id" not in msg: msg["session_id"] = session_id`.
    pub(crate) fn fill_session(&mut self, session_id: &SessionId) {
        let named = self.session_id.is_some()
            || self.additional_fields.contains_key("session_id")
            || matches!(&self.content, InputContent::Message(message) if message.contains_key("session_id"));
        if !named {
            self.trailing_session = Some(session_id.clone());
        }
    }

    pub fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        let mut object = JsonObject::new();
        let content = match (&self.content, self.shape) {
            (InputContent::Message(message), _) => {
                object.clone_from(message);
                None
            }
            (InputContent::Text(prompt), InputShape::QueryPrompt { text_block: true }) => {
                Some(Value::Array(vec![InputBlock::text(prompt.as_str()).0.into()]))
            }
            (InputContent::Text(prompt), _) => Some(Value::from(prompt.as_str())),
            (InputContent::Blocks(blocks), _) => Some(Value::Array(
                blocks.iter().map(|block| block.0.clone().into()).collect(),
            )),
        };
        if let Some(content) = content {
            let mut message = JsonObject::new();
            message.insert("role".into(), "user".into());
            message.insert("content".into(), content);
            object.insert("type".into(), "user".into());
            if let InputShape::QueryPrompt { .. } = self.shape {
                object.insert("session_id".into(), "".into());
            }
            object.insert("message".into(), message.into());
            object.insert("parent_tool_use_id".into(), Value::Null);
        }
        let origin = self.origin.as_ref().map(serde_json::to_value).transpose()?;
        for (key, value) in [
            (
                "parent_tool_use_id",
                self.parent_tool_use_id.as_ref().map(|id| Value::from(id.as_str())),
            ),
            ("isSynthetic", self.is_synthetic.map(Value::from)),
            ("tool_use_result", self.tool_use_result.clone()),
            (
                "priority",
                self.priority.as_ref().map(|priority| Value::from(priority.as_str())),
            ),
            ("origin", origin),
            ("shouldQuery", self.should_query.map(Value::from)),
            ("timestamp", self.timestamp.clone().map(Value::from)),
            ("client_composed", self.client_composed.map(Value::from)),
            ("uuid", self.uuid.as_ref().map(|uuid| Value::from(uuid.as_str()))),
            ("pasted_content", self.pasted_content.clone().map(Value::from)),
            ("inline_pastes", self.inline_pastes.clone().map(Value::from)),
            (
                "session_id",
                self.session_id.as_ref().map(|session| Value::from(session.as_str())),
            ),
            ("agent_id", self.agent_id.clone().map(Value::from)),
            ("subagent_type", self.subagent_type.clone().map(Value::from)),
            ("task_description", self.task_description.clone().map(Value::from)),
        ] {
            if let Some(value) = value {
                object.insert(key.into(), value);
            }
        }
        for (key, value) in &self.additional_fields {
            object.entry(key.clone()).or_insert_with(|| value.clone());
        }
        if let Some(session_id) = &self.trailing_session {
            object.entry("session_id").or_insert_with(|| session_id.as_str().into());
        }
        Ok(Value::Object(object))
    }
}

#[cfg(test)]
#[path = "user_input_tests.rs"]
mod tests;
