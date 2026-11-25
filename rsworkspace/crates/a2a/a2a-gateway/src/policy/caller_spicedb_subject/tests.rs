use super::*;

fn subject_for(kind: &str, id: &str) -> CallerSpiceDbSubject {
    CallerSpiceDbSubject::from(&SubjectPrincipal::from_tokens(kind, id).expect("principal"))
}

#[test]
fn each_principal_kind_maps_to_its_spicedb_object_type() {
    for (kind, id, object_type) in [
        ("person", "alice", "user"),
        ("agent", "planner", "agent"),
        ("service", "scheduler", "service"),
    ] {
        let subject = subject_for(kind, id);

        assert_eq!(subject.object_type(), object_type);
        assert_eq!(subject.object_id(), id);
        assert_eq!(subject.to_string(), format!("{object_type}/{id}"));
        assert_eq!(subject.to_spicedb_subject().as_str(), format!("{object_type}/{id}"));
    }
}

#[test]
fn principals_of_different_kinds_never_share_a_spicedb_subject() {
    assert_ne!(subject_for("person", "alice"), subject_for("agent", "alice"));
    assert_ne!(
        subject_for("person", "alice").to_string(),
        subject_for("agent", "alice").to_string()
    );
}
