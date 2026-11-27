use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn cap(n: u32) -> Result<Option<TunnelCap>, Box<dyn std::error::Error>> {
    Ok(Some(TunnelCap::new(n)?))
}

fn take(gate: &Arc<TunnelGate>) -> Result<TunnelSlot, Box<dyn std::error::Error>> {
    gate.take().map_err(|full| format!("the cap of {full} is full").into())
}

#[test]
fn a_full_cap_refuses_until_a_slot_is_given_back() -> TestResult {
    let gate = Arc::new(TunnelGate::new(cap(1)?, None));
    let slot = take(&gate)?;
    assert_eq!(gate.take().err(), Some(1));
    drop(slot);
    assert!(gate.take().is_ok());
    Ok(())
}

#[test]
fn the_names_kept_are_bounded_and_past_a_hundred_they_are_forgotten() -> TestResult {
    let gate = Arc::new(TunnelGate::new(cap(1)?, None));
    let names: Vec<u64> = (0..101).map(|_| gate.audience()).collect();
    for name in &names[..100] {
        assert!(gate.first_refusal(*name));
    }
    assert!(!gate.first_refusal(names[0]));
    assert!(gate.first_refusal(names[100]));
    assert!(gate.first_refusal(names[0]));
    Ok(())
}

#[test]
fn a_changed_cap_applies_to_the_next_tunnel() -> TestResult {
    let gate = Arc::new(TunnelGate::new(cap(1)?, None));
    let _held = take(&gate)?;
    assert!(gate.take().is_err());
    gate.set_cap(cap(2)?);
    let _second = take(&gate)?;
    gate.set_cap(None);
    assert_eq!(gate.limit(), DEFAULT_TUNNEL_CAP);
    let _third = take(&gate)?;
    assert_eq!(gate.terminated(), 3);
    Ok(())
}

#[test]
fn without_a_configured_cap_the_default_of_256_applies() -> TestResult {
    let gate = Arc::new(TunnelGate::default());
    let held = (0..256).map(|_| take(&gate)).collect::<Result<Vec<_>, _>>()?;
    assert_eq!(gate.take().err(), Some(DEFAULT_TUNNEL_CAP));
    drop(held);
    assert!(gate.take().is_ok());
    Ok(())
}

#[test]
fn the_count_is_shared_by_every_holder_of_the_gate() -> TestResult {
    let gate = Arc::new(TunnelGate::new(cap(2)?, None));
    let other = Arc::clone(&gate);
    let _a = take(&gate)?;
    let _b = take(&other)?;
    assert!(gate.take().is_err());
    Ok(())
}

#[test]
fn each_command_is_told_once_until_half_the_slots_are_free() -> TestResult {
    let gate = Arc::new(TunnelGate::new(cap(4)?, None));
    let (first, second) = (gate.audience(), gate.audience());
    let a = take(&gate)?;
    let b = take(&gate)?;
    let c = take(&gate)?;
    let d = take(&gate)?;
    assert!(gate.first_refusal(first));
    assert!(!gate.first_refusal(first));
    assert!(gate.first_refusal(second));
    drop(a);
    assert!(!gate.first_refusal(first));
    drop(b);
    assert!(gate.first_refusal(first));
    drop((c, d));
    Ok(())
}

#[test]
fn a_handshake_timeout_can_be_set_and_cleared() -> TestResult {
    let gate = TunnelGate::default();
    assert_eq!(gate.handshake(), None);
    gate.set_handshake(Some(Duration::from_millis(1500)));
    assert_eq!(gate.handshake(), Some(Duration::from_millis(1500)));
    gate.set_handshake(None);
    assert_eq!(gate.handshake(), None);
    Ok(())
}
