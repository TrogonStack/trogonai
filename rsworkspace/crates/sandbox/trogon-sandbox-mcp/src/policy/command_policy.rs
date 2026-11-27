use regex::RegexSet;

use super::CommandVetoError;

/// Regex policy over the argv of a command, matched against the arguments joined by single spaces. This is a
/// guard rail for a cooperative agent, not a sandbox boundary: it cannot see through a script the command goes on
/// to run.
#[derive(Debug, Clone, Default)]
pub struct CommandPolicy {
    deny: Option<(RegexSet, Vec<String>)>,
    allow: Option<RegexSet>,
}

impl CommandPolicy {
    pub fn new(deny: &[String], allow: &[String]) -> Result<Self, regex::Error> {
        let deny = if deny.is_empty() {
            None
        } else {
            Some((RegexSet::new(deny)?, deny.to_vec()))
        };
        let allow = if allow.is_empty() {
            None
        } else {
            Some(RegexSet::new(allow)?)
        };
        Ok(Self { deny, allow })
    }

    pub fn is_empty(&self) -> bool {
        self.deny.is_none() && self.allow.is_none()
    }

    pub fn check(&self, argv: &[String]) -> Result<(), CommandVetoError> {
        let line = argv.join(" ");
        if let Some((set, patterns)) = &self.deny
            && let Some(index) = set.matches(&line).iter().next()
        {
            return Err(CommandVetoError::Denied(patterns[index].clone()));
        }
        if let Some(set) = &self.allow
            && !set.is_match(&line)
        {
            return Err(CommandVetoError::NotAllowed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
