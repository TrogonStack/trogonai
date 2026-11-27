use super::*;

#[test]
fn pool_ids_follow_the_non_nested_proxmox_format() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        "sandbox-pool_1.a".parse::<ResourcePoolId>()?.to_string(),
        "sandbox-pool_1.a"
    );
    assert!("".parse::<ResourcePoolId>().is_err());
    assert!("parent/child".parse::<ResourcePoolId>().is_err());
    assert!("pool name".parse::<ResourcePoolId>().is_err());
    Ok(())
}
