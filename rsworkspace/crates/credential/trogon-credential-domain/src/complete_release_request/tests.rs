use super::*;

#[test]
fn exposes_every_dimension_it_was_built_with() {
    let runtime_service = RuntimeServiceId::new("trogon-gateway").unwrap();
    let host = ReleaseHost::new("api.example.com").unwrap();
    let location = InjectionLocation::header("authorization").unwrap();

    let request = CompleteReleaseRequest::new(runtime_service.clone(), host.clone(), location.clone());

    assert_eq!(request.runtime_service(), &runtime_service);
    assert_eq!(request.host(), &host);
    assert_eq!(request.location(), &location);
}
