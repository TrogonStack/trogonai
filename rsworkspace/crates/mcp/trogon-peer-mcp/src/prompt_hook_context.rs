use serde::Serialize;

use crate::prompt_hook_event::PromptHookEvent;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptHookContext<'a> {
    pub(crate) hook_event_name: PromptHookEvent,
    pub(crate) additional_context: &'a str,
}
