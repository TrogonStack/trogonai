use super::*;

#[test]
fn converts_nats_message_with_ttl_to_proto() {
    let delivery = ScheduleEventDelivery::NatsMessage {
        subject: DeliveryRoute::new("agent.run").unwrap(),
        ttl: Some(TtlDuration::from_secs(30).unwrap()),
        source: None,
    };

    let proto = v1::Delivery::try_from(&delivery).unwrap();
    let v1::delivery::Kind::NatsMessage(message) = proto.kind.unwrap();
    assert_eq!(message.subject, "agent.run");
    assert_eq!(message.ttl.as_option().unwrap().seconds, 30);
}
