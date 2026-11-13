#![cfg(unix)]

use std::{
    cell::RefCell,
    ffi::OsString,
    path::{Path, PathBuf},
};

use trogon_claude_agent_sdk::{
    NativePackageTarget, NodeModuleLookup, ResolveNodeModule, adapters::NativeFileSystem, find_bundled_cli,
    find_native_cli,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn touch(path: &Path) -> TestResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, "")?;
    Ok(())
}

#[test]
fn bundled_cli_is_a_file_under_the_bundled_directory() -> TestResult {
    let root = tempfile::tempdir()?;
    assert_eq!(find_bundled_cli(root.path(), false, &NativeFileSystem), None);
    std::fs::create_dir_all(root.path().join("_bundled/claude"))?;
    assert_eq!(find_bundled_cli(root.path(), false, &NativeFileSystem), None);
    std::fs::remove_dir(root.path().join("_bundled/claude"))?;
    touch(&root.path().join("_bundled/claude"))?;
    assert_eq!(
        find_bundled_cli(root.path(), false, &NativeFileSystem),
        Some(root.path().join("_bundled/claude"))
    );
    assert_eq!(find_bundled_cli(root.path(), true, &NativeFileSystem), None);
    touch(&root.path().join("_bundled/claude.exe"))?;
    assert_eq!(
        find_bundled_cli(root.path(), true, &NativeFileSystem),
        Some(root.path().join("_bundled/claude.exe"))
    );
    Ok(())
}

fn specifiers(platform: &str, arch: &str, musl: bool) -> Vec<String> {
    NativePackageTarget::new(platform, arch, musl).specifiers()
}

#[test]
fn linux_orders_the_libc_variants_by_preference() {
    let s = "@anthropic-ai/claude-agent-sdk";
    assert_eq!(
        specifiers("linux", "x64", false),
        [format!("{s}-linux-x64/claude"), format!("{s}-linux-x64-musl/claude")]
    );
    assert_eq!(
        specifiers("linux", "arm64", true),
        [
            format!("{s}-linux-arm64-musl/claude"),
            format!("{s}-linux-arm64/claude")
        ]
    );
}

#[test]
fn other_platforms_have_one_package_and_only_windows_adds_exe() {
    let s = "@anthropic-ai/claude-agent-sdk";
    assert_eq!(
        specifiers("darwin", "arm64", true),
        [format!("{s}-darwin-arm64/claude")]
    );
    assert_eq!(
        specifiers("win32", "arm64", false),
        [format!("{s}-win32-arm64/claude.exe")]
    );
    assert_eq!(
        specifiers("android", "arm64", true),
        [format!("{s}-linux-arm64-android/claude")]
    );
    assert_eq!(specifiers("freebsd", "x64", false), [format!("{s}-freebsd-x64/claude")]);
}

struct Scripted(RefCell<Vec<String>>, Vec<(String, PathBuf)>);

impl ResolveNodeModule for Scripted {
    type Error = ();

    fn resolve(&self, specifier: &str) -> Result<PathBuf, ()> {
        self.0.borrow_mut().push(specifier.to_owned());
        self.1
            .iter()
            .find(|(known, _)| known == specifier)
            .map(|(_, path)| path.clone())
            .ok_or(())
    }
}

#[test]
fn a_resolved_path_must_exist_and_later_candidates_are_tried() -> TestResult {
    let root = tempfile::tempdir()?;
    touch(&root.path().join("musl"))?;
    let target = NativePackageTarget::new("linux", "x64", false);
    let resolver = Scripted(
        RefCell::default(),
        vec![
            (
                "@anthropic-ai/claude-agent-sdk-linux-x64/claude".into(),
                root.path().join("missing"),
            ),
            (
                "@anthropic-ai/claude-agent-sdk-linux-x64-musl/claude".into(),
                root.path().join("musl"),
            ),
        ],
    );
    assert_eq!(
        find_native_cli(&target, &resolver, &NativeFileSystem),
        Ok(root.path().join("musl"))
    );
    assert_eq!(resolver.0.borrow().len(), 2);
    Ok(())
}

#[test]
fn a_miss_names_the_platform_and_the_upstream_remedy() {
    let target = NativePackageTarget::new("win32", "x64", false);
    let resolver = Scripted(RefCell::default(), vec![]);
    let error = find_native_cli(&target, &resolver, &NativeFileSystem).err();
    assert_eq!(
        error.map(|e| e.to_string()),
        Some("Native CLI binary for win32-x64 not found. Reinstall @anthropic-ai/claude-agent-sdk without --omit=optional, or set options.pathToClaudeCodeExecutable.".to_owned())
    );
}

#[test]
fn lookup_directories_follow_node_order() {
    let node_path = OsString::from("/np1::/np2");
    let lookup = NodeModuleLookup::new(
        Path::new("/a/node_modules/pkg/dist"),
        Some(&node_path),
        Some(Path::new("/home/u")),
        Some(Path::new("/opt/node")),
        NativeFileSystem,
    );
    let expected: Vec<PathBuf> = [
        "/a/node_modules/pkg/dist/node_modules",
        "/a/node_modules/pkg/node_modules",
        "/a/node_modules",
        "/node_modules",
        "/np1",
        "/np2",
        "/home/u/.node_modules",
        "/home/u/.node_libraries",
        "/opt/node/lib/node",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    assert_eq!(lookup.directories(), expected.as_slice());
}

#[test]
fn lookup_resolves_the_nearest_file_to_its_canonical_path() -> TestResult {
    let root = tempfile::tempdir()?;
    let root_path = root.path().canonicalize()?;
    let package = "@anthropic-ai/claude-agent-sdk-linux-x64/claude";
    touch(&root_path.join("node_modules").join(package))?;
    touch(&root_path.join("app/node_modules").join(package))?;
    let lookup = NodeModuleLookup::new(&root_path.join("app/src"), None, None, None, NativeFileSystem);
    assert_eq!(
        lookup.resolve(package),
        Ok(root_path.join("app/node_modules").join(package))
    );
    std::fs::remove_file(root_path.join("app/node_modules").join(package))?;
    assert_eq!(
        lookup.resolve(package),
        Ok(root_path.join("node_modules").join(package))
    );
    let miss = lookup.resolve("nope/claude").err().map(|e| e.to_string());
    assert_eq!(miss.as_deref(), Some("Cannot find module 'nope/claude'"));
    Ok(())
}

#[test]
fn host_target_is_a_node_spelling() {
    let host = NativePackageTarget::host();
    assert!(!host.platform().is_empty() && !host.arch().is_empty());
    assert_ne!(host.platform(), "macos");
    assert_ne!(host.arch(), "x86_64");
}
