use serde_json::Value;

use super::*;

// Expected strings below were produced by running the pinned 0.3.323 `PA`/`rU` and `Date#toISOString` in node.

fn open(id: &str) -> String {
    format!("<pasted_content id=\"{id}\">\n")
}

fn close(id: &str) -> String {
    format!("\n</pasted_content id=\"{id}\">")
}

#[test]
fn strip_pasted_matches_the_bundle() {
    assert_eq!(strip_pasted("plain"), "plain");
    let wrapped = format!("before\n\n{}BODY{}\n\nafter", open("ab12"), close("ab12"));
    assert_eq!(strip_pasted(&wrapped), "beforeBODYafter");
    let only = format!("{}only{}", open("ab12"), close("ab12"));
    assert_eq!(strip_pasted(&only), "only");
    let uppercase_id = format!("x {}not{} y", open("zz12"), close("zz12"));
    assert_eq!(strip_pasted(&uppercase_id), uppercase_id, "a non-hex id is not a block");
    let two = format!(
        "a\n{}one{}\n{}two{}\nz",
        open("ab12"),
        close("ab12"),
        open("cd34"),
        close("cd34")
    );
    assert_eq!(strip_pasted(&two), "aonetwoz");
    let unterminated = format!("{}unterminated", open("ab12"));
    assert_eq!(strip_pasted(&unterminated), unterminated);
    let empty = format!("{}{}tail", open("ab12"), "\n</pasted_content id=\"ab12\">");
    assert_eq!(strip_pasted(&empty), "tail");
}

#[test]
fn iso_from_epoch_ms_matches_date_to_iso_string() {
    assert_eq!(iso_from_epoch_ms(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(iso_from_epoch_ms(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
    assert_eq!(iso_from_epoch_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
    assert_eq!(iso_from_epoch_ms(4_107_542_400_999), "2100-03-01T00:00:00.999Z");
    assert_eq!(iso_from_epoch_ms(1_582_934_400_000), "2020-02-29T00:00:00.000Z");
}

#[test]
fn plugin_prompt_rewrites_only_the_exact_envelope() {
    let plugin: Value = serde_json::json!({"text": "HELLO"});
    let content = format!("The foo plugin sent a message:\nHELLO{TS_PLUGIN_PROMPT_TRAILER}");
    assert_eq!(
        plugin_prompt(&content, Some(&plugin)).as_deref(),
        Some("Prompt from the foo plugin: HELLO")
    );
    assert_eq!(plugin_prompt(&content, None), None);
    assert_eq!(plugin_prompt(&content, Some(&serde_json::json!({"text": ""}))), None);
    assert_eq!(
        plugin_prompt(&content, Some(&serde_json::json!({"text": "OTHER"}))),
        None
    );
    let altered = format!("{content} extra");
    assert_eq!(plugin_prompt(&altered, Some(&plugin)), None);
    let one_letter = format!("The a plugin sent a message:\nHELLO{TS_PLUGIN_PROMPT_TRAILER}");
    assert_eq!(
        plugin_prompt(&one_letter, Some(&plugin)).as_deref(),
        Some("Prompt from the a plugin: HELLO")
    );
    let empty_label = format!("The  plugin sent a message:\nHELLO{TS_PLUGIN_PROMPT_TRAILER}");
    assert_eq!(
        plugin_prompt(&empty_label, Some(&plugin)),
        None,
        "the header must exceed 28 UTF-16 units"
    );
}

#[test]
fn js_trim_follows_javascript_whitespace() {
    assert_eq!(js_trim("\u{feff} a \u{a0}"), "a");
    assert_eq!(
        js_trim("\u{85}a\u{85}"),
        "\u{85}a\u{85}",
        "U+0085 is not JavaScript whitespace"
    );
    assert_eq!(js_trim("\u{2028}a\u{2029}"), "a");
}

#[test]
fn typescript_command_name_stops_at_every_js_line_terminator() {
    let text = "<command-name>/x\u{2028}y</command-name>";
    assert_eq!(command_name(text, true), None);
    assert_eq!(command_name(text, false), Some("/x\u{2028}y"));
}
