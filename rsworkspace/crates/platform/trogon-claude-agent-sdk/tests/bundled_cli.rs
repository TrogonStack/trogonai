#![cfg(unix)]

use std::{
    cell::RefCell,
    ffi::OsString,
    path::{Path, PathBuf},
};

use trogon_claude_agent_sdk::{
    NativePackageTarget, NodeModuleLookup, ResolveNodeModule,
    adapters::{NativeFileSystem, NativeHost},
    bundled_cli_derives_client_app, find_bundled_cli, find_native_cli, meets_derived_client_app_minimum,
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

#[test]
fn derived_client_app_minimum_compares_the_leading_triple_as_a_list() {
    for version in [
        "999.0.0",
        "999.0.1",
        "999.1.0",
        "1000.0.0",
        "999.0.0-beta",
        "999.0.0 trailing",
        "999.0.1234567890",
    ] {
        assert!(meets_derived_client_app_minimum(version), "{version}");
    }
    for version in [
        "",
        "2.1.248",
        "998.9.9",
        "998.999.999",
        "999.0",
        "999",
        "v999.0.0",
        " 999.0.0",
        "\u{feff}999.0.0",
        "999.0.x",
        "1234567890.0.0",
        "999.1234567890.0",
        "999,0,0",
    ] {
        assert!(!meets_derived_client_app_minimum(version), "{version}");
    }
}

const QUALIFYING_VERSION: &str = "999.0.0";

fn bundled_install(version_record: Option<&[u8]>) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let cli = root.path().join("_bundled/claude");
    touch(&cli)?;
    if let Some(record) = version_record {
        std::fs::write(root.path().join("_bundled/claude.version"), record)?;
    }
    Ok((root, cli))
}

fn derives(root: &Path, cli: &Path, pinned: &str) -> bool {
    bundled_cli_derives_client_app(cli, root, false, pinned, &NativeFileSystem, &NativeHost)
}

#[test]
fn the_bundled_cli_derives_a_label_only_when_the_pin_and_its_record_qualify() -> TestResult {
    let (root, cli) = bundled_install(Some(b"999.0.0\n"))?;
    assert!(derives(root.path(), &cli, QUALIFYING_VERSION));
    assert!(
        !derives(root.path(), &cli, "2.1.248"),
        "a pin below the minimum reads nothing"
    );
    assert!(!derives(root.path(), &cli, ""));
    for record in [
        &b"2.1.248"[..],
        b"",
        b"not a version",
        b"\xff\xfe999.0.0",
        b"\xef\xbb\xbf999.0.0",
    ] {
        let (root, cli) = bundled_install(Some(record))?;
        assert!(!derives(root.path(), &cli, QUALIFYING_VERSION), "{record:?}");
    }
    Ok(())
}

#[test]
fn a_bundled_cli_without_a_record_counts_as_too_old() -> TestResult {
    let (root, cli) = bundled_install(None)?;
    assert!(!derives(root.path(), &cli, QUALIFYING_VERSION));
    Ok(())
}

#[test]
fn only_the_executable_found_in_the_bundled_directory_derives_a_label() -> TestResult {
    let (root, cli) = bundled_install(Some(b"999.0.0"))?;
    let other = root.path().join("elsewhere/claude");
    touch(&other)?;
    std::fs::write(root.path().join("elsewhere/claude.version"), "999.0.0")?;
    assert!(!derives(root.path(), &other, QUALIFYING_VERSION));
    assert!(!derives(
        root.path(),
        &root.path().join("_bundled/missing"),
        QUALIFYING_VERSION
    ));
    assert!(
        !bundled_cli_derives_client_app(
            &cli,
            root.path(),
            true,
            QUALIFYING_VERSION,
            &NativeFileSystem,
            &NativeHost
        ),
        "claude.exe is the bundled name on Windows, so claude is not it"
    );
    Ok(())
}
