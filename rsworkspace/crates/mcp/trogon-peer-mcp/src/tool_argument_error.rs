use trogon_peer::PeerValueError;

use crate::argument_issue::ArgumentIssue;
use crate::model_tool_spec::ModelToolSpec;
use crate::recipient_list::RecipientList;

/// A rejected model tool call: which field is wrong, what the tool takes, and a call that works.
#[derive(Debug, thiserror::Error)]
#[error(
    "Invalid {} arguments: {issue} {} Example: {}{}",
    .spec.name,
    .spec.usage(),
    .spec.example,
    recipients_suffix(.recipients.as_ref())
)]
pub struct ToolArgumentError {
    spec: &'static ModelToolSpec,
    issue: Box<ArgumentIssue>,
    /// The typed value rejection behind the issue, when there is one.
    #[source]
    source: Option<PeerValueError>,
    recipients: Option<RecipientList>,
}

impl ToolArgumentError {
    pub(crate) fn new(spec: &'static ModelToolSpec, issue: ArgumentIssue) -> Self {
        Self {
            spec,
            source: issue.source().cloned(),
            issue: Box::new(issue),
            recipients: None,
        }
    }

    pub(crate) fn with_recipients(mut self, recipients: RecipientList) -> Self {
        self.recipients = Some(recipients);
        self
    }
}

fn recipients_suffix(recipients: Option<&RecipientList>) -> String {
    recipients.map_or_else(String::new, |recipients| format!(" {recipients}"))
}
