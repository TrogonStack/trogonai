use super::*;

#[test]
fn every_signal_maps_to_its_unix_number() {
    let expected = [
        (UnixSignal::Hup, 1),
        (UnixSignal::Int, 2),
        (UnixSignal::Quit, 3),
        (UnixSignal::Kill, 9),
        (UnixSignal::Usr1, 10),
        (UnixSignal::Usr2, 12),
        (UnixSignal::Term, 15),
    ];
    for (signal, number) in expected {
        assert_eq!(signal_number(signal), number, "{signal:?}");
    }
}
