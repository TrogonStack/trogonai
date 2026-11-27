use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn digest(fill: &str) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(format!("sha256:{}", fill.repeat(64)))?)
}

fn retained() -> Result<NativeCustody, Box<dyn std::error::Error>> {
    Ok(NativeCustody {
        allocation: KnownAllocation {
            container_id: "a".repeat(64),
            labels: BTreeMap::new(),
            memory_mib: 256,
            cpu_bits: 1.0_f32.to_bits(),
            snapshot: Some(digest("b")?),
        },
        birth: Some(Birth {
            image: format!("sha256:{}", "c".repeat(64)),
            init_pid: 42,
            started_at: "native-birth".into(),
        }),
        custody: Custody::Pooled {
            token: "one-use-pool-claim".into(),
        },
    })
}

fn claim(retained: &NativeCustody) -> Result<WarmContainer, Box<dyn std::error::Error>> {
    let digest = retained
        .allocation
        .snapshot
        .clone()
        .ok_or("retained allocation has no snapshot")?;
    let warm = WarmContainer::new(
        retained.allocation.container_id.clone(),
        digest.clone(),
        "snapshot-image".into(),
    );
    Ok(warm.with_claim(PoolClaim {
        container_id: retained.allocation.container_id.clone(),
        digest,
        image_ref: "snapshot-image".into(),
        token: "one-use-pool-claim".into(),
    }))
}

#[test]
fn raw_and_changed_pool_claims_cannot_mint_native_authority() -> TestResult {
    let retained = retained()?;
    let valid = claim(&retained)?;
    assert!(retained.accepts_claim(&valid, 256, 1.0));
    assert!(!retained.accepts_claim(
        &WarmContainer::new(
            valid.container_id.clone(),
            valid.digest.clone(),
            valid.image_ref.clone()
        ),
        256,
        1.0
    ));
    let mut wrong = valid.clone();
    wrong.container_id = "d".repeat(64);
    assert!(!retained.accepts_claim(&wrong, 256, 1.0));
    let mut wrong = valid.clone();
    wrong.digest = digest("e")?;
    assert!(!retained.accepts_claim(&wrong, 256, 1.0));
    let mut wrong = valid.clone();
    wrong.image_ref = "another-image".into();
    assert!(!retained.accepts_claim(&wrong, 256, 1.0));
    assert!(!retained.accepts_claim(&valid, 512, 1.0));
    assert!(!retained.accepts_claim(&valid, 256, 2.0));
    Ok(())
}

#[test]
fn consumed_or_unverified_birth_proof_cannot_replay_a_pool_claim() -> TestResult {
    let mut retained = retained()?;
    let valid = claim(&retained)?;
    retained.custody = Custody::Held {
        lease: LeaseId::new("new-owner"),
        registration: "new-registration".into(),
    };
    assert!(!retained.accepts_claim(&valid, 256, 1.0));
    retained.custody = Custody::Pooled {
        token: "another-generation".into(),
    };
    assert!(!retained.accepts_claim(&valid, 256, 1.0));
    retained.custody = Custody::Pooled {
        token: "one-use-pool-claim".into(),
    };
    retained.birth = None;
    assert!(!retained.accepts_claim(&valid, 256, 1.0));
    Ok(())
}

#[test]
fn a_known_creation_still_requires_its_exact_native_birth() -> TestResult {
    let mut retained = retained()?;
    retained.allocation.labels = BTreeMap::from([
        ("sandbox.trogon.live/owner".into(), "known-owner".into()),
        ("sandbox.trogon.live/lease-id".into(), "original-birth-lease".into()),
    ]);
    let actual = serde_json::json!({
        "Id": retained.allocation.container_id.clone(),
        "Image": format!("sha256:{}", "c".repeat(64)),
        "State": {"Running": true, "Pid": 42, "StartedAt": "2026-01-01T00:00:00Z"},
        "Config": {"Labels": retained.allocation.labels.clone()},
    });
    let native: ContainerInspectResponse = serde_json::from_value(actual.clone())?;
    let original = Birth::inspect(&retained.allocation, &native)?;
    for (field, value) in [
        ("/Id", serde_json::json!("d".repeat(64))),
        ("/Image", serde_json::json!(format!("sha256:{}", "e".repeat(64)))),
        ("/State/Pid", serde_json::json!(43)),
        ("/State/StartedAt", serde_json::json!("2026-01-02T00:00:00Z")),
        ("/State/Running", serde_json::json!(false)),
    ] {
        let mut changed = actual.clone();
        *changed.pointer_mut(field).ok_or("fixture field is missing")? = value;
        let native: ContainerInspectResponse = serde_json::from_value(changed)?;
        assert!(
            Birth::inspect(&retained.allocation, &native).map_or(true, |birth| birth != original),
            "changed native field {field}"
        );
    }
    let mut changed = actual;
    changed["Config"]["Labels"]["sandbox.trogon.live/owner"] = serde_json::json!("foreign-owner");
    let native: ContainerInspectResponse = serde_json::from_value(changed)?;
    assert!(Birth::inspect(&retained.allocation, &native).is_err());
    Ok(())
}
