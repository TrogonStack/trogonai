use std::collections::HashSet;

use trogon_sandbox_contract::TenantId;
use trogon_sandbox_protocol::{
    legacy_tenant_kv_key, legacy_tenant_scoped_kv_key, legacy_tenant_scoped_kv_prefix, parse_tenant_scoped_kv_key,
    parse_tenant_scoped_kv_key_any, tenant_kv_key, tenant_scoped_kv_key, tenant_scoped_kv_prefix,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn tenant(value: &str) -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new(value)?)
}

#[test]
fn any_parser_recovers_tenant_and_name_without_knowing_the_tenant() -> TestResult {
    for (t, n) in [("acme", "x_y"), ("a.b", "p/q"), ("acme_x", "y"), ("t-1", "n")] {
        let key = tenant_scoped_kv_key(&tenant(t)?, n);
        let (got_t, got_n) = parse_tenant_scoped_kv_key_any(&key).ok_or("parses")?;
        assert_eq!((got_t.as_str(), got_n.as_str()), (t, n));
    }
    assert_eq!(parse_tenant_scoped_kv_key_any("noseparator"), None);
    assert_eq!(parse_tenant_scoped_kv_key_any("_name"), None);
    assert_eq!(parse_tenant_scoped_kv_key_any("a.b_c"), None);
    Ok(())
}

#[test]
fn any_parser_rejects_a_tenant_part_that_is_not_a_valid_tenant() {
    // `=00` decodes to a control character, which no tenant id may contain.
    assert_eq!(parse_tenant_scoped_kv_key_any("a=00b_name"), None);
    // A name part that is not canonical.
    assert_eq!(parse_tenant_scoped_kv_key_any("acme_=2e"), None);
    // A tenant escape that is not hexadecimal.
    assert_eq!(parse_tenant_scoped_kv_key_any("a=zzb_name"), None);
}

#[test]
fn tenant_only_keys_never_collide() -> TestResult {
    let tenants = ["a.b", "a_b", "a:b", "a/b", "a=2Eb", "ab"];
    let keys: HashSet<String> = tenants
        .iter()
        .map(|t| Ok(tenant_kv_key(&tenant(t)?)))
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;
    assert_eq!(keys.len(), tenants.len());
    assert_eq!(
        legacy_tenant_kv_key(&tenant("a.b")?),
        legacy_tenant_kv_key(&tenant("a_b")?)
    );
    assert_eq!(tenant_kv_key(&tenant("acme-1")?), "acme-1");
    assert_eq!(tenant_kv_key(&tenant("a.b")?), "a=2Eb");
    Ok(())
}

#[test]
fn tenant_underscore_and_name_underscore_never_collide() -> TestResult {
    let a = tenant_scoped_kv_key(&tenant("acme")?, "x_y");
    let b = tenant_scoped_kv_key(&tenant("acme_x")?, "y");
    assert_ne!(a, b);
    assert_eq!(
        legacy_tenant_scoped_kv_key(&tenant("acme")?, "x_y"),
        legacy_tenant_scoped_kv_key(&tenant("acme_x")?, "y"),
        "the legacy layout is the collision this module removes"
    );
    Ok(())
}

#[test]
fn folded_characters_stay_distinct() -> TestResult {
    let pairs = [
        ("acme", "v1.2"),
        ("acme", "v1_2"),
        ("acme", "v1:2"),
        ("acme", "v1/2"),
        ("acme.x", "y"),
        ("acme:x", "y"),
        ("acme/x", "y"),
        ("acme_x", "y"),
        ("acme", "x_y"),
        ("acme", "x.y"),
        ("acme=5Fx", "y"),
        ("acme", "=5F"),
        ("acme", "_"),
    ];
    let mut keys = HashSet::new();
    for (t, n) in pairs {
        assert!(
            keys.insert(tenant_scoped_kv_key(&tenant(t)?, n)),
            "collision for ({t}, {n})"
        );
    }
    Ok(())
}

#[test]
fn plain_pairs_keep_their_legacy_key() -> TestResult {
    for (t, n) in [("acme", "prod"), ("team-1", "x_y-z"), ("A9", "Tag_01")] {
        assert_eq!(
            tenant_scoped_kv_key(&tenant(t)?, n),
            legacy_tenant_scoped_kv_key(&tenant(t)?, n)
        );
    }
    Ok(())
}

#[test]
fn legacy_prefix_is_shared_across_tenants_but_the_current_prefix_is_not() -> TestResult {
    let owner = tenant("acme")?;
    let other = tenant("acme_team")?;
    assert_eq!(legacy_tenant_scoped_kv_prefix(&owner), "acme_");
    assert_eq!(tenant_scoped_kv_prefix(&owner), "acme_");
    assert!(legacy_tenant_scoped_kv_key(&other, "prod").starts_with(&legacy_tenant_scoped_kv_prefix(&owner)));
    assert!(!tenant_scoped_kv_key(&other, "prod").starts_with(&tenant_scoped_kv_prefix(&owner)));
    assert_eq!(legacy_tenant_scoped_kv_prefix(&tenant("a.b:c/d")?), "a_b_c_d_");
    Ok(())
}

#[test]
fn keys_are_valid_nats_kv_keys() -> TestResult {
    let key = tenant_scoped_kv_key(&tenant("org:acme/eu.1 \u{fc}")?, ".hidden name=1");
    assert!(
        key.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'='))
    );
    Ok(())
}

#[test]
fn parse_round_trips_and_rejects_other_tenants() -> TestResult {
    let owner = tenant("acme")?;
    for name in ["prod", "x_y", "v1.2", "a/b:c", "=", "\u{fc}"] {
        let key = tenant_scoped_kv_key(&owner, name);
        assert_eq!(parse_tenant_scoped_kv_key(&key, &owner).as_deref(), Some(name));
    }
    let foreign = tenant_scoped_kv_key(&tenant("acme_team")?, "prod");
    assert_eq!(parse_tenant_scoped_kv_key(&foreign, &owner), None);
    let foreign = tenant_scoped_kv_key(&tenant("acme-team")?, "prod");
    assert_eq!(parse_tenant_scoped_kv_key(&foreign, &owner), None);
    Ok(())
}

#[test]
fn parse_rejects_non_canonical_escapes() -> TestResult {
    let owner = tenant("acme")?;
    assert_eq!(parse_tenant_scoped_kv_key("acme_=61", &owner), None);
    assert_eq!(parse_tenant_scoped_kv_key("acme_=2e", &owner), None);
    assert_eq!(parse_tenant_scoped_kv_key("acme_=2", &owner), None);
    assert_eq!(parse_tenant_scoped_kv_key("acme_a.b", &owner), None);
    // Escapes that decode to bytes that are not valid UTF-8.
    assert_eq!(parse_tenant_scoped_kv_key("acme_=FF", &owner), None);
    Ok(())
}
