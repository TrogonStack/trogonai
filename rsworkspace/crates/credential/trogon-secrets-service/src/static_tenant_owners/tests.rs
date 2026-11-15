use std::path::Path;

use nkeys::KeyPair;
use trogon_credential_domain::CredentialOwnerIdError;
use trogon_std::fs::MemFs;

use super::*;
use crate::{TenantAccountError, TenantOwnersFileError};

const FILE: &str = "tenant-owners.toml";

fn tenant() -> TenantAccount {
    TenantAccount::parse(&KeyPair::new_account().public_key()).unwrap()
}

fn owner(value: &str) -> CredentialOwnerId {
    CredentialOwnerId::new(value).unwrap()
}

fn load(document: &str) -> Result<StaticTenantOwners, TenantOwnersFileError> {
    let fs = MemFs::new();
    fs.insert(FILE, document);
    StaticTenantOwners::load(Path::new(FILE), &fs)
}

#[test]
fn a_tenant_owners_file_that_cannot_be_read_is_refused() {
    let error = StaticTenantOwners::load(Path::new(FILE), &MemFs::new()).unwrap_err();

    assert!(matches!(error, TenantOwnersFileError::Unreadable { path, .. } if path == Path::new(FILE)));
}

#[test]
fn a_tenant_owners_file_that_does_not_parse_is_refused() {
    let error = load("[tenants]\n\"fake-account\" = = []\n").unwrap_err();

    let TenantOwnersFileError::Unparsable { path, syntax } = error else {
        panic!("a broken file was refused as {error:?}");
    };
    assert_eq!(path, Path::new(FILE));
    assert_eq!(syntax.line(), Some(2));
}

#[test]
fn a_tenant_owners_file_with_a_setting_it_does_not_define_is_refused() {
    let error = load(&format!("owners = []\n[tenants]\n\"{}\" = [\"project-1\"]\n", tenant())).unwrap_err();

    assert!(matches!(error, TenantOwnersFileError::Unparsable { .. }));
}

#[test]
fn a_tenant_owners_file_without_its_tenants_table_is_refused() {
    let error = load("").unwrap_err();

    assert!(matches!(error, TenantOwnersFileError::Unparsable { .. }));
}

#[test]
fn a_tenant_owners_file_that_names_no_tenants_is_refused() {
    let error = load("[tenants]\n").unwrap_err();

    assert!(matches!(error, TenantOwnersFileError::NoTenants { path } if path == Path::new(FILE)));
}

#[test]
fn a_tenant_that_is_not_an_account_key_is_refused() {
    let user = KeyPair::new_user().public_key();

    for (named, expected) in [
        ("fake-account", TenantAccountError::NotAPublicKey),
        (user.as_str(), TenantAccountError::NotAnAccount),
    ] {
        let error = load(&format!("[tenants]\n\"{named}\" = [\"project-1\"]\n")).unwrap_err();

        assert!(
            matches!(&error, TenantOwnersFileError::InvalidTenant { tenant, source, .. } if tenant == named && *source == expected),
            "{named} was refused as {error:?}"
        );
    }
}

#[test]
fn a_tenant_that_holds_no_owners_is_refused() {
    let known = tenant();

    let error = load(&format!("[tenants]\n\"{known}\" = []\n")).unwrap_err();

    assert!(matches!(error, TenantOwnersFileError::NoOwners { tenant, .. } if tenant == known));
}

#[test]
fn an_owner_that_is_not_an_owner_id_is_refused() {
    let known = tenant();

    let error = load(&format!("[tenants]\n\"{known}\" = [\"project 1\"]\n")).unwrap_err();

    assert!(matches!(
        error,
        TenantOwnersFileError::InvalidOwner { tenant, source: CredentialOwnerIdError::InvalidCharacter(' '), .. }
            if tenant == known
    ));
}

#[test]
fn an_owner_held_by_two_tenants_is_refused() {
    let mut tenants = [tenant(), tenant()];
    tenants.sort();
    let [low, high] = tenants;

    let error = load(&format!(
        "[tenants]\n\"{high}\" = [\"project-1\"]\n\"{low}\" = [\"project-2\", \"project-1\"]\n"
    ))
    .unwrap_err();

    assert!(matches!(
        error,
        TenantOwnersFileError::SharedOwner { owner: shared, first, second, .. }
            if shared == owner("project-1") && first == low && second == high
    ));
}

#[test]
fn each_tenant_holds_the_owners_its_file_names() {
    let (first, second) = (tenant(), tenant());

    let owners = load(&format!(
        "[tenants]\n\"{first}\" = [\"project-1\", \"project-2\"]\n\"{second}\" = [\"project-3\"]\n"
    ))
    .unwrap();

    assert_eq!(owners.holding(&first, &owner("project-1")), OwnerHolding::Held);
    assert_eq!(owners.holding(&first, &owner("project-2")), OwnerHolding::Held);
    assert_eq!(owners.holding(&first, &owner("project-3")), OwnerHolding::NotHeld);
    assert_eq!(owners.holding(&second, &owner("project-3")), OwnerHolding::Held);
    assert_eq!(
        owners.holding(&tenant(), &owner("project-1")),
        OwnerHolding::UnknownTenant
    );
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
