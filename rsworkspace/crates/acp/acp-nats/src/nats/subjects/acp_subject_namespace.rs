use crate::acp_prefix::AcpPrefix;

#[derive(Debug)]
pub(crate) struct AcpSubjectNamespace(String);

impl AcpSubjectNamespace {
    pub(crate) fn new(prefix: &AcpPrefix) -> Self {
        Self(format!("{}.v1.", prefix.as_str()))
    }

    pub(crate) fn contains(&self, subject: &str) -> bool {
        subject.starts_with(&self.0)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}
