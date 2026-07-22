use std::collections::BTreeSet;

use super::nonblank::validate_nonblank;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DelegateSelectors {
    required: BTreeSet<String>,
    optional: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DelegateSelectorsError {
    #[error("required delegate selector '{selector}' must be nonblank and trimmed")]
    InvalidRequired { selector: String },
    #[error("optional delegate selector '{selector}' must be nonblank and trimmed")]
    InvalidOptional { selector: String },
    #[error("delegate selector '{selector}' cannot be both required and optional")]
    Overlap { selector: String },
}

impl DelegateSelectors {
    pub fn new(required: BTreeSet<String>, optional: BTreeSet<String>) -> Result<Self, DelegateSelectorsError> {
        for selector in &required {
            if validate_nonblank(selector).is_err() {
                return Err(DelegateSelectorsError::InvalidRequired {
                    selector: selector.clone(),
                });
            }
        }
        for selector in &optional {
            if validate_nonblank(selector).is_err() {
                return Err(DelegateSelectorsError::InvalidOptional {
                    selector: selector.clone(),
                });
            }
        }
        if let Some(selector) = required.intersection(&optional).next() {
            return Err(DelegateSelectorsError::Overlap {
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
