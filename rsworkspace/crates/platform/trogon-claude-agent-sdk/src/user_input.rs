use crate::{JsonObject, MessageId, MessageOrigin, Prompt, SessionId, ToolUseId};
use serde::Serialize;
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
}

/// How a [`UserInput`] goes on the wire.
#[derive(Clone, Copy, Debug)]
enum InputShape {
    /// As the caller built it: a text prompt stays a string.
    Caller,
    /// As `sdk.mjs` writes a string prompt to `query()`: the text becomes a single text block, and the fields
    /// follow its object literal.
    SdkMjsPrompt,
}

#[derive(Clone, Debug)]
pub struct UserInput {
    content: InputContent,
    shape: InputShape,
    pub uuid: Option<MessageId>,
    pub session_id: Option<SessionId>,
    pub parent_tool_use_id: Option<ToolUseId>,
    pub origin: Option<MessageOrigin>,
}

impl From<Prompt> for UserInput {
    fn from(value: Prompt) -> Self {
        Self {
            content: InputContent::Text(value),
            shape: InputShape::Caller,
            uuid: None,
            session_id: None,
            parent_tool_use_id: None,
            origin: None,
        }
    }
}

#[derive(Serialize)]
struct UserInputMessageWire {
    role: &'static str,
    content: Value,
}

/// `{type:"user",session_id:"",message:{role:"user",content:[{type:"text",text:t}]},parent_tool_use_id:null}`, the
/// line `sdk.mjs` writes for a string prompt.
#[derive(Serialize)]
struct SdkMjsPromptWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    session_id: &'a str,
    message: UserInputMessageWire,
    parent_tool_use_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uuid: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<&'a MessageOrigin>,
}

#[derive(Serialize)]
struct UserInputWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    session_id: &'a str,
    parent_tool_use_id: Option<&'a str>,
    message: UserInputMessageWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    uuid: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<&'a MessageOrigin>,
}

impl UserInput {
    pub fn blocks(blocks: Vec<InputBlock>) -> Self {
        Self {
            content: InputContent::Blocks(blocks),
            shape: InputShape::Caller,
            uuid: None,
            session_id: None,
            parent_tool_use_id: None,
            origin: None,
        }
    }

    /// This input as `sdk.mjs` writes the prompt of a `query()` call.
    pub(crate) fn into_sdk_mjs_prompt(mut self) -> Self {
        self.shape = InputShape::SdkMjsPrompt;
        self
    }

    pub fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        let content = match (&self.content, self.shape) {
            (InputContent::Text(prompt), InputShape::Caller) => Value::String(prompt.as_str().to_owned()),
            (InputContent::Text(prompt), InputShape::SdkMjsPrompt) => {
                Value::Array(vec![InputBlock::text(prompt.as_str()).0.into()])
            }
            (InputContent::Blocks(blocks), _) => {
                Value::Array(blocks.iter().map(|block| block.0.clone().into()).collect())
            }
        };
        if let InputShape::SdkMjsPrompt = self.shape {
            return serde_json::to_value(SdkMjsPromptWire {
                kind: "user",
                session_id: self.session_id.as_ref().map_or("", SessionId::as_str),
                message: UserInputMessageWire { role: "user", content },
                parent_tool_use_id: self.parent_tool_use_id.as_ref().map(ToolUseId::as_str),
                uuid: self.uuid.as_ref().map(MessageId::as_str),
                origin: self.origin.as_ref(),
            });
        }
        serde_json::to_value(UserInputWire {
            kind: "user",
            session_id: self.session_id.as_ref().map_or("", SessionId::as_str),
            parent_tool_use_id: self.parent_tool_use_id.as_ref().map(ToolUseId::as_str),
            message: UserInputMessageWire { role: "user", content },
            uuid: self.uuid.as_ref().map(MessageId::as_str),
            origin: self.origin.as_ref(),
        })
    }
}

#[cfg(test)]
#[path = "user_input_tests.rs"]
mod tests;
