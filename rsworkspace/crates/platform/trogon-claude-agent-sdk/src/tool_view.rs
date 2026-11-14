//! Typed views of tool inputs and results, keyed by the tool name the way the pinned `sdk-tools` declarations
//! pair each built-in tool with its input and output types. Every view decodes a copy: the raw value stays
//! untouched, nothing is validated, and a value the named type does not reproduce exactly is `Unknown`.

use serde_json::Value;

use crate::{
    BuiltinToolName,
    upstream::{
        tool_inputs::ToolInputSchemas,
        tool_outputs::{AgentOutput, ArtifactOutput, FileReadOutput, McpOutput, ProjectsOutput, ToolOutputSchemas},
    },
};

/// The prefix the CLI gives every MCP tool name; `sdk-tools` types those inputs as `McpInput` and their results
/// as `McpOutput`.
const MCP_TOOL_PREFIX: &str = "mcp__";

impl ToolInputSchemas {
    /// `input` as the `sdk-tools` input type of the tool `tool_name` names, a legacy alias resolved like the CLI
    /// resolves it, or [`ToolInputSchemas::Unknown`] with the value unchanged when the tool has no declared input
    /// type or the value does not decode to it exactly. Encoding the result reproduces `input`.
    pub fn for_tool(tool_name: &str, input: &Value) -> Self {
        let decoded = match BuiltinToolName::canonical_name(tool_name) {
            "Agent" => exact(input).map(Self::AgentInput),
            "Artifact" => exact(input).map(Self::ArtifactInput),
            "AskUserQuestion" => exact(input).map(Self::AskUserQuestionInput),
            "Bash" => exact(input).map(Self::BashInput),
            "ClaudeDesign" => exact(input).map(Self::ClaudeDesignInput),
            "CronCreate" => exact(input).map(Self::CronCreateInput),
            "CronDelete" => exact(input).map(Self::CronDeleteInput),
            "CronList" => exact(input).map(Self::CronListInput),
            "Edit" => exact(input).map(Self::FileEditInput),
            "EnterPlanMode" => exact(input).map(Self::EnterPlanModeInput),
            "EnterWorktree" => exact(input).map(Self::EnterWorktreeInput),
            "ExitPlanMode" => exact(input).map(Self::ExitPlanModeInput),
            "ExitWorktree" => exact(input).map(Self::ExitWorktreeInput),
            "Glob" => exact(input).map(Self::GlobInput),
            "Grep" => exact(input).map(Self::GrepInput),
            "ListMcpResourcesTool" => exact(input).map(Self::ListMcpResourcesInput),
            "Monitor" => exact(input).map(Self::MonitorInput),
            "NotebookEdit" => exact(input).map(Self::NotebookEditInput),
            "OfferChromeSetup" => exact(input).map(Self::OfferChromeSetupInput),
            "Projects" => exact(input).map(Self::ProjectsInput),
            "propose_skills" => exact(input).map(Self::ProposeSkillsInput),
            "ProposeGoal" => exact(input).map(Self::ProposeGoalInput),
            "PushNotification" => exact(input).map(Self::PushNotificationInput),
            "Read" => exact(input).map(Self::FileReadInput),
            "ReadMcpResourceDirTool" => exact(input).map(Self::ReadMcpResourceDirInput),
            "ReadMcpResourceTool" => exact(input).map(Self::ReadMcpResourceInput),
            "ReadNotifications" => exact(input).map(Self::ReadNotificationsInput),
            "RefreshMcpTools" => exact(input).map(Self::RefreshMcpToolsInput),
            "RemoteTrigger" => exact(input).map(Self::RemoteTriggerInput),
            "ReportFindings" => exact(input).map(Self::ReportFindingsInput),
            "ScheduleWakeup" => exact(input).map(Self::ScheduleWakeupInput),
            "SendFeedback" => exact(input).map(Self::SendFeedbackInput),
            "ShowOnboardingRolePicker" => exact(input).map(Self::ShowOnboardingRolePickerInput),
            "TaskCreate" => exact(input).map(Self::TaskCreateInput),
            "TaskGet" => exact(input).map(Self::TaskGetInput),
            "TaskList" => exact(input).map(Self::TaskListInput),
            "TaskStop" => exact(input).map(Self::TaskStopInput),
            "TaskUpdate" => exact(input).map(Self::TaskUpdateInput),
            "TodoWrite" => exact(input).map(Self::TodoWriteInput),
            "WebFetch" => exact(input).map(Self::WebFetchInput),
            "WebSearch" => exact(input).map(Self::WebSearchInput),
            "Workflow" => exact(input).map(Self::WorkflowInput),
            "Write" => exact(input).map(Self::FileWriteInput),
            name if name.starts_with(MCP_TOOL_PREFIX) => exact(input).map(Self::McpInput),
            _ => None,
        };
        decoded.unwrap_or_else(|| Self::Unknown(input.clone()))
    }
}

impl ToolOutputSchemas {
    /// `output` as the `sdk-tools` output type of the tool `tool_name` names, a legacy alias resolved like the
    /// CLI resolves it, or [`ToolOutputSchemas::Unknown`] with the value unchanged when the tool has no declared
    /// output type or the value does not decode to it exactly. Encoding the result reproduces `output`.
    pub fn for_tool(tool_name: &str, output: &Value) -> Self {
        let decoded = match BuiltinToolName::canonical_name(tool_name) {
            "Agent" => AgentOutput::best_match(output).map(|(_, inner)| Self::AgentOutput(inner)),
            "Artifact" => ArtifactOutput::best_match(output).map(|(_, inner)| Self::ArtifactOutput(inner)),
            "AskUserQuestion" => exact(output).map(Self::AskUserQuestionOutput),
            "Bash" => exact(output).map(Self::BashOutput),
            "ClaudeDesign" => exact(output).map(Self::ClaudeDesignOutput),
            "CronCreate" => exact(output).map(Self::CronCreateOutput),
            "CronDelete" => exact(output).map(Self::CronDeleteOutput),
            "CronList" => exact(output).map(Self::CronListOutput),
            "Edit" => exact(output).map(Self::FileEditOutput),
            "EnterPlanMode" => exact(output).map(Self::EnterPlanModeOutput),
            "EnterWorktree" => exact(output).map(Self::EnterWorktreeOutput),
            "ExitPlanMode" => exact(output).map(Self::ExitPlanModeOutput),
            "ExitWorktree" => exact(output).map(Self::ExitWorktreeOutput),
            "Glob" => exact(output).map(Self::GlobOutput),
            "Grep" => exact(output).map(Self::GrepOutput),
            "ListMcpResourcesTool" => exact(output).map(Self::ListMcpResourcesOutput),
            "Monitor" => exact(output).map(Self::MonitorOutput),
            "NotebookEdit" => exact(output).map(Self::NotebookEditOutput),
            "OfferChromeSetup" => exact(output).map(Self::OfferChromeSetupOutput),
            "Projects" => ProjectsOutput::best_match(output).map(|(_, inner)| Self::ProjectsOutput(inner)),
            "propose_skills" => exact(output).map(Self::ProposeSkillsOutput),
            "ProposeGoal" => exact(output).map(Self::ProposeGoalOutput),
            "PushNotification" => exact(output).map(Self::PushNotificationOutput),
            "Read" => FileReadOutput::best_match(output).map(|(_, inner)| Self::FileReadOutput(inner)),
            "ReadMcpResourceDirTool" => exact(output).map(Self::ReadMcpResourceDirOutput),
            "ReadMcpResourceTool" => exact(output).map(Self::ReadMcpResourceOutput),
            "ReadNotifications" => exact(output).map(Self::ReadNotificationsOutput),
            "RefreshMcpTools" => exact(output).map(Self::RefreshMcpToolsOutput),
            "RemoteTrigger" => exact(output).map(Self::RemoteTriggerOutput),
            "ReportFindings" => exact(output).map(Self::ReportFindingsOutput),
            "ScheduleWakeup" => exact(output).map(Self::ScheduleWakeupOutput),
            "SendFeedback" => exact(output).map(Self::SendFeedbackOutput),
            "ShowOnboardingRolePicker" => exact(output).map(Self::ShowOnboardingRolePickerOutput),
            "TaskCreate" => exact(output).map(Self::TaskCreateOutput),
            "TaskGet" => exact(output).map(Self::TaskGetOutput),
            "TaskList" => exact(output).map(Self::TaskListOutput),
            "TaskStop" => exact(output).map(Self::TaskStopOutput),
            "TaskUpdate" => exact(output).map(Self::TaskUpdateOutput),
            "TodoWrite" => exact(output).map(Self::TodoWriteOutput),
            "WebFetch" => exact(output).map(Self::WebFetchOutput),
            "WebSearch" => exact(output).map(Self::WebSearchOutput),
            "Workflow" => exact(output).map(Self::WorkflowOutput),
            "Write" => exact(output).map(Self::FileWriteOutput),
            name if name.starts_with(MCP_TOOL_PREFIX) => {
                McpOutput::best_match(output).map(|(_, inner)| Self::McpOutput(inner))
            }
            _ => None,
        };
        decoded.unwrap_or_else(|| Self::Unknown(output.clone()))
    }
}

/// `value` decoded as `T`, only when encoding the result reproduces `value`.
pub(crate) fn exact<T: serde::Serialize + serde::de::DeserializeOwned>(value: &Value) -> Option<T> {
    let typed: T = serde_json::from_value(value.clone()).ok()?;
    serde_json::to_value(&typed)
        .is_ok_and(|round| round == *value)
        .then_some(typed)
}
