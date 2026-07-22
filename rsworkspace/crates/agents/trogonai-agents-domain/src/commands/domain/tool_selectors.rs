use std::collections::BTreeSet;

use super::nonblank::validate_nonblank;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolSelectors {
    required: BTreeSet<String>,
    optional: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolSelectorsError {
    #[error("required tool selector '{selector}' must be nonblank and trimmed")]
    InvalidRequired { selector: String },
    #[error("optional tool selector '{selector}' must be nonblank and trimmed")]
    InvalidOptional { selector: String },
    #[error("tool selector '{selector}' cannot be both required and optional")]
    Overlap { selector: String },
}

impl ToolSelectors {
    pub fn new(required: BTreeSet<String>, optional: BTreeSet<String>) -> Result<Self, ToolSelectorsError> {
        for selector in &required {
            if validate_nonblank(selector).is_err() {
                return Err(ToolSelectorsError::InvalidRequired {
                    selector: selector.clone(),
                });
            }
        }
        for selector in &optional {
            if validate_nonblank(selector).is_err() {
                return Err(ToolSelectorsError::InvalidOptional {
                    selector: selector.clone(),
                });
            }
        }
        if let Some(selector) = required.intersection(&optional).next() {
            return Err(ToolSelectorsError::Overlap {
                selector: selector.clone(),
            });
        }
        Ok(Self { required, optional })
    }

    pub fn required(&self) -> &BTreeSet<String> {
        &self.required
    }

    pub fn optional(&self) -> &BTreeSet<String> {
        &self.optional
    }
}

#[cfg(test)]
mod tests;
