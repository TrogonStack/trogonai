use super::{TenantPosition, TenantPositionError};

#[test]
fn a_host_without_a_configured_position_reads_no_tenant_token() {
    assert_eq!(
        TenantPosition::default(),
        TenantPosition::Absent,
        "a host inside a tenant's own account has no tenant token to read, and assuming one would shift every principal token by one"
    );
}

#[test]
fn every_position_parses_back_from_the_name_it_displays() {
    for position in [TenantPosition::Absent, TenantPosition::Wildcard] {
        assert_eq!(position.to_string().parse::<TenantPosition>(), Ok(position));
    }
}

#[test]
fn a_name_that_is_no_position_is_refused() {
    assert_eq!(
        "tenant".parse::<TenantPosition>(),
        Err(TenantPositionError {
            value: "tenant".to_owned()
        }),
        "a typo here would otherwise subscribe at the wrong token and route every principal to the wrong position"
    );
}
