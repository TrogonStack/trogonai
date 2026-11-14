#![allow(clippy::expect_used)]

use super::command;
use crate::{ArgvStyle, ClaudeAgentOptions, ScriptRuntime, environment_plan::Inherited};
use std::path::Path;

fn program_and_leading(options: &ClaudeAgentOptions, cli: &str, leading: usize) -> (String, Vec<String>) {
    let built = command(options, Path::new(cli), &Inherited::new(), None).expect("command");
    let std = built.as_std();
    (
        std.get_program().to_string_lossy().into_owned(),
        std.get_args()
            .take(leading)
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect(),
    )
}

fn runtime(style: ArgvStyle) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        argv_style: style,
        script_runtime: ScriptRuntime {
            executable: Some("bun".into()),
            arguments: vec!["--smol".into()],
        },
        ..ClaudeAgentOptions::default()
    }
}

#[test]
fn typescript_runs_a_script_under_the_executable_with_its_arguments_first() {
    let (program, leading) = program_and_leading(&runtime(ArgvStyle::TypeScript), "/x/cli.mjs", 2);
    assert_eq!(program, "bun");
    assert_eq!(leading, ["--smol", "/x/cli.mjs"]);
}

#[test]
fn typescript_prepends_executable_arguments_to_a_native_binary() {
    let (program, leading) = program_and_leading(&runtime(ArgvStyle::TypeScript), "/x/claude", 2);
    assert_eq!(program, "/x/claude");
    assert_eq!(leading[0], "--smol");
    assert_ne!(leading[1], "/x/claude");
}

#[test]
fn typescript_extension_test_is_case_sensitive() {
    let (program, _) = program_and_leading(&runtime(ArgvStyle::TypeScript), "/x/cli.JS", 0);
    assert_eq!(program, "/x/cli.JS");
    for script in ["a.js", "a.mjs", "a.ts", "a.tsx", "a.jsx"] {
        let (program, _) = program_and_leading(&runtime(ArgvStyle::TypeScript), script, 0);
        assert_eq!(program, "bun", "{script}");
    }
}

#[test]
fn python_keeps_its_lenient_extension_check_and_ignores_executable_arguments_for_a_binary() {
    let (program, _) = program_and_leading(&runtime(ArgvStyle::Python), "/x/cli.JS", 0);
    assert_eq!(program, "bun");
    let (program, leading) = program_and_leading(&runtime(ArgvStyle::Python), "/x/claude", 1);
    assert_eq!(program, "/x/claude");
    assert_ne!(leading[0], "--smol");
}
