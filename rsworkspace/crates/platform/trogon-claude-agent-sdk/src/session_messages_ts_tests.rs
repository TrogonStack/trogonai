use std::cmp::Ordering::{Greater, Less};

use super::locale_compare;

#[test]
fn collation_differs_from_code_point_order_for_case() {
    assert_eq!(locale_compare("a", "B"), Less);
    assert_eq!("a".cmp("B"), Greater);
    assert_eq!(locale_compare("a", "A"), Less);
    assert_eq!("a".cmp("A"), Greater);
}

#[test]
fn collation_ranks_accents_after_the_base_letters() {
    assert_eq!(locale_compare("\u{e9}", "f"), Less);
    assert_eq!("\u{e9}".cmp("f"), Greater);
    assert_eq!(locale_compare("resume", "r\u{e9}sum\u{e9}"), Less);
    assert_eq!(locale_compare("r\u{e9}sum\u{e9}", "resumf"), Less);
}

#[test]
fn collation_puts_punctuation_before_digits_before_letters() {
    assert_eq!(locale_compare("_", "-"), Less);
    assert_eq!(locale_compare("-", "1"), Less);
    assert_eq!(locale_compare("1", "a"), Less);
    assert_eq!(locale_compare("10", "9"), Less);
    assert_eq!(locale_compare("Z", "a"), Greater);
    assert_eq!("Z".cmp("a"), Less);
}

#[test]
fn iso_timestamps_still_sort_chronologically() {
    assert_eq!(
        locale_compare("2024-01-01T00:00:00.000Z", "2024-01-01T00:00:01.000Z"),
        Less
    );
    assert_eq!(locale_compare("", "2024"), Less);
}
