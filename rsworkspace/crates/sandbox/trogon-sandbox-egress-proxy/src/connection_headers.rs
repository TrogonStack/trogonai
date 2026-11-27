/// Whether the request asks for its connection to close after the answer.
pub(crate) fn asks_to_close(headers: &[(String, String)]) -> bool {
    names_connection_token(headers, "close")
}

pub(crate) fn asks_to_keep_alive(headers: &[(String, String)]) -> bool {
    names_connection_token(headers, "keep-alive")
}

fn names_connection_token(headers: &[(String, String)], wanted: &str) -> bool {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case(wanted))
}
