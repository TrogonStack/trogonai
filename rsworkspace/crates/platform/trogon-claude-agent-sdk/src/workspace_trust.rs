//! `workspaceTrust` and the other initialize-only helpers of `sdk.mjs` 0.3.323.

/// `workspaceTrust`: the host's answer to the CLI's workspace trust dialog.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceTrust {
    pub accepted: Option<bool>,
    /// The directory the trust applies to, as it exists where Claude Code runs.
    pub directory: Option<String>,
}

/// Node `path.win32.isAbsolute`: a leading separator, or a drive letter followed by `:` and a separator.
fn windows_absolute(path: &str) -> bool {
    let mut chars = path.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some('/' | '\\'), _, _) => true,
        (Some(drive), Some(':'), Some('/' | '\\')) => drive.is_ascii_alphabetic(),
        _ => false,
    }
}

fn absolute_anywhere(path: &str) -> bool {
    path.starts_with('/') || windows_absolute(path)
}

impl WorkspaceTrust {
    pub(crate) fn to_object(&self) -> crate::JsonObject {
        let mut object = crate::JsonObject::new();
        if let Some(accepted) = self.accepted {
            object.insert("accepted".into(), accepted.into());
        }
        if let Some(directory) = &self.directory {
            object.insert("directory".into(), directory.clone().into());
        }
        object
    }

    /// `R7`: an accepted trust needs a directory that is absolute on either platform, otherwise upstream drops the
    /// whole value and logs a warning.
    pub(crate) fn validated(&self) -> Option<&Self> {
        let unusable = self.accepted == Some(true) && !self.directory.as_deref().is_some_and(absolute_anywhere);
        if unusable {
            tracing::warn!(
                "workspaceTrust ignored: accepted requires an absolute directory, as it exists where Claude Code runs"
            );
            None
        } else {
            Some(self)
        }
    }
}

#[cfg(test)]
#[path = "workspace_trust_tests.rs"]
mod tests;
