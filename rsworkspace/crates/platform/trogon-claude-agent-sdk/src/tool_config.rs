//! TypeScript's `toolConfig`. It reaches the CLI only through environment variables, so it has no argv or
//! initialize-request form. Python has no equivalent.

/// `toolConfig.askUserQuestion.previewFormat`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuestionPreviewFormat {
    Markdown,
    Html,
}

impl QuestionPreviewFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Html => "html",
        }
    }
}

/// `toolConfig.askUserQuestion`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AskUserQuestionConfig {
    /// Sets `CLAUDE_CODE_QUESTION_PREVIEW_FORMAT` when present.
    pub preview_format: Option<QuestionPreviewFormat>,
    /// Sets `CLAUDE_CODE_QUESTION_EXTENDED=1` when true.
    pub extended_questions: bool,
    /// Sets `CLAUDE_CODE_QUESTION_OPTIONAL_DESCRIPTIONS=1` when true.
    pub optional_descriptions: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ToolConfig {
    pub ask_user_question: AskUserQuestionConfig,
}

#[cfg(test)]
mod tests;
