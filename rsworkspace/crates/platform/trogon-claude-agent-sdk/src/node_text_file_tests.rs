use super::read_node_text;

fn utf16(units: &[u16]) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
    bytes
}

#[test]
fn utf_8_keeps_its_byte_order_mark_and_replaces_each_maximal_invalid_subpart() {
    assert_eq!(
        read_node_text(b"\xef\xbb\xbfa\xed\xa0\x80\xe2\x82"),
        "\u{feff}a\u{fffd}\u{fffd}\u{fffd}\u{fffd}"
    );
}

#[test]
fn utf_16le_spells_each_lone_unit_as_its_escape_and_drops_a_trailing_odd_byte() {
    let mut bytes = utf16(&[0x22, 0xD800, 0x22, 0xD83D, 0xDE00, 0xDC00]);
    bytes.push(0x7B);
    assert_eq!(read_node_text(&bytes), "\u{feff}\"\\ud800\"\u{1f600}\\udc00");
}

#[test]
fn a_lone_unit_after_an_odd_run_of_backslashes_is_u_fffd() {
    assert_eq!(read_node_text(&utf16(&[0x5C, 0xD800])), "\u{feff}\\\u{fffd}");
    assert_eq!(read_node_text(&utf16(&[0x5C, 0x5C, 0xD800])), "\u{feff}\\\\\\ud800");
}
