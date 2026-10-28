use std::fmt;

use super::constants::{SUBSCRIPTION_ID_MAX_LEN, SUBSCRIPTION_ID_MIN_LEN, SUBSCRIPTION_ID_RESERVED_PREFIX};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PubSubSubscriptionNameError {
    #[error("must not be empty")]
    Empty,
    #[error("must match projects/<project>/subscriptions/<subscription>")]
    InvalidShape,
    #[error(
        "subscription id must be 3 to 255 characters, start with a letter, use only letters, digits, '-', '_', '.', '~', '+' or '%', and not start with 'goog'"
    )]
    InvalidSubscriptionId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PubSubSubscriptionName(String);

impl PubSubSubscriptionName {
    pub fn new(value: impl AsRef<str>) -> Result<Self, PubSubSubscriptionNameError> {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(PubSubSubscriptionNameError::Empty);
        }

        let segments: Vec<&str> = value.split('/').collect();
        let well_formed = matches!(
            segments.as_slice(),
            ["projects", project, "subscriptions", subscription]
                if is_name_segment(project) && !subscription.is_empty()
        );
        if !well_formed {
            return Err(PubSubSubscriptionNameError::InvalidShape);
        }
        if !is_subscription_id(segments[3]) {
            return Err(PubSubSubscriptionNameError::InvalidSubscriptionId);
        }

        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_name_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.chars().all(|c| !c.is_whitespace() && !c.is_control())
}

fn is_subscription_id(id: &str) -> bool {
    (SUBSCRIPTION_ID_MIN_LEN..=SUBSCRIPTION_ID_MAX_LEN).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_alphabetic())
        && !id.starts_with(SUBSCRIPTION_ID_RESERVED_PREFIX)
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~' | '+' | '%'))
}

impl fmt::Display for PubSubSubscriptionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
