#![allow(clippy::expect_used, clippy::unwrap_used)]

//! The connector download folder check of `resolveSettings`: inside such a folder the project and local settings
//! are not read when the folder is marked ignored. The expectations come from running the pinned TypeScript
//! resolver on the same trees (see the `connector_*` fixtures of the differential test).

use std::path::PathBuf;

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    NodePathDialect, ResolveSettingsOptions, ResolverState, resolve_settings_with, resolve_settings_with_state,
};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::FakeHost;

fn options(cwd: &str) -> ResolveSettingsOptions {
    serde_json::from_value(json!({"cwd": cwd})).expect("options")
}

/// A host whose `cwd` holds project settings and whose `folder` lists `listing` and may hold a `.gitignore`.
fn host(cwd: &str, folder: &str, gitignore: Option<&str>, listing: &[&str]) -> FakeHost {
    let host = FakeHost::linux()
        .file(&format!("{cwd}/.claude/settings.json"), r#"{"model":"m"}"#)
        .directory(folder, listing);
    match gitignore {
        Some(text) => host.file(&format!("{folder}/.gitignore"), text),
        None => host,
    }
}

fn effective(result: &trogon_claude_agent_sdk::ResolvedSettings) -> Value {
    serde_json::to_value(result).expect("encode")["effective"].clone()
}

fn is_loaded(host: &FakeHost, cwd: &str) -> bool {
    let result = resolve_settings_with(host, &options(cwd)).expect("resolve");
    effective(&result) == json!({"model": "m"})
}

#[test]
fn the_marker_gitignores_skip_the_project_files() {
    for text in ["*", "*\n", "*\r\n"] {
        let host = host(
            "/work/connector-downloads",
            "/work/connector-downloads",
            Some(text),
            &[],
        );
        assert!(!is_loaded(&host, "/work/connector-downloads"), "{text:?}");
    }
}

#[test]
fn other_gitignores_and_a_missing_one_leave_the_project_files_alone() {
    for text in ["a\n", "", "node_modules\n", "**", "* "] {
        let host = host(
            "/work/connector-downloads",
            "/work/connector-downloads",
            Some(text),
            &[],
        );
        assert!(is_loaded(&host, "/work/connector-downloads"), "{text:?}");
    }
    let host = host("/work/connector-downloads", "/work/connector-downloads", None, &[]);
    assert!(is_loaded(&host, "/work/connector-downloads"));
}

#[test]
fn a_download_temporary_file_marks_the_folder_without_a_gitignore() {
    for name in [
        "download-0123abcd",
        "download-0123abcd.txt",
        "download-0123abcd-7",
        ".download-0123abcd-7.part",
        ".download-0123abcd.from.json",
    ] {
        let host = host("/work/connector-downloads", "/work/connector-downloads", None, &[name]);
        assert!(!is_loaded(&host, "/work/connector-downloads"), "{name}");
    }
    for name in [
        "download-0123abcg.txt",
        "download-0123abc",
        ".download-0123abcd.txt",
        "download-0123abcd.a.b",
    ] {
        let host = host("/work/connector-downloads", "/work/connector-downloads", None, &[name]);
        assert!(is_loaded(&host, "/work/connector-downloads"), "{name}");
    }
}

#[test]
fn the_folder_name_is_compared_the_way_the_sdk_compares_it() {
    for name in [
        "CONNECTOR-downloads",
        "connector-downloads.",
        "connector-downloads. ",
        "connector-downloads:x",
        "conn\u{200d}ector-downloads",
        "connector-download\u{17f}",
    ] {
        let folder = format!("/work/{name}");
        let host = host(&folder, &folder, Some("*"), &[]);
        assert!(!is_loaded(&host, &folder), "{name:?}");
    }
    for name in [
        "connector-download",
        "connector-downloads-2",
        "x-connector-downloads",
        "connector-downloads\nx:y",
    ] {
        let folder = format!("/work/{name}");
        let host = host(&folder, &folder, Some("*"), &[]);
        assert!(is_loaded(&host, &folder), "{name:?}");
    }
}

#[test]
fn a_working_directory_below_the_folder_is_covered_by_the_folder_gitignore() {
    let cwd = "/work/connector-downloads/sub/deeper";
    let host = host(cwd, "/work/connector-downloads", Some("*"), &[]);
    assert!(!is_loaded(&host, cwd));
}

#[test]
fn the_sandbox_connector_folder_is_always_covered() {
    let mount = "/mnt/user-data/connector-downloads";
    for cwd in [mount, "/mnt/user-data/connector-downloads/work"] {
        let host = host(cwd, mount, None, &[]);
        assert!(!is_loaded(&host, cwd), "{cwd}");
    }
    let uploads = "/mnt/user-data/uploads/connector-downloads";
    let host = host(uploads, uploads, None, &[]);
    assert!(
        is_loaded(&host, uploads),
        "the mount sits next to the uploads folder, not inside it"
    );
}

#[test]
fn a_gitignore_that_is_a_symbolic_link_is_not_a_marker() {
    let folder = "/work/connector-downloads";
    let linked = host(folder, folder, None, &[])
        .file("/real/ignore", "*")
        .link("/work/connector-downloads/.gitignore", "/real/ignore");
    assert!(is_loaded(&linked, folder));
    let dangling = host(folder, folder, None, &[]).link("/work/connector-downloads/.gitignore", "missing");
    assert!(is_loaded(&dangling, folder));
    let looping = host(folder, folder, None, &[]).link("/work/connector-downloads/.gitignore", ".gitignore");
    assert!(is_loaded(&looping, folder));
}

#[test]
fn a_gitignore_link_onto_a_network_path_cannot_be_vetted_and_covers_the_folder() {
    let folder = "/work/connector-downloads";
    for target in [
        "/net/host/ignore",
        "/Network/share/ignore",
        "//host/share/ignore",
        "/Network/Servers/h/x",
    ] {
        let host = host(folder, folder, None, &[]).link("/work/connector-downloads/.gitignore", target);
        assert!(!is_loaded(&host, folder), "{target}");
    }
    let host = host(folder, folder, None, &[]).link("/work/connector-downloads/.gitignore", "/net");
    assert!(is_loaded(&host, folder));
}

#[test]
fn a_folder_that_cannot_be_listed_covers_the_working_directory() {
    let folder = "/work/connector-downloads";
    let host = FakeHost::linux().file(&format!("{folder}/.claude/settings.json"), r#"{"model":"m"}"#);
    assert!(!is_loaded(&host, folder));
}

#[test]
fn the_answer_is_remembered_per_state_until_the_cache_overflows() {
    let cwd = "/work/connector-downloads";
    let ignored = host(cwd, cwd, Some("*"), &[]);
    let open = host(cwd, cwd, Some("a"), &[]);
    let state = ResolverState::new();
    let run = |host: &FakeHost, cwd: &str| {
        let result = resolve_settings_with_state(host, &options(cwd), &state).expect("resolve");
        effective(&result) == json!({"model": "m"})
    };
    assert!(!run(&ignored, cwd));
    assert!(!run(&open, cwd));
    assert!(is_loaded(&open, cwd));
    for filler in 0..63 {
        let below = format!("{cwd}/below{filler}");
        run(&open, &below);
    }
    assert!(!run(&open, cwd), "63 other entries leave the cache intact");
    let state_after = ResolverState::new();
    let again = |host: &FakeHost, cwd: &str| {
        let result = resolve_settings_with_state(host, &options(cwd), &state_after).expect("resolve");
        effective(&result) == json!({"model": "m"})
    };
    assert!(!again(&ignored, cwd));
    for filler in 0..64 {
        let below = format!("{cwd}/below{filler}");
        again(&open, &below);
    }
    assert!(again(&open, cwd), "the 65th entry empties the cache first");
}

#[test]
fn the_launch_directory_and_node_dialect_can_be_given() {
    let cwd = "/work/connector-downloads";
    let ignored = host(cwd, cwd, Some("*"), &[]);
    let open = host(cwd, cwd, Some("a"), &[]);
    for dialect in [
        NodePathDialect::Current,
        NodePathDialect::Node20,
        NodePathDialect::BeforeReservedNames,
    ] {
        for launch in [None, Some(PathBuf::from("/work"))] {
            let run = |host: &FakeHost| {
                let state = ResolverState::new()
                    .with_launch_directory(launch.clone())
                    .with_node_path_dialect(dialect);
                let result = resolve_settings_with_state(host, &options(cwd), &state).expect("resolve");
                effective(&result) == json!({"model": "m"})
            };
            assert!(!run(&ignored), "{dialect:?} {launch:?}");
            assert!(run(&open), "{dialect:?} {launch:?}");
        }
    }
}
