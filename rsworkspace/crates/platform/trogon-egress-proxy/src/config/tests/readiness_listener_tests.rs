use super::*;

#[test]
fn readiness_listens_only_on_loopback_until_explicitly_overridden() {
    let args = parse(&[]).expect("default arguments");
    assert_eq!(
        args.readiness_listen,
        "127.0.0.1:8081".parse().expect("loopback address")
    );
    let args = parse(&["--readiness-listen", "0.0.0.0:9090"]).expect("explicit restricted deployment address");
    assert_eq!(args.readiness_listen, "0.0.0.0:9090".parse().expect("explicit address"));
}
