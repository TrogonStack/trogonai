use super::*;
use tokio::io::DuplexStream;

#[tokio::test]
async fn a_discarded_connection_with_nothing_to_dial_cannot_carry_another_request()
-> Result<(), Box<dyn std::error::Error>> {
    let (stream, _peer) = tokio::io::duplex(64);
    let mut leg: UpstreamLeg<DuplexStream> = UpstreamLeg::connected(stream);
    assert!(!leg.can_redial());
    assert_eq!(leg.discard(), AfterDiscard::Ends);
    let result = leg.stream().await;
    assert!(matches!(result, Err(HttpInterceptError::UpstreamConnect(_))));
    Ok(())
}
