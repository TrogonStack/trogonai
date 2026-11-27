use super::*;

fn emitter(fixture: &Fixture, ending: &str) -> Result<RunCommandRequest, Boxed> {
    fixture.request(&format!("{EMIT}; {ending}"), false)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_204_kill_acknowledgement_requires_the_real_terminal_and_preserves_binary_spools() -> TestResult {
    let fixture = Fixture::new(false).await?;
    fixture.tweak(|native| native.kill_204 = true);
    let response = fixture.run(&fixture.interrupted()?).await;
    let observed = fixture.evidence()?;
    fixture.shutdown().await?;
    assert_eq!(observed.starts, 1);
    assert_eq!(observed.budgets, [Some(1000)]);
    assert_eq!(observed.replies, [204]);
    assert_eq!(observed.accepted_kills, 1);
    assert_eq!(observed.native_exit, Some(137));
    assert!(observed.spooled);
    let output = completed(response)?;
    assert_eq!(output.exit_code, -1);
    assert!(output.timed_out.is_timed_out() && !output.truncated.is_truncated());
    assert_eq!(output.stdout, OUT);
    assert_eq!(output.stderr, ERR);
    Ok(())
}

async fn user_exit_race(code: i32) -> TestResult {
    let fixture = Fixture::new(false).await?;
    fixture.tweak(|native| native.kill_404_after_terminal = true);
    let response = fixture.run(&emitter(&fixture, &format!("exit {code}"))?).await;
    let observed = fixture.evidence()?;
    fixture.shutdown().await?;
    assert_eq!((observed.starts, observed.accepted_kills), (1, 0));
    assert_eq!(observed.budgets, [Some(1000)]);
    assert_eq!(observed.replies, [404]);
    assert_eq!(observed.native_exit, Some(0));
    assert!(observed.spooled && observed.owner_valid && !observed.timeout_proof);
    assert_eq!(observed.status, Some(code.to_string()));
    let output = completed(response)?;
    assert_eq!(output.exit_code, code);
    assert!(!output.timed_out.is_timed_out() && !output.truncated.is_truncated());
    assert_eq!(output.stdout, OUT);
    assert_eq!(output.stderr, ERR);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_404_reconciles_real_user_exit_124_without_inventing_a_timeout() -> TestResult {
    user_exit_race(124).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_404_reconciles_real_user_exit_137_without_inventing_a_timeout() -> TestResult {
    user_exit_race(137).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_404_reconciles_a_real_owned_guest_timeout_marker_and_saved_status() -> TestResult {
    let fixture = Fixture::new(false).await?;
    fixture.tweak(|native| native.kill_404_after_terminal = true);
    let request = fixture.request(&format!("trap 'exit 0' TERM; {EMIT}; while :; do :; done"), false)?;
    let response = fixture.run(&request).await;
    let observed = fixture.evidence()?;
    fixture.shutdown().await?;
    assert_eq!((observed.starts, observed.accepted_kills), (1, 0));
    assert_eq!(observed.budgets, [Some(1000)]);
    assert_eq!(observed.replies, [404]);
    assert_eq!(observed.native_exit, Some(0));
    assert!(observed.spooled && observed.owner_valid && observed.timeout_proof);
    assert_eq!(observed.status.as_deref(), Some("timeout"));
    let output = completed(response)?;
    assert_eq!(output.exit_code, -1);
    assert!(output.timed_out.is_timed_out() && !output.truncated.is_truncated());
    assert_eq!(output.stdout, OUT);
    assert_eq!(output.stderr, b"command timed out after 1s");
    Ok(())
}

async fn until_started(fixture: &Fixture) -> TestResult {
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state().user_starts != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

async fn external_kill_case(detached: bool) -> TestResult {
    let fixture = Fixture::new(false).await?;
    fixture.tweak(|native| native.kill_404_after_terminal = true);
    let request = fixture.request(&format!("trap '' TERM; {EMIT}; sleep 60"), detached)?;
    let (output, repeated) = if detached {
        started(fixture.run(&request).await)?;
        until_started(&fixture).await?;
        fixture.external_kill().await?;
        let first = fixture.poll().await;
        (polled(first.clone())?, Some(fixture.poll().await))
    } else {
        let (response, killed) = tokio::join!(fixture.run(&request), async {
            until_started(&fixture).await?;
            fixture.external_kill().await
        });
        killed?;
        (completed(response)?, None)
    };
    let observed = fixture.evidence()?;
    fixture.shutdown().await?;
    assert_eq!((observed.starts, observed.accepted_kills), (1, 0));
    assert_eq!(observed.budgets, [Some(1000)]);
    assert_eq!(observed.native_exit, Some(137));
    assert!(observed.spooled && observed.owner_valid && !observed.timeout_proof);
    assert!(observed.status.is_none());
    assert_eq!(output.exit_code, 137);
    assert!(!output.timed_out.is_timed_out() && !output.truncated.is_truncated());
    assert_eq!(output.stdout, OUT);
    assert_eq!(output.stderr, ERR);
    if detached {
        assert_eq!(repeated, Some(PollCommandResponse::Completed(output)));
    } else {
        assert_eq!(observed.replies, [404]);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_external_kill_before_the_deadline_is_not_a_timeout_even_after_kill_404() -> TestResult {
    external_kill_case(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detached_external_kill_before_the_deadline_replays_native_137_without_timeout() -> TestResult {
    external_kill_case(true).await
}

async fn unconfirmed_race(view: NativeView, read_failure: bool) -> TestResult {
    let fixture = Fixture::new(false).await?;
    fixture.tweak(|native| {
        native.kill_404_after_terminal = true;
        native.view = view;
        native.reject_original_logs = read_failure;
        native.reject_controls = read_failure;
    });
    let response = fixture.run(&emitter(&fixture, "exit 7")?).await;
    let pending = fixture.poll().await;
    let before = fixture.calls();
    let foreign = fixture.poll_as("foreign-lease", false).await;
    let foreign_session = fixture.poll_as(LEASE, true).await;
    let after = fixture.calls();
    fixture.tweak(|native| {
        native.view = NativeView::Exact;
        native.reject_original_logs = false;
        native.reject_controls = false;
    });
    let recovered = fixture.poll().await;
    let repeated = fixture.poll().await;
    let observed = fixture.evidence()?;
    fixture.shutdown().await?;
    assert_eq!((observed.starts, observed.accepted_kills), (1, 0));
    assert_eq!(observed.budgets, [Some(1000)]);
    assert_eq!(observed.replies, [404]);
    assert_eq!(observed.native_exit, Some(0));
    assert!(observed.spooled && observed.owner_valid && !observed.timeout_proof);
    assert_eq!(observed.status.as_deref(), Some("7"));
    assert!(matches!(response, RunCommandResponse::Denied(_)));
    match view {
        NativeView::Missing => assert!(matches!(pending, PollCommandResponse::NotFound)),
        NativeView::Nonterminal => assert!(matches!(pending, PollCommandResponse::Running)),
        NativeView::Foreign | NativeView::Exact => assert!(matches!(pending, PollCommandResponse::Denied(_))),
    }
    assert!(matches!(foreign, PollCommandResponse::NotFound));
    assert!(matches!(foreign_session, PollCommandResponse::NotFound));
    assert_eq!(
        after, before,
        "a foreign lease or session must not read native metadata or run controls"
    );
    let output = polled(recovered.clone())?;
    assert_eq!(output.exit_code, 7);
    assert!(!output.timed_out.is_timed_out() && !output.truncated.is_truncated());
    assert_eq!(output.stdout, OUT);
    assert_eq!(output.stderr, ERR);
    assert_eq!(repeated, recovered);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_404_without_native_receipt_retains_original_identity_for_retry() -> TestResult {
    unconfirmed_race(NativeView::Missing, false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_404_with_nonterminal_native_receipt_remains_unknown_and_pollable() -> TestResult {
    unconfirmed_race(NativeView::Nonterminal, false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_404_with_foreign_native_receipt_does_not_authorize_completion() -> TestResult {
    unconfirmed_race(NativeView::Foreign, false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_404_with_positive_terminal_but_unreadable_output_retains_retry_authority() -> TestResult {
    unconfirmed_race(NativeView::Exact, true).await
}
