use super::*;

#[test]
fn no_idle_limit_turns_auto_stop_off() {
    assert_eq!(auto_intervals(None).auto_stop, Some(0));
}

#[test]
fn the_idle_limit_rounds_up_to_whole_minutes() {
    assert_eq!(auto_intervals(Some(1)).auto_stop, Some(1));
    assert_eq!(auto_intervals(Some(60)).auto_stop, Some(1));
    assert_eq!(auto_intervals(Some(61)).auto_stop, Some(2));
}

#[test]
fn archive_and_delete_keep_the_daytona_defaults() {
    let intervals = auto_intervals(Some(300));
    assert_eq!(intervals.auto_archive, None);
    assert_eq!(intervals.auto_delete, None);
}
