use super::init_logging;

#[test]
fn a_valid_a_broken_and_a_repeated_level_are_all_accepted() {
    init_logging("warn");
    init_logging("=[not a filter");
    init_logging("debug");
}
