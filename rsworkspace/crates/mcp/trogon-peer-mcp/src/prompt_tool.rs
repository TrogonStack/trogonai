use rmcp::model::Tool;

use crate::prompt_hook_input::PromptHookInput;

pub(crate) fn prompt_tool() -> Tool {
    let mut tool = Tool::default().with_input_schema::<PromptHookInput>();
    tool.name = "peer_prompt_hook".into();
    tool.description =
        Some("Bind this connector to the native host session and prepare peer context for UserPromptSubmit.".into());
    tool
}
