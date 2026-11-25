use super::*;
use crate::fixtures::{name, token, writer_id};

#[test]
fn names_the_writer_its_component_and_its_scope() {
    let identity = WriterIdentity::new(writer_id(), token("egress_proxy"), name("tenant_1"), token("project_1"));
    assert_eq!(identity.writer(), &writer_id());
    assert_eq!(identity.component().as_str(), "egress_proxy");
    assert_eq!(identity.tenant(), &name("tenant_1"));
    assert_eq!(identity.project().as_str(), "project_1");
}

#[test]
fn scopes_the_writers_own_records_to_no_actor() {
    let identity = WriterIdentity::new(writer_id(), token("egress_proxy"), name("tenant_1"), token("project_1"));
    let scope = identity.scope();
    assert_eq!(
        scope,
        RecordScope::new(name("tenant_1"), token("project_1"), Actor::Unattributed)
    );
}
