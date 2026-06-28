use super::*;

#[test]
fn normalize_adds_prefix() {
    assert_eq!(
        normalize_type_url("trogonai.scheduler.schedules.v1.CreateSchedule"),
        "type.googleapis.com/trogonai.scheduler.schedules.v1.CreateSchedule"
    );
}
