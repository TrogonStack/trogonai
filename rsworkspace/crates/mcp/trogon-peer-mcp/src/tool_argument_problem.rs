use std::fmt;

use crate::argument_issue::ArgumentIssue;
use crate::model_tool_spec::ModelToolSpec;
use crate::recipient_list::RecipientList;

/// A rejected model tool call: which field is wrong, what the tool takes, and a call that works.
#[derive(Debug)]
pub struct ToolArgumentProblem(Box<Problem>);

#[derive(Debug)]
struct Problem {
    spec: &'static ModelToolSpec,
    issue: ArgumentIssue,
    recipients: Option<RecipientList>,
}

impl ToolArgumentProblem {
    pub(crate) fn new(spec: &'static ModelToolSpec, issue: ArgumentIssue) -> Self {
        Self(Box::new(Problem {
            spec,
            issue,
            recipients: None,
        }))
    }

    pub(crate) fn with_recipients(mut self, recipients: RecipientList) -> Self {
        self.0.recipients = Some(recipients);
        self
    }
}

impl fmt::Display for ToolArgumentProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Problem {
            spec,
            issue,
            recipients,
        } = &*self.0;
        write!(formatter, "Invalid {} arguments: {issue} ", spec.name)?;
        spec.write_usage(formatter)?;
        write!(formatter, " Example: {}", spec.example)?;
        if let Some(recipients) = recipients {
            write!(formatter, " {recipients}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ToolArgumentProblem {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.issue.source().map(|source| source as _)
    }
}
