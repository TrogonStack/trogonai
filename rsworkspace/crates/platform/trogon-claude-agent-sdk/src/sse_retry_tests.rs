use super::*;

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

#[test]
fn reconnect_waits_double_up_to_the_cap() {
    let delays: Vec<Duration> = (1..=7).map(reconnect_delay).collect();
    assert_eq!(
        delays,
        [
            ms(1_000),
            ms(2_000),
            ms(4_000),
            ms(8_000),
            ms(16_000),
            ms(30_000),
            ms(30_000)
        ]
    );
    assert_eq!(reconnect_delay(0), ms(1_000));
    assert_eq!(reconnect_delay(200), ms(30_000));
}

#[test]
fn retry_waits_follow_the_backoff_without_a_retry_after() {
    assert_eq!(retry_delay(1, None, Duration::ZERO), ms(1_000));
    assert_eq!(retry_delay(2, None, ms(7)), ms(2_007));
    assert_eq!(retry_delay(9, Some(f64::NAN), Duration::ZERO), ms(30_000));
    assert_eq!(retry_delay(1, Some(f64::INFINITY), Duration::ZERO), ms(1_000));
}

#[test]
fn a_finite_retry_after_is_clamped_to_one_to_thirty_seconds() {
    assert_eq!(retry_delay(1, Some(0.0), Duration::ZERO), ms(1_000));
    assert_eq!(retry_delay(1, Some(-5_000.0), Duration::ZERO), ms(1_000));
    assert_eq!(retry_delay(1, Some(2_500.0), Duration::ZERO), ms(2_500));
    assert_eq!(retry_delay(1, Some(90_000.0), ms(3)), ms(30_003));
}

#[test]
fn jitter_stays_below_250_milliseconds() {
    assert!((0..64).all(|_| jitter() < ms(250)));
}

#[test]
fn retry_after_reads_like_javascript_number() {
    let cases: [(Option<&str>, Option<f64>); 17] = [
        (None, None),
        (Some(""), None),
        (Some("5"), Some(5_000.0)),
        (Some(" 2 "), Some(2_000.0)),
        (Some(" "), Some(0.0)),
        (Some("1.5"), Some(1_500.0)),
        (Some(".5"), Some(500.0)),
        (Some("5."), Some(5_000.0)),
        (Some("1e2"), Some(100_000.0)),
        (Some("-2"), Some(-2_000.0)),
        (Some("+3"), Some(3_000.0)),
        (Some("0x10"), Some(16_000.0)),
        (Some("0b11"), Some(3_000.0)),
        (Some("0o7"), Some(7_000.0)),
        (Some("\u{feff}4\u{a0}"), Some(4_000.0)),
        (Some("-Infinity"), Some(f64::NEG_INFINITY)),
        (Some("Infinity"), Some(f64::INFINITY)),
    ];
    for (header, expected) in cases {
        assert_eq!(retry_after_ms(header), expected, "{header:?}");
    }
}

#[test]
fn retry_after_that_is_not_a_number_is_nan() {
    for header in [
        "soon", "0x", "0xg", "1e", "1e+", "e5", ".", "1.2.3", "1_0", "inf", "nan", "\u{85}5", "--1", "0b2",
    ] {
        let value = retry_after_ms(Some(header));
        assert!(value.is_some_and(f64::is_nan), "{header:?} gave {value:?}");
    }
}
