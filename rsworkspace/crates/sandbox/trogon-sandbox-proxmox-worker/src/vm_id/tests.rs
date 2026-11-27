use super::*;

#[test]
fn ids_inside_the_proxmox_range_parse() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(u32::from(VmId::try_from(100)?), 100);
    assert_eq!("999999999".parse::<VmId>()?.to_string(), "999999999");
    Ok(())
}

#[test]
fn ids_outside_the_range_or_not_decimal_are_refused() {
    assert!(VmId::try_from(99).is_err());
    assert!(VmId::try_from(1_000_000_000).is_err());
    assert!("".parse::<VmId>().is_err());
    assert!("+201".parse::<VmId>().is_err());
    assert!("99999999999".parse::<VmId>().is_err());
}
