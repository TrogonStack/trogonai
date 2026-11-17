use super::*;

fn syntax(document: &str) -> TomlSyntax {
    let error = toml::from_str::<toml::Table>(document).unwrap_err();
    TomlSyntax::of(&error, document)
}

#[test]
fn a_syntax_error_keeps_the_parser_message_but_not_the_line_it_was_found_on() {
    let document = "[nats]\nurl = \"fake-unterminated\n";

    let syntax = syntax(document);

    assert!(!syntax.message().is_empty());
    assert!(!syntax.message().contains("fake-unterminated"));
}

#[test]
fn a_syntax_error_is_located_by_line_and_column() {
    let syntax = syntax("[nats]\nurl = = 1\n");

    assert_eq!(syntax.line(), Some(2));
    assert_eq!(syntax.column(), Some(7));
}

#[test]
fn a_located_syntax_error_reads_as_its_message_and_position() {
    let syntax = TomlSyntax {
        message: "expected a value".to_owned(),
        position: Some((2, 7)),
    };

    assert_eq!(format!("{syntax}"), "expected a value at line 2, column 7");
}

#[test]
fn an_unlocated_syntax_error_reads_as_its_message() {
    let error = <toml::de::Error as serde::de::Error>::custom("expected a value");

    let syntax = TomlSyntax::of(&error, "");

    assert_eq!(syntax.line(), None);
    assert_eq!(syntax.column(), None);
    assert_eq!(format!("{syntax}"), "expected a value");
}
