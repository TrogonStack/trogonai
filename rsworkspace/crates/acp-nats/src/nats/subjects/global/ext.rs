/// Core NATS request/reply.
#[derive(Debug)]
pub struct ExtSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    method: crate::ext_method_name::ExtMethodName,
}

impl ExtSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix, method: &crate::ext_method_name::ExtMethodName) -> Self {
        Self {
            prefix: prefix.clone(),
            method: method.clone(),
        }
    }
}

impl std::fmt::Display for ExtSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.ext.{}", self.prefix.as_str(), self.method)
    }
}

impl super::super::markers::Requestable for ExtSubject {}

impl super::super::stream::StreamAssignment for ExtSubject {
    const STREAM: Option<super::super::stream::AcpStream> = Some(super::super::stream::AcpStream::GlobalExt);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> crate::acp_prefix::AcpPrefix {
        crate::acp_prefix::AcpPrefix::new("acp").expect("prefix")
    }
    fn method() -> crate::ext_method_name::ExtMethodName {
        crate::ext_method_name::ExtMethodName::new("my_op").expect("method")
    }

    #[test]
    fn display_formats_subject_correctly() {
        let s = ExtSubject::new(&prefix(), &method());
        assert_eq!(s.to_string(), "acp.agent.ext.my_op");
    }
}
