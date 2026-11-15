use std::borrow::Cow;

use crate::ToolName;

/// A tool name for [`crate::ToolSet::Only`] and the typed tool views. Any name is accepted as written; the
/// associated constants are the names the pinned CLI (2.1.323) gives its built-in tools.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BuiltinToolName(Cow<'static, str>);

impl BuiltinToolName {
    pub const AGENT: Self = Self(Cow::Borrowed("Agent"));
    pub const ARTIFACT: Self = Self(Cow::Borrowed("Artifact"));
    pub const ASK_USER_QUESTION: Self = Self(Cow::Borrowed("AskUserQuestion"));
    pub const BASH: Self = Self(Cow::Borrowed("Bash"));
    pub const CLAUDE_DESIGN: Self = Self(Cow::Borrowed("ClaudeDesign"));
    pub const CRON_CREATE: Self = Self(Cow::Borrowed("CronCreate"));
    pub const CRON_DELETE: Self = Self(Cow::Borrowed("CronDelete"));
    pub const CRON_LIST: Self = Self(Cow::Borrowed("CronList"));
    pub const EDIT: Self = Self(Cow::Borrowed("Edit"));
    pub const END_CONVERSATION: Self = Self(Cow::Borrowed("EndConversation"));
    pub const ENTER_PLAN_MODE: Self = Self(Cow::Borrowed("EnterPlanMode"));
    pub const ENTER_WORKTREE: Self = Self(Cow::Borrowed("EnterWorktree"));
    pub const EXIT_PLAN_MODE: Self = Self(Cow::Borrowed("ExitPlanMode"));
    pub const EXIT_WORKTREE: Self = Self(Cow::Borrowed("ExitWorktree"));
    pub const GLOB: Self = Self(Cow::Borrowed("Glob"));
    pub const GREP: Self = Self(Cow::Borrowed("Grep"));
    pub const LIST_AGENTS: Self = Self(Cow::Borrowed("ListAgents"));
    pub const LIST_MCP_RESOURCES_TOOL: Self = Self(Cow::Borrowed("ListMcpResourcesTool"));
    pub const LSP: Self = Self(Cow::Borrowed("LSP"));
    pub const MONITOR: Self = Self(Cow::Borrowed("Monitor"));
    pub const NOTEBOOK_EDIT: Self = Self(Cow::Borrowed("NotebookEdit"));
    pub const OFFER_CHROME_SETUP: Self = Self(Cow::Borrowed("OfferChromeSetup"));
    pub const POWER_SHELL: Self = Self(Cow::Borrowed("PowerShell"));
    pub const PROJECTS: Self = Self(Cow::Borrowed("Projects"));
    pub const PROPOSE_SKILLS: Self = Self(Cow::Borrowed("propose_skills"));
    pub const PROPOSE_GOAL: Self = Self(Cow::Borrowed("ProposeGoal"));
    pub const PUSH_NOTIFICATION: Self = Self(Cow::Borrowed("PushNotification"));
    pub const READ: Self = Self(Cow::Borrowed("Read"));
    pub const READ_MCP_RESOURCE_DIR_TOOL: Self = Self(Cow::Borrowed("ReadMcpResourceDirTool"));
    pub const READ_MCP_RESOURCE_TOOL: Self = Self(Cow::Borrowed("ReadMcpResourceTool"));
    pub const READ_NOTIFICATIONS: Self = Self(Cow::Borrowed("ReadNotifications"));
    pub const REMOTE_TRIGGER: Self = Self(Cow::Borrowed("RemoteTrigger"));
    pub const REPORT_FINDINGS: Self = Self(Cow::Borrowed("ReportFindings"));
    pub const SCHEDULE_WAKEUP: Self = Self(Cow::Borrowed("ScheduleWakeup"));
    pub const SEND_FEEDBACK: Self = Self(Cow::Borrowed("SendFeedback"));
    pub const SEND_MESSAGE: Self = Self(Cow::Borrowed("SendMessage"));
    pub const SEND_USER_FILE: Self = Self(Cow::Borrowed("SendUserFile"));
    pub const SEND_USER_MESSAGE: Self = Self(Cow::Borrowed("SendUserMessage"));
    pub const SHARE_ONBOARDING_GUIDE: Self = Self(Cow::Borrowed("ShareOnboardingGuide"));
    pub const SHOW_ONBOARDING_ROLE_PICKER: Self = Self(Cow::Borrowed("ShowOnboardingRolePicker"));
    pub const SKILL: Self = Self(Cow::Borrowed("Skill"));
    pub const SUBAGENT_HANDBACK: Self = Self(Cow::Borrowed("SubagentHandback"));
    pub const TASK_CREATE: Self = Self(Cow::Borrowed("TaskCreate"));
    pub const TASK_GET: Self = Self(Cow::Borrowed("TaskGet"));
    pub const TASK_LIST: Self = Self(Cow::Borrowed("TaskList"));
    pub const TASK_STOP: Self = Self(Cow::Borrowed("TaskStop"));
    pub const TASK_UPDATE: Self = Self(Cow::Borrowed("TaskUpdate"));
    pub const TODO_WRITE: Self = Self(Cow::Borrowed("TodoWrite"));
    pub const TOOL_SEARCH: Self = Self(Cow::Borrowed("ToolSearch"));
    pub const WAIT_FOR_MCP_SERVERS: Self = Self(Cow::Borrowed("WaitForMcpServers"));
    pub const WEB_FETCH: Self = Self(Cow::Borrowed("WebFetch"));
    pub const WEB_SEARCH: Self = Self(Cow::Borrowed("WebSearch"));
    pub const WORKFLOW: Self = Self(Cow::Borrowed("Workflow"));
    pub const WRITE: Self = Self(Cow::Borrowed("Write"));
    /// The tool `sdk-tools` declares `RefreshMcpToolsInput` for; the CLI names it outside [`Self::ALL`].
    pub const REFRESH_MCP_TOOLS: Self = Self(Cow::Borrowed("RefreshMcpTools"));

    /// Every built-in tool name, in the order of TypeScript `BUILTIN_TOOL_NAMES` and the pinned CLI.
    pub const ALL: [Self; 54] = [
        Self::AGENT,
        Self::ARTIFACT,
        Self::ASK_USER_QUESTION,
        Self::BASH,
        Self::CLAUDE_DESIGN,
        Self::CRON_CREATE,
        Self::CRON_DELETE,
        Self::CRON_LIST,
        Self::EDIT,
        Self::END_CONVERSATION,
        Self::ENTER_PLAN_MODE,
        Self::ENTER_WORKTREE,
        Self::EXIT_PLAN_MODE,
        Self::EXIT_WORKTREE,
        Self::GLOB,
        Self::GREP,
        Self::LIST_AGENTS,
        Self::LIST_MCP_RESOURCES_TOOL,
        Self::LSP,
        Self::MONITOR,
        Self::NOTEBOOK_EDIT,
        Self::OFFER_CHROME_SETUP,
        Self::POWER_SHELL,
        Self::PROJECTS,
        Self::PROPOSE_SKILLS,
        Self::PROPOSE_GOAL,
        Self::PUSH_NOTIFICATION,
        Self::READ,
        Self::READ_MCP_RESOURCE_DIR_TOOL,
        Self::READ_MCP_RESOURCE_TOOL,
        Self::READ_NOTIFICATIONS,
        Self::REMOTE_TRIGGER,
        Self::REPORT_FINDINGS,
        Self::SCHEDULE_WAKEUP,
        Self::SEND_FEEDBACK,
        Self::SEND_MESSAGE,
        Self::SEND_USER_FILE,
        Self::SEND_USER_MESSAGE,
        Self::SHARE_ONBOARDING_GUIDE,
        Self::SHOW_ONBOARDING_ROLE_PICKER,
        Self::SKILL,
        Self::SUBAGENT_HANDBACK,
        Self::TASK_CREATE,
        Self::TASK_GET,
        Self::TASK_LIST,
        Self::TASK_STOP,
        Self::TASK_UPDATE,
        Self::TODO_WRITE,
        Self::TOOL_SEARCH,
        Self::WAIT_FOR_MCP_SERVERS,
        Self::WEB_FETCH,
        Self::WEB_SEARCH,
        Self::WORKFLOW,
        Self::WRITE,
    ];

    /// The legacy names the pinned CLI still accepts, each with the built-in name it resolves to.
    pub const LEGACY_ALIASES: [(&'static str, &'static str); 8] = [
        ("Task", "Agent"),
        ("KillShell", "TaskStop"),
        ("KillBash", "TaskStop"),
        ("ListPeers", "ListAgents"),
        ("Brief", "SendUserMessage"),
        ("ListMcpResources", "ListMcpResourcesTool"),
        ("ReadMcpResource", "ReadMcpResourceTool"),
        ("ReadMcpResourceDir", "ReadMcpResourceDirTool"),
    ];

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The built-in name `name` resolves to the way the pinned CLI resolves a legacy alias, or `name` itself.
    pub fn canonical_name(name: &str) -> &str {
        Self::LEGACY_ALIASES
            .iter()
            .find(|(alias, _)| *alias == name)
            .map_or(name, |(_, target)| *target)
    }
}

impl From<String> for BuiltinToolName {
    fn from(value: String) -> Self {
        Self(Cow::Owned(value))
    }
}

impl From<&str> for BuiltinToolName {
    fn from(value: &str) -> Self {
        Self(Cow::Owned(value.to_owned()))
    }
}

impl From<ToolName> for BuiltinToolName {
    fn from(value: ToolName) -> Self {
        Self::from(value.as_str())
    }
}
