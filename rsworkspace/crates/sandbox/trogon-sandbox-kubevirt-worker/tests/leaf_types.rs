use std::collections::BTreeMap;
use std::error::Error;

use trogon_sandbox_contract::{Digest, WorkerId};
use trogon_sandbox_kubevirt_worker::{
    BoundedName, BoundedNameError, CustodyToken, LeaseResources, ResourceReceipt, WarmCandidate, WarmClaimIntent,
    WarmPoolConfig, resource_name,
};

type TestResult = Result<(), Box<dyn Error>>;

fn receipt(name: &str, uid: &str) -> ResourceReceipt {
    ResourceReceipt {
        namespace: "ns".into(),
        name: name.into(),
        uid: uid.into(),
    }
}

fn candidate() -> Result<WarmCandidate, Box<dyn Error>> {
    Ok(WarmCandidate {
        vm: receipt("vm", "vm-uid"),
        data_volume: receipt("dv", "dv-uid"),
        resource_version: "7".into(),
        digest: Digest::new(format!("sha256:{}", "a".repeat(64)))?,
    })
}

#[test]
fn resource_names_are_dns_labels() {
    let name = resource_name("vm", "LEASE_01JABC");
    assert_eq!(name, "vm-lease-01jabc");
    assert_eq!(resource_name("vm", "---"), "vm-lease");
    assert_eq!(resource_name("vm", &"x".repeat(100)).len(), 63);
}

#[test]
fn bounded_names_enforce_their_grammar() -> TestResult {
    assert_eq!(BoundedName::new("a-b_c.d")?.as_str(), "a-b_c.d");
    assert_eq!(BoundedName::new("a")?.to_string(), "a");
    assert_eq!(BoundedName::new(""), Err(BoundedNameError::Empty));
    assert_eq!(BoundedName::new("a/b"), Err(BoundedNameError::InvalidCharacter));
    assert_eq!(
        BoundedName::new("a".repeat(254)),
        Err(BoundedNameError::TooLong { max: 253 })
    );
    assert_eq!(String::from(BoundedName::try_from("ok".to_owned())?), "ok");
    assert!(serde_json::from_str::<BoundedName>("\"bad name\"").is_err());
    Ok(())
}

#[test]
fn custody_tokens_round_trip() -> TestResult {
    let token = CustodyToken::generate();
    assert_ne!(token, CustodyToken::generate());
    assert_eq!(token.to_string().parse::<CustodyToken>()?, token);
    assert_eq!(CustodyToken::try_from(String::from(token))?, token);
    assert!(CustodyToken::new("nope").is_err());
    assert!(serde_json::from_str::<CustodyToken>("\"nope\"").is_err());
    Ok(())
}

#[test]
fn warm_pool_fits_only_when_enabled_and_small_enough() {
    let config = WarmPoolConfig::default();
    assert!(config.fits(512));
    assert!(!config.fits(513));
    let off = WarmPoolConfig {
        enabled: false,
        ..config
    };
    assert!(!off.fits(1));
}

#[test]
fn authority_keys_name_the_exact_resources() -> TestResult {
    assert!(LeaseResources::default().is_empty());
    assert_eq!(LeaseResources::default().authority_key(), None);
    let owned = LeaseResources::claiming(&candidate()?);
    assert!(!owned.is_empty());
    assert_eq!(owned.authority_key().as_deref(), Some("ns/vm/vm-uid/dv-uid"));
    let claim = LeaseResources {
        pending_claim: Some(WarmClaimIntent {
            candidate: candidate()?,
            token: CustodyToken::generate(),
            labels: BTreeMap::new(),
        }),
        ..LeaseResources::default()
    };
    assert_eq!(claim.authority_key().as_deref(), Some("ns/vm/vm-uid/dv-uid"));
    let volume_only = LeaseResources {
        data_volume: Some(receipt("dv", "dv-uid")),
        ..LeaseResources::default()
    };
    assert_eq!(volume_only.authority_key().as_deref(), Some("ns/dv/none/dv-uid"));
    let _ = WorkerId::new("w")?;
    Ok(())
}
