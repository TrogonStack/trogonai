use super::*;

#[tokio::test]
async fn exporting_the_workspace_execs_tar_in_the_pod_of_the_job() -> TestResult {
    let p = provider(MockK8sOps::new(vec![]));

    assert_eq!(p.export_workspace_tar("job-1").await?, Vec::<u8>::new());
    assert_eq!(p.export_workspace_tar_from_pod("pod-1").await?, Vec::<u8>::new());
    Ok(())
}

#[tokio::test]
async fn exec_with_stdin_returns_what_the_command_printed() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    *mock.exec_output.locked() = Some(PodExecOutput {
        stdout: b"echoed".to_vec(),
        ..Default::default()
    });

    let output = provider(mock.clone())
        .exec_in_pod_with_stdin("pod-1", vec!["cat".to_string()], b"input".to_vec())
        .await?;

    assert_eq!(output.stdout, b"echoed".to_vec());
    assert_eq!(mock.exec_calls.locked().clone(), vec![vec!["cat".to_string()]]);
    Ok(())
}

#[tokio::test]
async fn an_interactive_exec_and_a_tty_exec_attach_to_the_pod() -> TestResult {
    let p = provider(MockK8sOps::new(vec![]));

    let interactive = p
        .exec_in_pod_interactive(
            "pod-1",
            vec!["sh".to_string()],
            OutputRetention::default(),
            DEFAULT_MAX_OUTPUT_BYTES,
        )
        .await?;
    assert_eq!(interactive.completion.await?, PodExecOutput::default());

    let tty = p.exec_in_pod_tty("pod-1", vec!["sh".to_string()], (24, 80)).await?;
    assert_eq!(tty.completion.await?, 0);
    Ok(())
}
