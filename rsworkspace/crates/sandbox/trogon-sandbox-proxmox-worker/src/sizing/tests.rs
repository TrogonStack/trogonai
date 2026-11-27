use super::*;

#[test]
fn whole_cores_are_not_limited() {
    assert_eq!(
        cpu_config(2000),
        CpuConfig {
            cores: 2,
            cpulimit: None
        }
    );
}

#[test]
fn fractional_requests_round_up_and_cap_the_guest() {
    assert_eq!(
        cpu_config(1500),
        CpuConfig {
            cores: 2,
            cpulimit: Some(1.5)
        }
    );
    assert_eq!(
        cpu_config(100),
        CpuConfig {
            cores: 1,
            cpulimit: Some(0.1)
        }
    );
}
