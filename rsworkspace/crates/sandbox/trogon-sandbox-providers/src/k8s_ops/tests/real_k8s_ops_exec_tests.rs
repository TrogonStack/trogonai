use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_contract::OutputRetention;

use super::exec_step_tests::{
    ExecStep, RESIZE_CHANNEL, STATUS_CHANNEL, STATUS_SUCCESS, STDERR_CHANNEL, STDIN_CHANNEL, STDOUT_CHANNEL,
    status_exit_code,
};
use super::fake_api_server_tests::FakeApiServer;
use super::fake_exec_server_tests::FakeExecServer;
use super::within_tests::within;
use crate::K8sOpsError;
use crate::k8s_ops::real_k8s_ops::{exec_io, require_stream};
use crate::k8s_ops::{K8sOps, RealK8sOps};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn command() -> Vec<String> {
    vec!["sh".to_string(), "-c".to_string(), "run".to_string()]
}

fn out(channel: u8, data: &str) -> ExecStep {
    ExecStep::Out(channel, data.as_bytes().to_vec())
}

fn status(json: &str) -> ExecStep {
    out(STATUS_CHANNEL, json)
}

async fn server(script: Vec<ExecStep>) -> Result<(FakeExecServer, RealK8sOps), Box<dyn std::error::Error>> {
    let server = FakeExecServer::start(script).await?;
    let ops = RealK8sOps::new(server.client());
    Ok((server, ops))
}

#[tokio::test]
async fn exec_collect_stdout_returns_what_the_command_printed() -> TestResult {
    within(async {
        let (server, ops) = server(vec![
            out(STDOUT_CHANNEL, "hello"),
            out(STDERR_CHANNEL, "warning"),
            status(STATUS_SUCCESS),
        ])
        .await?;

        let stdout = ops.exec_collect_stdout("work", "pod-1", "agent", command()).await?;

        assert_eq!(stdout, b"hello");
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].starts_with("/api/v1/namespaces/work/pods/pod-1/exec?"),
            "{}",
            requests[0]
        );
        assert!(requests[0].contains("container=agent"), "{}", requests[0]);
        assert!(requests[0].contains("command=sh"), "{}", requests[0]);
        assert!(
            requests[0].contains("stdout=true") && requests[0].contains("stderr=true"),
            "{}",
            requests[0]
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn exec_collect_stdout_fails_when_the_connection_drops() -> TestResult {
    within(async {
        let (_server, ops) = server(vec![out(STDOUT_CHANNEL, "partial")]).await?;

        let result = ops.exec_collect_stdout("work", "pod-1", "agent", command()).await;

        assert!(matches!(result, Err(K8sOpsError::ExecJoin(_))), "{result:?}");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn exec_with_output_reports_streams_and_exit_code() -> TestResult {
    within(async {
        let (server, ops) = server(vec![
            out(STDOUT_CHANNEL, "out"),
            out(STDERR_CHANNEL, "err"),
            status(&status_exit_code(3)),
        ])
        .await?;

        let output = ops
            .exec_with_output("work", "pod-1", "agent", command(), None, 1024)
            .await?;

        assert_eq!(output.exit_code, 3);
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");
        assert!(!output.truncated.is_truncated());
        assert!(!output.timed_out);
        assert!(!server.requests()[0].contains("stdin=true"), "{}", server.requests()[0]);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn exec_with_output_feeds_stdin_and_truncates_long_output() -> TestResult {
    within(async {
        let long = "x".repeat(10_000);
        let (server, ops) = server(vec![
            ExecStep::AwaitFrame(STDIN_CHANNEL),
            out(STDOUT_CHANNEL, &long),
            status(STATUS_SUCCESS),
        ])
        .await?;

        let output = ops
            .exec_with_output("work", "pod-1", "agent", command(), Some(b"input".to_vec()), 100)
            .await?;

        assert_eq!(output.exit_code, 0);
        assert!(output.truncated.is_truncated());
        assert!(output.stdout.len() < 1_000, "kept {} bytes", output.stdout.len());
        assert!(server.frames().contains(&(STDIN_CHANNEL, b"input".to_vec())));
        assert!(server.requests()[0].contains("stdin=true"), "{}", server.requests()[0]);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn exec_with_output_fails_when_the_connection_drops() -> TestResult {
    within(async {
        let (_server, ops) = server(Vec::new()).await?;

        let result = ops
            .exec_with_output("work", "pod-1", "agent", command(), None, 1024)
            .await;

        assert!(matches!(result, Err(K8sOpsError::ExecJoin(_))), "{result:?}");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn exec_interactive_buffers_output_when_asked() -> TestResult {
    within(async {
        let (server, ops) = server(vec![
            ExecStep::AwaitFrame(STDIN_CHANNEL),
            out(STDOUT_CHANNEL, "out"),
            out(STDERR_CHANNEL, "err"),
            status(&status_exit_code(2)),
        ])
        .await?;
        let mut exec = ops
            .exec_interactive("work", "pod-1", "agent", command(), OutputRetention::Buffer, 1024)
            .await?;

        exec.stdin.write_all(b"typed").await?;
        let output = exec.completion.await?;

        assert_eq!(output.exit_code, 2);
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");
        assert!(server.frames().contains(&(STDIN_CHANNEL, b"typed".to_vec())));
        Ok(())
    })
    .await
}

#[tokio::test]
async fn exec_interactive_streaming_keeps_only_a_stderr_tail() -> TestResult {
    within(async {
        let stderr = "e".repeat(OutputRetention::STREAM_ONLY_STDERR_TAIL_BYTES + 100);
        let (_server, ops) = server(vec![
            ExecStep::AwaitFrame(STDIN_CHANNEL),
            out(STDOUT_CHANNEL, "discarded"),
            out(STDERR_CHANNEL, &stderr),
            status(STATUS_SUCCESS),
        ])
        .await?;
        let mut exec = ops
            .exec_interactive("work", "pod-1", "agent", command(), OutputRetention::StreamOnly, 10)
            .await?;

        exec.stdin.write_all(b"go").await?;
        let output = exec.completion.await?;

        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr.len(), OutputRetention::STREAM_ONLY_STDERR_TAIL_BYTES);
        assert!(output.truncated.is_truncated());
        Ok(())
    })
    .await
}

#[tokio::test]
async fn exec_interactive_fails_when_the_connection_drops() -> TestResult {
    within(async {
        let (_server, ops) = server(Vec::new()).await?;
        let exec = ops
            .exec_interactive("work", "pod-1", "agent", command(), OutputRetention::Buffer, 10)
            .await?;

        let result = exec.completion.await;

        assert!(matches!(result, Err(K8sOpsError::ExecJoin(_))), "{result:?}");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn exec_tty_streams_a_terminal_and_forwards_resizes() -> TestResult {
    within(async {
        let (server, ops) = server(vec![
            ExecStep::AwaitFrame(RESIZE_CHANNEL),
            out(STDOUT_CHANNEL, "hi"),
            ExecStep::AwaitFrame(RESIZE_CHANNEL),
            out(STDOUT_CHANNEL, " bye"),
            status(&status_exit_code(7)),
        ])
        .await?;
        let mut exec = ops.exec_tty("work", "pod-1", "agent", command(), (80, 24)).await?;

        exec.resize.send((100, 40)).await?;
        exec.stdin.write_all(b"x").await?;
        let mut screen = Vec::new();
        exec.stdout.read_to_end(&mut screen).await?;
        let code = exec.completion.await?;

        assert_eq!(screen, b"hi bye");
        assert_eq!(code, 7);
        let resizes: Vec<String> = server
            .frames()
            .into_iter()
            .filter(|(channel, _)| *channel == RESIZE_CHANNEL)
            .map(|(_, payload)| String::from_utf8_lossy(&payload).into_owned())
            .collect();
        assert_eq!(resizes.len(), 2, "{resizes:?}");
        assert!(resizes[0].contains("80") && resizes[0].contains("24"), "{resizes:?}");
        assert!(resizes[1].contains("100") && resizes[1].contains("40"), "{resizes:?}");
        let request = &server.requests()[0];
        assert!(
            request.contains("tty=true") && !request.contains("stderr=true"),
            "{request}"
        );
        exec.resize.send((1, 1)).await.ok();
        exec.resize.closed().await;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn exec_tty_fails_when_the_connection_drops() -> TestResult {
    within(async {
        let (_server, ops) = server(Vec::new()).await?;
        let exec = ops.exec_tty("work", "pod-1", "agent", command(), (80, 24)).await?;

        let result = exec.completion.await;

        assert!(matches!(result, Err(K8sOpsError::ExecJoin(_))), "{result:?}");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn an_apiserver_that_refuses_the_exec_is_a_kube_error() {
    let api = FakeApiServer::new(Vec::new());
    let ops = api.ops();

    let collect = ops.exec_collect_stdout("work", "pod-1", "agent", command()).await;
    let output = ops
        .exec_with_output("work", "pod-1", "agent", command(), None, 10)
        .await;
    let interactive = ops
        .exec_interactive("work", "pod-1", "agent", command(), OutputRetention::Buffer, 10)
        .await;
    let tty = ops.exec_tty("work", "pod-1", "agent", command(), (80, 24)).await;

    assert!(matches!(collect, Err(K8sOpsError::Kube(_))));
    assert!(matches!(output, Err(K8sOpsError::Kube(_))));
    assert!(matches!(interactive, Err(K8sOpsError::Kube(_))));
    assert!(matches!(tty, Err(K8sOpsError::Kube(_))));
}

#[test]
fn a_stream_the_exec_did_not_provide_is_named_in_the_error() {
    assert!(matches!(
        require_stream(None::<u8>, "stdin"),
        Err(K8sOpsError::ExecStreamMissing { stream: "stdin" })
    ));
    assert!(matches!(require_stream(Some(1), "stdin"), Ok(1)));
}

#[test]
fn an_io_failure_names_the_action_and_stream() {
    let error = exec_io("read", "stdout")(std::io::Error::other("boom"));

    assert!(matches!(
        error,
        K8sOpsError::ExecIo {
            action: "read",
            stream: "stdout",
            ..
        }
    ));
}
