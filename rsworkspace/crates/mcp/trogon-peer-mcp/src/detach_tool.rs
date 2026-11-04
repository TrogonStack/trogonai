use rmcp::model::Tool;

use crate::detach_hook_input::DetachHookInput;

pub(crate) fn detach_tool() -> Tool {
    let mut tool = Tool::default().with_input_schema::<DetachHookInput>();
    tool.name = "peer_detach_hook".into();
    tool.description = Some("Detach the bound native session on SessionEnd, preserving durable delivery.".into());
    tool
}
