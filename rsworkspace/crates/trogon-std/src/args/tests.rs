use super::*;

#[derive(Clone, Debug, PartialEq)]
struct TestArgs {
    prefix: String,
}

#[test]
fn fixed_args_returns_provided_value() {
    let args = FixedArgs(TestArgs {
        prefix: "test".to_string(),
    });
    let result = args.parse_args();
    assert_eq!(result.prefix, "test");
}
