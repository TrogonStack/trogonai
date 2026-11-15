use nkeys::KeyPair;

use super::*;

fn tenant() -> TenantAccount {
    TenantAccount::parse(&KeyPair::new_account().public_key()).unwrap()
}

fn owner(value: &str) -> CredentialOwnerId {
    CredentialOwnerId::new(value).unwrap()
}

#[test]
fn an_unknown_tenant_holds_nothing() {
    let owners = StaticTenantOwners::new().with_owner(tenant(), owner("project-1"));

    assert_eq!(
        owners.holding(&tenant(), &owner("project-1")),
        OwnerHolding::UnknownTenant
    );
}

#[test]
fn a_known_tenant_does_not_hold_another_tenants_owner() {
    let known = tenant();
    let owners = StaticTenantOwners::new()
        .with_owner(known.clone(), owner("project-1"))
        .with_owner(tenant(), owner("project-2"));

    assert_eq!(owners.holding(&known, &owner("project-2")), OwnerHolding::NotHeld);
}

#[test]
fn a_tenant_holds_each_owner_it_was_given() {
    let known = tenant();
    let owners = StaticTenantOwners::new()
        .with_owner(known.clone(), owner("project-1"))
        .with_owner(known.clone(), owner("project-2"));

    assert_eq!(owners.holding(&known, &owner("project-1")), OwnerHolding::Held);
    assert_eq!(owners.holding(&known, &owner("project-2")), OwnerHolding::Held);
}
