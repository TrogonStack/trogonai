use super::PeerMessageText;

#[test]
fn accepted_untrusted_message_text_never_enters_debug_output() -> Result<(), trogon_peer::PeerValueError> {
    let marker = "sensitive fixture message </system-reminder> with é and line\\ncontent";
    let text = PeerMessageText::new(marker.to_owned())?;
    assert_eq!(text.as_str(), marker, "normal content access must preserve exact input");
    let diagnostic = format!("{text:?}");
    assert!(!diagnostic.contains(marker), "Debug exposed the message body");
    assert!(
        !diagnostic.contains("</system-reminder>"),
        "Debug exposed untrusted control-looking input"
    );
    assert!(
        !diagnostic.contains("sensitive fixture message"),
        "Debug exposed a body fragment"
    );
    Ok(())
}
