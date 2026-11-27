use std::str::FromStr;

use reqwest::header::HeaderValue;

use crate::DaytonaOrganizationIdError;

/// The Daytona organization a request is made for, sent in the `X-Daytona-Organization-ID` header.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DaytonaOrganizationId {
    text: String,
    header: HeaderValue,
}

impl DaytonaOrganizationId {
    pub fn new(id: &str) -> Result<Self, DaytonaOrganizationIdError> {
        if id.is_empty() {
            return Err(DaytonaOrganizationIdError::Empty);
        }
        let header = HeaderValue::from_str(id).map_err(|_| DaytonaOrganizationIdError::NotAHeader)?;
        Ok(Self {
            text: id.to_owned(),
            header,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn header(&self) -> HeaderValue {
        self.header.clone()
    }
}

impl FromStr for DaytonaOrganizationId {
    type Err = DaytonaOrganizationIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}
