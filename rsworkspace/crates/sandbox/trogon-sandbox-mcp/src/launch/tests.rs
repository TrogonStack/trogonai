use trogon_sandbox_contract::{CommandDeadline, CommandExecution, OutputRetention, ReadinessProbe, StdinMode};

use super::{LaunchMode, LaunchPlan, plan as plan_listening};
use crate::inputs::command_inputs::{
    CommandDeadlineInput, OutputRetentionInput, ProcessInput, ReadinessInput, RedactValueInput, RunCommandInput,
    StdinModeInput,
};
use crate::testing::TestResult;
use crate::tool_error::ToolError;

fn plan(input: RunCommandInput) -> Result<LaunchPlan, ToolError> {
    plan_listening(input, true)
}

fn input() -> RunCommandInput {
    RunCommandInput {
        lease_id: "l-1".to_string(),
        command: vec!["echo".to_string(), "hi".to_string()],
        cwd: None,
        env: Default::default(),
        timeout_seconds: 30,
        redact_values: Vec::new(),
        detached: None,
        user: None,
        stream_output: None,
        stream_progress: None,
        deadline: None,
        stdin: None,
        output_retention: None,
        process: None,
        launch_key: None,
    }
}

fn combination(result: Result<super::LaunchPlan, ToolError>) -> Option<&'static str> {
    match result {
        Err(ToolError::InvalidCombination { option, .. }) => Some(option),
        _ => None,
    }
}

#[test]
fn a_plain_run_is_attached() -> TestResult {
    let launch = plan(input())?;
    assert_eq!(launch.mode, LaunchMode::Attached);
    assert_eq!(launch.request.execution, CommandExecution::Attached);
    assert!(!launch.request.stream_output.is_enabled());
    Ok(())
}

#[test]
fn a_streamed_attached_run_keeps_its_streaming() -> TestResult {
    let launch = plan(RunCommandInput {
        stream_output: Some(true),
        ..input()
    })?;
    assert!(launch.request.stream_output.is_enabled());
    Ok(())
}

#[test]
fn a_detached_run_carries_its_options() -> TestResult {
    let launch = plan(RunCommandInput {
        detached: Some(true),
        deadline: Some(CommandDeadlineInput::Lease),
        stdin: Some(StdinModeInput::Interactive),
        output_retention: Some(OutputRetentionInput::StreamOnly),
        launch_key: Some("key-1".to_string()),
        user: Some("dev".to_string()),
        process: Some(ProcessInput {
            name: Some("web".to_string()),
            readiness: Some(ReadinessInput::TcpPort { port: 8080 }),
        }),
        redact_values: vec![RedactValueInput("secret-value".to_string())],
        ..input()
    })?;
    assert_eq!(launch.mode, LaunchMode::Detached);
    assert_eq!(launch.request.output_retention, OutputRetention::StreamOnly);
    assert!(launch.request.launch_key.is_some());
    assert!(launch.request.run_as_user.is_some());
    let CommandExecution::Detached {
        stdin,
        deadline,
        process,
    } = launch.request.execution
    else {
        return Err("expected a detached execution".into());
    };
    assert_eq!((stdin, deadline), (StdinMode::Interactive, CommandDeadline::Lease));
    let spec = process.ok_or("expected a process spec")?;
    assert_eq!(spec.name.map(|n| n.to_string()), Some("web".to_string()));
    assert!(matches!(spec.readiness, Some(ReadinessProbe::TcpPort { .. })));
    Ok(())
}

#[test]
fn a_detached_run_accepts_the_default_options_spelled_out() -> TestResult {
    let launch = plan(RunCommandInput {
        detached: Some(true),
        deadline: Some(CommandDeadlineInput::Timeout),
        stdin: Some(StdinModeInput::Closed),
        output_retention: Some(OutputRetentionInput::Buffer),
        ..input()
    })?;
    assert_eq!(launch.request.output_retention, OutputRetention::Buffer);
    Ok(())
}

#[test]
fn a_progress_run_is_detached_and_streamed() -> TestResult {
    let launch = plan(RunCommandInput {
        stream_progress: Some(true),
        ..input()
    })?;
    assert_eq!(launch.mode, LaunchMode::Progress);
    assert!(launch.request.stream_output.is_enabled());
    assert!(matches!(launch.request.execution, CommandExecution::Detached { .. }));
    Ok(())
}

#[test]
fn progress_without_a_token_is_an_ordinary_run() -> TestResult {
    let launch = plan_listening(
        RunCommandInput {
            stream_progress: Some(true),
            ..input()
        },
        false,
    )?;
    assert_eq!(launch.mode, LaunchMode::Attached);
    Ok(())
}

#[test]
fn progress_conflicts_with_detached() {
    let result = plan(RunCommandInput {
        stream_progress: Some(true),
        detached: Some(true),
        ..input()
    });
    assert_eq!(combination(result), Some("stream_progress"));
}

#[test]
fn detached_only_options_are_refused_on_an_attached_run() {
    let cases = [
        (
            RunCommandInput {
                deadline: Some(CommandDeadlineInput::Lease),
                ..input()
            },
            "deadline lease",
        ),
        (
            RunCommandInput {
                stdin: Some(StdinModeInput::Interactive),
                ..input()
            },
            "stdin interactive",
        ),
        (
            RunCommandInput {
                output_retention: Some(OutputRetentionInput::StreamOnly),
                ..input()
            },
            "output_retention stream_only",
        ),
        (
            RunCommandInput {
                process: Some(ProcessInput {
                    name: None,
                    readiness: None,
                }),
                ..input()
            },
            "process",
        ),
    ];
    for (case, option) in cases {
        assert_eq!(combination(plan(case)), Some(option));
    }
}

#[test]
fn bad_values_name_their_field() {
    let bad_key = plan(RunCommandInput {
        launch_key: Some("no spaces".to_string()),
        ..input()
    });
    assert!(matches!(
        bad_key,
        Err(ToolError::InvalidInput {
            field: "launch_key",
            ..
        })
    ));
    let bad_user = plan(RunCommandInput {
        user: Some(String::new()),
        ..input()
    });
    assert!(matches!(bad_user, Err(ToolError::InvalidInput { field: "user", .. })));
    let bad_env = plan(RunCommandInput {
        env: [(
            trogon_sandbox_contract::CommandEnv::RESERVED_KEY.to_string(),
            "x".to_string(),
        )]
        .into(),
        ..input()
    });
    assert!(matches!(bad_env, Err(ToolError::InvalidInput { field: "env", .. })));
    let too_many = plan(RunCommandInput {
        redact_values: (0..65).map(|i| RedactValueInput(format!("value-{i}"))).collect(),
        ..input()
    });
    assert!(matches!(
        too_many,
        Err(ToolError::InvalidInput {
            field: "redact_values",
            ..
        })
    ));
}

#[test]
fn a_process_spec_is_validated() {
    let detached = |process| RunCommandInput {
        detached: Some(true),
        process: Some(process),
        ..input()
    };
    let bad_name = plan(detached(ProcessInput {
        name: Some("bad name".to_string()),
        readiness: None,
    }));
    assert!(matches!(
        bad_name,
        Err(ToolError::InvalidInput {
            field: "process.name",
            ..
        })
    ));
    let port_zero = plan(detached(ProcessInput {
        name: None,
        readiness: Some(ReadinessInput::TcpPort { port: 0 }),
    }));
    assert!(matches!(
        port_zero,
        Err(ToolError::InvalidChoice {
            field: "process.readiness.port",
            ..
        })
    ));
    let empty_needle = plan(detached(ProcessInput {
        name: None,
        readiness: Some(ReadinessInput::OutputContains { needle: String::new() }),
    }));
    assert!(matches!(
        empty_needle,
        Err(ToolError::InvalidInput {
            field: "process.readiness.needle",
            ..
        })
    ));
}

#[test]
fn a_needle_probe_is_accepted() -> TestResult {
    let launch = plan(RunCommandInput {
        detached: Some(true),
        process: Some(ProcessInput {
            name: None,
            readiness: Some(ReadinessInput::OutputContains {
                needle: "ready".to_string(),
            }),
        }),
        ..input()
    })?;
    let CommandExecution::Detached { process, .. } = launch.request.execution else {
        return Err("expected a detached execution".into());
    };
    assert!(matches!(
        process.and_then(|p| p.readiness),
        Some(ReadinessProbe::OutputContains { .. })
    ));
    Ok(())
}
