use super::base64_len;

#[test]
fn base64_len_rounds_up_to_whole_quanta() {
    assert_eq!(base64_len(0), 0);
    assert_eq!(base64_len(1), 4);
    assert_eq!(base64_len(3), 4);
    assert_eq!(base64_len(4), 8);
}
