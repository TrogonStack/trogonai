#![cfg(target_os = "linux")]

//! Runs `tests/data/mount-plan-cases.txt` through the planner.
//!
//! The table is a list of blocks. A block starts with `case: <description>`
//! and holds `key: value` lines; `#` starts a comment. Values that hold
//! several items are separated by ` | `.
//!
//! - `bind: <guest> <host> <ro|rw> [mask]` and `symlink: <guest> <target>` describe
//!   the base layout.
//! - `view: host` makes the layout the host's own, and goes before the binds
//!   and paths it applies to.
//! - `cwd: <guest path>` sets the working directory the mandatory read-only
//!   paths are looked for from; without it every outermost writable root is
//!   searched.
//! - `dir:`, `file:`, `fifo:`, `socket:` and `link: <path> -> <target>` build
//!   the host tree, and
//!   `nocreate:` marks a host directory the worker cannot create entries in,
//!   and `unlistable:` one it cannot list, and `uninspectable:` a path whose
//!   status cannot be read.
//! - `deny_read`, `credential_deny_read`, `allow_read`, `own_allow_read`,
//!   `allow_write` and
//!   `deny_write` are the policy
//!   lists; an item is a literal path, or `glob:<pattern>`.
//! - `expect` is the exact, ordered operation list. `expect-error` is the
//!   error kind. `expect-suppressed`, `expect-placeholders`,
//!   `expect-unguarded` and `expect-tmp` (`writable` or `readonly`) are
//!   optional further checks.
//! - `expect-sources` lists `<dest> = <host path>` for the binds named, to show
//!   what a mount is taken from.
//! - `options: allow-git-config`, `options: disabled` and
//!   `options: search-depth=<n>` set the matching filesystem options.
//! - `text: <path> = <content>` creates a host file with content.
//! - The paths that build the host tree may spell a byte that is no text as
//!   `\xHH`, so a name or a link target can be one that no string holds.
//! - The mandatory read-only paths apply at the working directory, so the
//!   operations and placeholders they add are left out of the comparison
//!   unless the case says `mandatory: show`. `expect-mandatory` lists the
//!   protected paths and is always compared when present;
//!   `expect-mandatory-includes` only requires the listed paths to be among
//!   them.

use std::collections::BTreeMap;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use trogon_sandbox_contract::{DirectoryMarker, GlobPattern};
use trogon_sandbox_contract::{FilesystemOptions, FsPathEntry, FsPolicy, LiteralPath, MandatoryDenySearchDepth};
use trogon_sandbox_protocol::LiteralPathExt;
use trogon_sandbox_subprocess_worker::isolation::host_tree::InMemoryHostTree;
use trogon_sandbox_subprocess_worker::isolation::mount_plan::{
    FsPlanError, GuestLayout, MAX_MANDATORY_VISITS, MountAccess, MountOp, SuppressedReason, plan_mounts_with,
};

fn literal(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::default(),
    })
}

fn glob(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Glob {
        pattern: GlobPattern::new(raw)?,
    })
}

const CASES: &str = include_str!("data/mount-plan-cases.txt");

#[derive(Default)]
struct Case {
    name: String,
    fields: Vec<(String, String)>,
}

impl Case {
    fn all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> {
        self.fields
            .iter()
            .filter(move |(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn list(&self, key: &str) -> Option<Vec<String>> {
        let mut values = self.all(key).peekable();
        values.peek()?;
        Some(
            values
                .flat_map(|v| v.split(" | "))
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }
}

fn parse(table: &str) -> Result<Vec<Case>, Box<dyn std::error::Error>> {
    let mut cases: Vec<Case> = Vec::new();
    for line in table.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once(':').ok_or("key: value")?;
        let (key, value) = (key.trim(), value.trim());
        if key == "case" {
            cases.push(Case {
                name: value.to_string(),
                fields: Vec::new(),
            });
        } else if let Some(case) = cases.last_mut() {
            case.fields.push((key.to_string(), value.to_string()));
        }
    }
    Ok(cases)
}

fn os_path(spelled: &str) -> PathBuf {
    let raw = spelled.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut at = 0;
    while at < raw.len() {
        let byte = raw[at..]
            .strip_prefix(b"\\x")
            .and_then(|rest| rest.get(..2))
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match byte {
            Some(byte) => {
                bytes.push(byte);
                at += 4;
            }
            None => {
                bytes.push(raw[at]);
                at += 1;
            }
        }
    }
    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

fn entries(case: &Case, key: &str) -> Result<Vec<FsPathEntry>, Box<dyn std::error::Error>> {
    case.list(key)
        .unwrap_or_default()
        .iter()
        .map(|item| match item.strip_prefix("glob:") {
            Some(pattern) => glob(pattern),
            None => literal(item),
        })
        .collect()
}

fn run(case: &Case) -> Result<(), Box<dyn std::error::Error>> {
    let mut tree = InMemoryHostTree::new();
    let mut layout = GuestLayout::new();
    let mut bindings: Vec<(String, String)> = Vec::new();
    for (key, value) in &case.fields {
        match key.as_str() {
            "bind" => {
                let parts: Vec<&str> = value.split_whitespace().collect();
                let access = match parts[2] {
                    "rw" => MountAccess::ReadWrite,
                    _ => MountAccess::ReadOnly,
                };
                tree.dir(parts[1]);
                bindings.push((parts[0].to_string(), parts[1].to_string()));
                if parts.get(3) == Some(&"mask") {
                    layout.mask_file(LiteralPath::new(parts[0])?);
                }
                layout.bind(LiteralPath::new(parts[0])?, PathBuf::from(parts[1]), access);
            }
            "view" if value == "host" => {
                layout = GuestLayout::host_view();
            }
            "cwd" => {
                layout = std::mem::take(&mut layout).with_working_dir(LiteralPath::new(value)?);
            }
            "symlink" => {
                let (dest, target) = value.split_once(' ').ok_or("symlink")?;
                layout.symlink(LiteralPath::new(dest)?, target.to_string());
            }
            "dir" => {
                tree.dir(os_path(value));
            }
            "file" => {
                tree.file(os_path(value));
            }
            "fifo" | "socket" => {
                tree.special(os_path(value));
            }
            "text" => {
                let (path, content) = value.split_once(" = ").ok_or("text")?;
                tree.file_with(path, content);
            }
            "link" => {
                let (path, target) = value.split_once(" -> ").ok_or("link")?;
                tree.link(os_path(path), os_path(target));
            }
            "unlistable" => {
                tree.forbid_list(value);
            }
            "uninspectable" => {
                tree.forbid_stat(value);
            }
            "nocreate" => {
                tree.forbid_create(value);
            }
            _ => {}
        }
    }

    let policy = FsPolicy::new(
        entries(case, "deny_read")?,
        entries(case, "credential_deny_read")?,
        entries(case, "allow_read")?,
        entries(case, "own_allow_read")?,
        entries(case, "allow_write")?,
        entries(case, "deny_write")?,
    )?;

    let depth = case
        .all("options")
        .find_map(|v| v.strip_prefix("search-depth="))
        .map(|depth| -> Result<_, Box<dyn std::error::Error>> { Ok(MandatoryDenySearchDepth::new(depth.parse()?)?) })
        .transpose()?;
    let options = FilesystemOptions {
        allow_git_config: case.all("options").any(|v| v == "allow-git-config").into(),
        disabled: case.all("options").any(|v| v == "disabled").into(),
        mandatory_deny_search_depth: depth,
    };
    let outcome = plan_mounts_with(&policy, &layout, &tree, &options);
    if let Some(expected) = case.all("expect-error").next() {
        return match outcome {
            Err(error) if error.kind() == expected => Ok(()),
            Err(error) => Err(format!("expected error {expected}, got {}", error.kind()).into()),
            Ok(plan) => Err(format!("expected error {expected}, got plan {:?}", rendered(&plan)).into()),
        };
    }
    let plan = outcome.map_err(|error| format!("unexpected error: {error}"))?;

    let shows_mandatory = case.all("mandatory").any(|v| v == "show");
    let hidden = |dest: &LiteralPath| !shows_mandatory && plan.mandatory.contains(dest);
    let actual: Vec<String> = plan
        .ops
        .iter()
        .filter(|op| !hidden(op_dest(op)))
        .map(|op| op.to_string())
        .collect();
    let expected = case.list("expect").unwrap_or_default();
    if actual != expected {
        return Err(format!("ops differ\n  expected: {expected:?}\n    actual: {actual:?}").into());
    }

    let check = |key: &str, actual: Vec<String>| -> Result<(), Box<dyn std::error::Error>> {
        match case.list(key) {
            Some(expected) if expected != actual => {
                Err(format!("{key} differs\n  expected: {expected:?}\n    actual: {actual:?}").into())
            }
            _ => Ok(()),
        }
    };
    check(
        "expect-suppressed",
        plan.suppressed
            .iter()
            .map(|s| format!("{} {} {}", s.list, s.path, reason(s.reason)))
            .collect(),
    )?;
    check(
        "expect-sources",
        plan.ops
            .iter()
            .filter_map(|op| match op {
                MountOp::Bind { source, dest, .. } => Some(format!("{dest} = {}", source.path.display())),
                _ => None,
            })
            .filter(|line| {
                case.list("expect-sources").is_some_and(|expected| {
                    expected
                        .iter()
                        .any(|e| e.split(" = ").next() == line.split(" = ").next())
                })
            })
            .collect(),
    )?;
    let mut placeholders = Vec::new();
    for placeholder in &plan.placeholders {
        if !guest_paths(&bindings, placeholder).iter().any(hidden) {
            placeholders.push(placeholder.display().to_string());
        }
    }
    check("expect-placeholders", placeholders)?;
    check(
        "expect-unguarded",
        plan.unguarded.iter().map(|p| p.to_string()).collect(),
    )?;
    check(
        "expect-mandatory",
        plan.mandatory.iter().map(|p| p.to_string()).collect(),
    )?;
    if let Some(included) = case.list("expect-mandatory-includes") {
        let protected: Vec<String> = plan.mandatory.iter().map(|p| p.to_string()).collect();
        if let Some(missing) = included.iter().find(|path| !protected.contains(path)) {
            return Err(format!("{missing} is not protected: {protected:?}").into());
        }
    }
    check(
        "expect-tmp",
        vec![if plan.tmp_writable { "writable" } else { "readonly" }.to_string()],
    )?;
    Ok(())
}

fn op_dest(op: &MountOp) -> &LiteralPath {
    match op {
        MountOp::Bind { dest, .. }
        | MountOp::EmptyDir { dest }
        | MountOp::FileStub { dest }
        | MountOp::Symlink { dest, .. }
        | MountOp::RemountReadOnly { dest } => dest,
    }
}

fn guest_paths(bindings: &[(String, String)], host: &Path) -> Vec<LiteralPath> {
    bindings
        .iter()
        .filter_map(|(guest, base)| {
            let rest = host.strip_prefix(base).ok()?;
            LiteralPath::new(format!("{guest}/{}", rest.display())).ok()
        })
        .collect()
}

fn reason(reason: SuppressedReason) -> &'static str {
    match reason {
        SuppressedReason::ViaSymlink => "via-symlink",
        SuppressedReason::HiddenFromReads => "hidden",
        SuppressedReason::ReadOnlyMount => "read-only-mount",
        SuppressedReason::ProvidedBySandbox => "provided",
    }
}

fn rendered(plan: &trogon_sandbox_subprocess_worker::isolation::mount_plan::MountPlan) -> Vec<String> {
    plan.ops.iter().map(|op| op.to_string()).collect()
}

#[test]
fn mandatory_scan_fails_with_a_typed_error_past_the_visit_limit() -> Result<(), Box<dyn std::error::Error>> {
    let mut tree = InMemoryHostTree::new();
    tree.dir("/h/ws");
    for index in 0..=MAX_MANDATORY_VISITS {
        tree.file(format!("/h/ws/f{index}"));
    }
    let mut layout = GuestLayout::new();
    layout.bind(
        LiteralPath::new("/workspace")?,
        PathBuf::from("/h/ws"),
        MountAccess::ReadWrite,
    );
    let policy = FsPolicy::new(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace")?],
        Vec::new(),
    )?;
    let Err(error) = plan_mounts_with(&policy, &layout, &tree, &FilesystemOptions::default()) else {
        return Err("expected the scan limit to be reached".into());
    };
    assert_eq!(error.kind(), "mandatory-scan-too-large", "{error}");
    Ok(())
}

fn workspace_layout() -> Result<GuestLayout, Box<dyn std::error::Error>> {
    let mut layout = GuestLayout::new();
    layout.bind(
        LiteralPath::new("/workspace")?,
        PathBuf::from("/h/ws"),
        MountAccess::ReadWrite,
    );
    Ok(layout)
}

fn denied_by(tree: &InMemoryHostTree, deny_read: Vec<FsPathEntry>) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let policy = FsPolicy::new(deny_read, Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new())?;
    let plan = plan_mounts_with(&policy, &workspace_layout()?, tree, &FilesystemOptions::default())?;
    Ok(rendered(&plan))
}

#[test]
fn a_name_that_holds_a_line_terminator_is_matched() -> Result<(), Box<dyn std::error::Error>> {
    let mut tree = InMemoryHostTree::new();
    tree.dir("/h/ws/build");
    tree.file("/h/ws/build/a\nb.out");
    let ops = denied_by(&tree, vec![glob("/workspace/**/build/**")?])?;
    assert_eq!(
        ops,
        [
            "bind ro /workspace",
            "tmpfs /workspace/build",
            "remount-ro /workspace/build"
        ]
    );
    Ok(())
}

#[test]
fn a_chain_of_links_is_followed_to_its_end() -> Result<(), Box<dyn std::error::Error>> {
    let links = 300;
    let mut tree = InMemoryHostTree::new();
    for index in 0..=links {
        tree.dir(format!("/h/ws/d{index}"));
        tree.file(format!("/h/ws/d{index}/id.pem"));
    }
    for index in 0..links {
        tree.link(format!("/h/ws/d{index}/next"), format!("../d{}", index + 1));
    }
    let ops = denied_by(&tree, vec![glob("/workspace/d0/**/*.pem")?])?;
    let stubs = ops.iter().filter(|op| op.starts_with("stub ")).count();
    assert_eq!(stubs, links + 1, "{ops:?}");
    Ok(())
}

#[test]
fn packages_that_link_to_each_other_are_listed_once() -> Result<(), Box<dyn std::error::Error>> {
    let names: Vec<String> = (0..12).map(|index| format!("p{index}")).collect();
    let mut tree = InMemoryHostTree::new();
    for name in &names {
        tree.dir(format!("/h/ws/{name}/node_modules"));
        tree.file(format!("/h/ws/{name}/index.js"));
    }
    for from in &names {
        for to in &names {
            if from != to {
                tree.link(format!("/h/ws/{from}/node_modules/{to}"), format!("/workspace/{to}"));
            }
        }
    }
    let ops = denied_by(&tree, vec![glob("/workspace/**/index.js")?])?;
    let mut stubs: Vec<&String> = ops.iter().filter(|op| op.starts_with("stub ")).collect();
    stubs.sort();
    stubs.dedup();
    assert_eq!(stubs.len(), names.len(), "{ops:?}");
    Ok(())
}

#[test]
fn a_directory_reached_by_many_links_is_walked_under_each_name_the_pattern_tells_apart()
-> Result<(), Box<dyn std::error::Error>> {
    let mut tree = InMemoryHostTree::new();
    tree.dir("/h/ws/vault");
    tree.file("/h/ws/vault/id.pem");
    for index in 0..20 {
        tree.dir(format!("/h/ws/config{index}"));
        tree.link(format!("/h/ws/config{index}/secrets"), "../vault");
        tree.link(format!("/h/ws/config{index}/other"), "../vault");
    }
    let ops = denied_by(&tree, vec![glob("/workspace/**/secrets/*.pem")?])?;
    assert_eq!(ops, ["bind ro /workspace", "stub /workspace/vault/id.pem"]);
    Ok(())
}

#[test]
fn many_matches_of_one_pattern_are_all_masked_not_just_warned_about() -> Result<(), Box<dyn std::error::Error>> {
    let mut tree = InMemoryHostTree::new();
    tree.dir("/h/ws/keys");
    for index in 0..300 {
        tree.file(format!("/h/ws/keys/{index}.key"));
    }
    let ops = denied_by(&tree, vec![glob("/workspace/keys/*.key")?])?;
    let stubs = ops.iter().filter(|op| op.starts_with("stub ")).count();
    assert_eq!(stubs, 300, "{ops:?}");
    Ok(())
}

type ArgumentShares = (usize, usize, usize);

fn argument_shares(
    tree: &InMemoryHostTree,
    named: impl Fn(&str) -> Result<Vec<FsPathEntry>, Box<dyn std::error::Error>>,
    lists: usize,
) -> Result<Result<(), ArgumentShares>, Box<dyn std::error::Error>> {
    let policy = FsPolicy::new(
        if lists > 1 { named("r")? } else { Vec::new() },
        if lists > 2 { named("k")? } else { Vec::new() },
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace")?],
        if lists > 0 { named("c")? } else { Vec::new() },
    )?;
    Ok(
        match plan_mounts_with(&policy, &workspace_layout()?, tree, &FilesystemOptions::default()) {
            Ok(_) => Ok(()),
            Err(FsPlanError::TooManyArguments {
                total,
                mandatory,
                configured,
            }) => Err((total, mandatory, configured)),
            Err(_) => Err((0, 0, 0)),
        },
    )
}

#[test]
fn a_profile_past_the_arguments_bwrap_accepts_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let mut tree = InMemoryHostTree::new();
    tree.dir("/h/ws");
    let named = |stem: &str| -> Result<Vec<FsPathEntry>, Box<dyn std::error::Error>> {
        (0..1024)
            .map(|index| literal(&format!("/workspace/{stem}{index}")))
            .collect()
    };
    for stem in ["c", "r", "k"] {
        for index in 0..1024 {
            tree.file(format!("/h/ws/{stem}{index}"));
        }
    }
    assert_eq!(argument_shares(&tree, named, 2)?, Ok(()));
    let Err((total, mandatory, configured)) = argument_shares(&tree, named, 3)? else {
        return Err("expected the argument limit to be refused".into());
    };
    assert!(total > 8900, "{total}");
    assert_eq!(total, mandatory + configured);
    assert!(configured > 8800, "{configured}");
    assert!(mandatory < 100, "{mandatory}");
    Ok(())
}

#[test]
fn the_arguments_of_the_mandatory_denies_are_told_from_those_of_the_configuration()
-> Result<(), Box<dyn std::error::Error>> {
    let mut tree = InMemoryHostTree::new();
    tree.dir("/h/ws");
    for index in 0..1600 {
        tree.dir(format!("/h/ws/d{index}"));
        tree.file(format!("/h/ws/d{index}/.bashrc"));
    }
    let Err((total, mandatory, configured)) = argument_shares(&tree, |_| Ok(Vec::new()), 0)? else {
        return Err("expected the argument limit to be refused".into());
    };
    assert_eq!(total, mandatory + configured);
    assert!(mandatory > 8800, "{mandatory}");
    assert!(configured < 100, "{configured}");
    Ok(())
}

fn multiset(
    tree: &InMemoryHostTree,
    policy: &FsPolicy,
) -> Result<Result<Vec<String>, String>, Box<dyn std::error::Error>> {
    Ok(
        plan_mounts_with(policy, &workspace_layout()?, tree, &FilesystemOptions::default())
            .map(|plan| {
                let mut ops = rendered(&plan);
                ops.sort();
                ops
            })
            .map_err(|error| error.kind().to_string()),
    )
}

fn denies_in_both_orders(
    tree: &InMemoryHostTree,
    allow: &[&str],
    deny: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let policy = |denies: Vec<&str>| -> Result<FsPolicy, Box<dyn std::error::Error>> {
        Ok(FsPolicy::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            allow.iter().map(|path| literal(path)).collect::<Result<_, _>>()?,
            denies.into_iter().map(literal).collect::<Result<_, _>>()?,
        )?)
    };
    let forward = multiset(tree, &policy(deny.to_vec())?)?;
    let mut reversed_deny = deny.to_vec();
    reversed_deny.reverse();
    let reversed = multiset(tree, &policy(reversed_deny)?)?;
    assert_eq!(forward, reversed);
    assert!(forward.is_ok(), "{forward:?}");
    Ok(())
}

#[test]
fn a_file_named_directly_and_through_a_link_is_bound_in_either_order() -> Result<(), Box<dyn std::error::Error>> {
    let mut tree = InMemoryHostTree::new();
    tree.dir("/h/ws/area");
    tree.file("/h/ws/area/settings.json");
    tree.link("/h/ws/area/link", "/workspace/area/settings.json");
    denies_in_both_orders(
        &tree,
        &["/workspace/area"],
        &[
            "/workspace/area",
            "/workspace/area/settings.json",
            "/workspace/area/link",
        ],
    )?;
    Ok(())
}

#[test]
fn a_directory_named_through_a_chain_that_leaves_the_allowed_area_is_bound_in_either_order()
-> Result<(), Box<dyn std::error::Error>> {
    let mut tree = InMemoryHostTree::new();
    tree.dir("/h/ws/area/repo/.git/hooks");
    tree.dir("/h/ws/outside");
    tree.link("/h/ws/outside/to-hooks", "/workspace/area/repo/.git/hooks");
    tree.link("/h/ws/area/away", "/workspace/outside");
    denies_in_both_orders(
        &tree,
        &["/workspace/area"],
        &[
            "/workspace/area",
            "/workspace/area/repo/.git/hooks",
            "/workspace/area/away/to-hooks",
        ],
    )?;
    Ok(())
}

#[test]
fn a_deny_set_with_no_covering_directory_is_left_alone_in_either_order() -> Result<(), Box<dyn std::error::Error>> {
    let mut tree = InMemoryHostTree::new();
    tree.dir("/h/ws/area");
    tree.file("/h/ws/area/settings.json");
    tree.link("/h/ws/area/link", "/workspace/area/settings.json");
    denies_in_both_orders(
        &tree,
        &["/workspace/area"],
        &["/workspace/area/settings.json", "/workspace/area/link"],
    )?;
    Ok(())
}

struct Mulberry32(u32);

impl Mulberry32 {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = (self.0 ^ (self.0 >> 15)).wrapping_mul(1 | self.0);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() * items.len() as f64) as usize]
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() * n as f64) as usize
    }
}

#[test]
fn a_deny_list_and_its_reverse_give_the_same_mounts_over_random_trees() -> Result<(), Box<dyn std::error::Error>> {
    const NAMES: [&str; 14] = [
        ".git", "hooks", "config", "src", "sub", "deep", "a", "b", ".env", ".bashrc", "link", "loop", "stale",
        "file.txt",
    ];
    const SEEDS: u32 = 120;
    let mut compared = 0;
    for seed in 800..800 + SEEDS {
        let mut rng = Mulberry32(seed);
        let mut tree = InMemoryHostTree::new();
        let mut dirs = vec![String::new()];
        let mut used: Vec<String> = vec![String::new()];
        let mut files: Vec<String> = Vec::new();
        let mut links: Vec<String> = Vec::new();
        for _ in 0..3 + rng.below(8) {
            let path = format!("{}/{}", rng.pick(&dirs), rng.pick(&NAMES));
            if !used.contains(&path) {
                tree.dir(format!("/h/ws{path}"));
                used.push(path.clone());
                dirs.push(path);
            }
        }
        for _ in 0..3 + rng.below(8) {
            let path = format!("{}/{}", rng.pick(&dirs), rng.pick(&NAMES));
            if !used.contains(&path) {
                tree.file(format!("/h/ws{path}"));
                used.push(path.clone());
                files.push(path);
            }
        }
        for index in 0..1 + rng.below(5) {
            let link = format!("{}/l{index}", rng.pick(&dirs));
            if used.contains(&link) {
                continue;
            }
            let kind = rng.next();
            let target = if kind < 0.35 {
                format!("/workspace{}", rng.pick(&dirs))
            } else if kind < 0.6 && !files.is_empty() {
                format!("/workspace{}", rng.pick(&files))
            } else if kind < 0.85 {
                "/workspace/nowhere".to_string()
            } else {
                format!("/workspace{link}")
            };
            tree.link(format!("/h/ws{link}"), target);
            used.push(link.clone());
            links.push(link);
        }
        let everything: Vec<String> = dirs
            .iter()
            .filter(|path| !path.is_empty())
            .chain(&files)
            .chain(&links)
            .map(|path| format!("/workspace{path}"))
            .chain(["/workspace/absent0".to_string(), "/workspace/gone/leaf".to_string()])
            .collect();
        let globs = ["/*.txt", "/**/*.env", "/[ab].*", "/?.txt", "/**/hooks"];
        let mut deny_write = Vec::new();
        for _ in 0..rng.below(6) {
            deny_write.push(literal(rng.pick(&everything))?);
        }
        if deny_write.len() < 2 {
            continue;
        }
        let allow_write = if rng.next() < 0.15 {
            vec![literal("/workspace")?]
        } else {
            (0..1 + rng.below(2))
                .map(|_| {
                    let dir = rng.pick(&dirs);
                    literal(&format!("/workspace{dir}"))
                })
                .collect::<Result<_, _>>()?
        };
        let mut deny_read = Vec::new();
        for _ in 0..rng.below(3) {
            let path = rng.pick(&everything);
            deny_read.push(if rng.next() < 0.3 {
                glob(&format!("{path}{}", rng.pick(&globs)))?
            } else {
                literal(path)?
            });
        }
        let policy = |deny_write: Vec<FsPathEntry>, deny_read: Vec<FsPathEntry>| {
            FsPolicy::new(
                deny_read,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                allow_write.clone(),
                deny_write,
            )
        };
        let forward = multiset(&tree, &policy(deny_write.clone(), deny_read.clone())?)?;
        deny_write.reverse();
        deny_read.reverse();
        let reversed = multiset(&tree, &policy(deny_write, deny_read)?)?;
        assert_eq!(forward, reversed, "seed {seed}");
        compared += 1;
    }
    assert!(compared > SEEDS / 2, "{compared}");
    Ok(())
}

fn area_tree() -> InMemoryHostTree {
    let mut tree = InMemoryHostTree::new();
    tree.dir("/h/ws/area/proj/sub/deeper");
    tree.dir("/h/ws/area/other");
    tree.dir("/h/ws/area/proj2/in");
    tree.link("/h/ws/to-area", "/workspace/area");
    tree.link("/h/ws/to-proj", "/workspace/area/proj");
    tree.link("/h/ws/to-sub", "/workspace/area/proj/sub");
    tree
}

fn under(names: &[&str]) -> Result<Vec<FsPathEntry>, Box<dyn std::error::Error>> {
    names
        .iter()
        .map(|name| literal(&format!("/workspace/{name}")))
        .collect()
}

fn area_plan(
    tree: &InMemoryHostTree,
    deny_write: &[&str],
    deny_read: &[&str],
    allow_write: &[&str],
) -> Result<Vec<MountOp>, Box<dyn std::error::Error>> {
    let policy = FsPolicy::new(
        under(deny_read)?,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        under(allow_write)?,
        under(deny_write)?,
    )?;
    Ok(plan_mounts_with(&policy, &workspace_layout()?, tree, &FilesystemOptions::default())?.ops)
}

fn deciding_mount<'a>(ops: &'a [MountOp], path: &str) -> Option<&'a MountOp> {
    let path = LiteralPath::new(format!("/workspace/{path}")).ok()?;
    ops.iter().rev().find(|op| path.is_within(op_dest(op)))
}

fn is_read_only(op: &MountOp) -> bool {
    match op {
        MountOp::Bind { access, .. } => *access == MountAccess::ReadOnly,
        MountOp::FileStub { .. } | MountOp::EmptyDir { .. } | MountOp::RemountReadOnly { .. } => true,
        MountOp::Symlink { .. } => false,
    }
}

#[test]
fn a_write_deny_and_a_write_grant_hold_by_where_each_lies() -> Result<(), Box<dyn std::error::Error>> {
    const A: &str = "area";
    const P: &str = "area/proj";
    const S: &str = "area/proj/sub";
    const O: &str = "area/other";
    const P2: &str = "area/proj2";
    type Row = (
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
        &'static [&'static str],
        &'static [&'static str],
    );
    let placements: [Row; 12] = [
        ("inside it", &[P], &[A], &[S], &[O]),
        ("equal to it", &[P], &[P], &[S], &[]),
        ("above it", &[P], &[S], &[S], &[]),
        ("two levels above it", &[A], &[S], &[S], &[]),
        ("between two of them", &[P], &[A, S], &[S], &[O]),
        ("above one and beside another", &[P], &[O, S], &[S], &[O]),
        ("above two of them", &[A], &[O, S], &[O, S], &[]),
        ("beside it", &[O], &[S], &[], &[S]),
        ("beside it, its name a prefix", &[P], &[P2], &[], &[P2]),
        ("above it, through a symlink", &["to-proj"], &[S], &[S], &[]),
        (
            "above it, through a symlinked parent",
            &["to-area/proj"],
            &[S],
            &[S],
            &[],
        ),
        ("above what it is a symlink to", &[P], &["to-sub"], &[S], &[]),
    ];
    let tree = area_tree();
    for (place, deny, allow, read_only, writable) in placements {
        let ops = area_plan(&tree, deny, &[], allow)?;
        for dir in read_only {
            let op = deciding_mount(&ops, dir).ok_or("a deciding mount")?;
            assert!(is_read_only(op), "{place}: {dir} is decided by {op}\n{ops:?}");
        }
        for dir in writable {
            let op = deciding_mount(&ops, dir).ok_or("a deciding mount")?;
            assert!(!is_read_only(op), "{place}: {dir} is decided by {op}\n{ops:?}");
        }
    }
    Ok(())
}

#[test]
fn an_absent_deny_path_is_laid_only_where_something_writable_holds_it() -> Result<(), Box<dyn std::error::Error>> {
    const A: &str = "area";
    const P: &str = "area/proj";
    const S: &str = "area/proj/sub";
    const D: &str = "area/proj/sub/deeper";
    const O: &str = "area/other";
    const P2: &str = "area/proj2";
    const P2_IN: &str = "area/proj2/in";
    type Row = (
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
        &'static str,
        bool,
    );
    let holders: [Row; 7] = [
        ("a covered write path", &[P], &[S], S, false),
        ("two covered write paths", &[P], &[S, D], D, false),
        ("one covered twice", &[A, P], &[S], S, false),
        ("one covered, a prefix beside it", &[P2], &[P, P2_IN], P2_IN, false),
        ("one beside the deny", &[P], &[S, O], O, true),
        ("one beside it, its name a prefix", &[P], &[S, P2], P2, true),
        ("one above the deny, beside it", &[S], &[P], P, true),
    ];
    let tree = area_tree();
    for read_denied in [&[][..], &[A][..]] {
        for (holder, deny, allow, dir, writable) in holders {
            let absent = format!("{dir}/.secret");
            let mut denies = deny.to_vec();
            denies.push(&absent);
            let ops = area_plan(&tree, &denies, read_denied, allow)?;
            let laid = ops
                .iter()
                .filter(
                    |op| matches!(op, MountOp::FileStub { dest } if dest.as_str() == format!("/workspace/{absent}")),
                )
                .count();
            let reads = if read_denied.is_empty() { "no" } else { "a" };
            assert_eq!(
                laid,
                usize::from(writable && read_denied.is_empty()),
                "{holder}, {reads} read deny: {ops:?}"
            );
            let op = deciding_mount(&ops, &absent).ok_or("a deciding mount")?;
            assert!(is_read_only(op), "{holder}, {reads} read deny: {op}\n{ops:?}");
        }
    }
    Ok(())
}

#[test]
fn a_write_deny_directory_above_the_working_directory_lays_no_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    const A: &str = "area";
    const P: &str = "area/proj";
    const S: &str = "area/proj/sub";
    let absent = "area/proj/sub/.secret";
    let rows: [(&str, &[&str]); 4] = [
        ("no read deny", &[]),
        ("a read deny at the write deny", &[A]),
        ("a read deny above the write deny", &[""]),
        ("a read deny between the two", &[P]),
    ];
    let tree = area_tree();
    for (place, read_denied) in rows {
        let policy = FsPolicy::new(
            read_denied
                .iter()
                .map(|name| literal(&format!("/workspace/{name}")))
                .collect::<Result<_, _>>()?,
            Vec::new(),
            under(&[S])?,
            Vec::new(),
            under(&[S])?,
            under(&[A, absent])?,
        )?;
        let layout = workspace_layout()?.with_working_dir(LiteralPath::new(format!("/workspace/{S}"))?);
        let plan = plan_mounts_with(&policy, &layout, &tree, &FilesystemOptions::default())
            .map_err(|error| format!("{place}: {error}"))?;
        assert!(plan.placeholders.is_empty(), "{place}: {:?}", plan.placeholders);
        let stubs: Vec<_> = plan
            .ops
            .iter()
            .filter(|op| matches!(op, MountOp::FileStub { .. }))
            .collect();
        assert!(stubs.is_empty(), "{place}: {stubs:?}");
        let op = deciding_mount(&plan.ops, absent).ok_or("a deciding mount")?;
        assert!(is_read_only(op), "{place}: {op}");
    }
    Ok(())
}

#[test]
fn mount_plan_cases() -> Result<(), Box<dyn std::error::Error>> {
    let cases = parse(CASES)?;
    assert!(cases.len() > 20, "the table must not be silently empty");
    let mut failures = BTreeMap::new();
    for case in &cases {
        if let Err(message) = run(case) {
            failures.insert(case.name.clone(), message);
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        cases.len(),
        failures
            .iter()
            .map(|(name, message)| format!("- {name}\n  {message}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(())
}
