use super::*;
use acp_nats::nats::AcpStream;
use trogon_nats::jetstream::MockJetStreamContext;

#[tokio::test]
async fn only_the_prompt_path_streams_are_created() {
    let js = MockJetStreamContext::new();
    let prefix = AcpPrefix::new("acp").expect("prefix");

    provision(&js, &prefix).await.expect("provision");

    let names: Vec<String> = js.created_streams().into_iter().map(|config| config.name).collect();
    assert_eq!(
        names,
        [
            AcpStream::Commands.stream_name(&prefix),
            AcpStream::Responses.stream_name(&prefix),
        ]
    );
}

#[test]
fn no_host_stream_captures_a_core_request_subject() {
    let prefix = AcpPrefix::new("acp").expect("prefix");
    for stream in HOST_STREAMS {
        for subject in stream.subject_patterns(&prefix) {
            assert!(
                !subject.contains(".global.") && !subject.contains(".client."),
                "{subject} is also used for core request/reply"
            );
        }
    }
}
