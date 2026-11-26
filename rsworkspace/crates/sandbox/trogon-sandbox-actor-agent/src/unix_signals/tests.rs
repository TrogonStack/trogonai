use super::*;

#[test]
fn every_signal_maps_to_its_unix_counterpart() {
    let pairs = [
        (UnixSignal::Term, Signal::SIGTERM),
        (UnixSignal::Kill, Signal::SIGKILL),
        (UnixSignal::Int, Signal::SIGINT),
        (UnixSignal::Hup, Signal::SIGHUP),
        (UnixSignal::Quit, Signal::SIGQUIT),
        (UnixSignal::Usr1, Signal::SIGUSR1),
        (UnixSignal::Usr2, Signal::SIGUSR2),
    ];
    for (wire, unix) in pairs {
        assert_eq!(to_signal(wire), unix);
    }
}

#[test]
fn a_group_needs_a_pid_that_fits() {
    assert_eq!(group_of(Some(7)), Some(Pid::from_raw(7)));
    assert_eq!(group_of(Some(u32::MAX)), None);
    assert_eq!(group_of(None), None);
}

#[test]
fn signalling_nothing_or_a_vanished_group_is_harmless() {
    deliver_signal(None, UnixSignal::Kill);
    deliver_signal(group_of(Some(i32::MAX as u32)), UnixSignal::Term);
}
