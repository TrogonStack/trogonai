use super::*;

/// `(k / 2 ** 52).toString(36)` as printed by Node 24, for `Math.random()` outputs `k / 2 ** 52`.
const NODE_RADIX_36: &[(u64, &str)] = &[
    (0, "0"),
    (1, "0.0000000000t84r35w1ryi"),
    (2, "0.0000000001mg9i6bs3jx"),
    (123_456_789, "0.00001no6ucnwpm4"),
    (629_548_461_913_282, "0.515xw3tq7in"),
    (1_351_441_293_601_126, "0.aswjl3hetmg"),
    (1_501_199_875_790_165, "0.bzzzzzzzzzn"),
    (2_251_799_813_685_248, "0.i"),
    (2_486_182_473_680_578, "0.jvg4xp0i18"),
    (2_840_411_670_329_124, "0.mpduljhg6i"),
    (3_335_162_033_025_665, "0.qnrbl04qcbj"),
    (3_764_178_865_637_584, "0.u37t9q0jjg"),
    (4_503_599_627_370_000, "0.zzzzzzzzotc"),
    (4_503_599_627_370_495, "0.zzzzzzzzzza"),
];

#[test]
fn radix_36_matches_v8_number_to_string() {
    for (k, expected) in NODE_RADIX_36 {
        #[allow(clippy::cast_precision_loss)]
        let value = *k as f64 / 4_503_599_627_370_496.0;
        assert_eq!(radix_36(value), *expected, "k = {k}");
        assert_eq!(
            typescript_id(value),
            expected.chars().skip(2).take(13).collect::<String>()
        );
    }
}

#[test]
fn typescript_ids_are_short_base_36_strings() {
    for _ in 0..1000 {
        let id = ControlRequestId::typescript();
        assert!(id.as_str().len() <= 13);
        assert!(id.as_str().bytes().all(|byte| DIGITS.contains(&byte)), "{id}");
    }
}

#[test]
fn python_ids_carry_the_counter_and_eight_hex_digits() {
    let id = ControlRequestId::python(7);
    let (prefix, random) = id.as_str().split_at(6);
    assert_eq!(prefix, "req_7_");
    assert_eq!(random.len(), 8);
    assert!(
        random
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
}
