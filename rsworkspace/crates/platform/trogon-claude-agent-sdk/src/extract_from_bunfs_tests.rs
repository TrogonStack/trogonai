use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    error::Error,
    ffi::{OsStr, OsString},
    fs::{self, DirBuilder, File, Metadata, OpenOptions, Permissions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

const GOLDENS: &str = include_str!("../tests/fixtures/extract_from_bunfs/goldens.json");
const CONTENT: &[u8] = b"claude-binary-bytes\n";
const HASH: &str = "910b1eae503c8208";
const PID: u32 = 4242;
const RANDOM: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
const RANDOM_HEX: &str = "0102030405060708";

enum Step {
    Real,
    Fail(i32),
    FailAndPlant(i32, PathBuf, Vec<u8>),
}

#[derive(Default)]
struct Script {
    renames: VecDeque<Step>,
    removes: VecDeque<Option<i32>>,
    ids: VecDeque<i128>,
}

/// The native host with the process facts pinned and individual calls scripted to fail.
struct Host {
    system: OperatingSystem,
    uid: Option<u32>,
    env: HashMap<String, String>,
    temp: PathBuf,
    script: RefCell<Script>,
    fail_stat: Option<PathBuf>,
    mode_error: Option<i32>,
    file_mode_error: Option<i32>,
    warnings: RefCell<Vec<String>>,
}

impl Host {
    fn new(system: OperatingSystem, uid: Option<u32>) -> Self {
        Self {
            system,
            uid,
            env: HashMap::new(),
            temp: PathBuf::from("/nonexistent-temp-dir"),
            script: RefCell::new(Script::default()),
            fail_stat: None,
            mode_error: None,
            file_mode_error: None,
            warnings: RefCell::new(Vec::new()),
        }
    }
}

impl ReadFile for Host {
    type Error = io::Error;

    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        NativeExtractHost.read_file(path)
    }
}

impl CreateDirectory for Host {
    type Error = io::Error;

    fn create_directory(&self, builder: &DirBuilder, path: &Path) -> io::Result<()> {
        NativeExtractHost.create_directory(builder, path)
    }
}

impl OpenFile for Host {
    type Error = io::Error;

    fn open(&self, options: &OpenOptions, path: &Path) -> io::Result<File> {
        NativeExtractHost.open(options, path)
    }
}

impl RemoveFile for Host {
    type Error = io::Error;

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        let scripted = self.script.borrow_mut().removes.pop_front().flatten();
        match scripted {
            Some(code) => Err(io::Error::from_raw_os_error(code)),
            None => NativeExtractHost.remove_file(path),
        }
    }
}

impl StatLink for Host {
    fn stat_link(&self, path: &Path) -> io::Result<Metadata> {
        if self.fail_stat.as_deref() == Some(path) {
            return Err(io::Error::from_raw_os_error(libc::EACCES));
        }
        NativeExtractHost.stat_link(path)
    }
}

impl RenamePath for Host {
    fn rename_path(&self, from: &Path, to: &Path) -> io::Result<()> {
        let step = self.script.borrow_mut().renames.pop_front().unwrap_or(Step::Real);
        match step {
            Step::Real => NativeExtractHost.rename_path(from, to),
            Step::Fail(code) => Err(io::Error::from_raw_os_error(code)),
            Step::FailAndPlant(code, path, bytes) => {
                fs::write(path, bytes)?;
                Err(io::Error::from_raw_os_error(code))
            }
        }
    }
}

impl ChangeMode for Host {
    fn change_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        match self.mode_error {
            Some(code) => Err(io::Error::from_raw_os_error(code)),
            None => NativeExtractHost.change_mode(path, mode),
        }
    }

    fn change_file_mode(&self, file: &File, mode: u32) -> io::Result<()> {
        match self.file_mode_error {
            Some(code) => Err(io::Error::from_raw_os_error(code)),
            None => NativeExtractHost.change_file_mode(file, mode),
        }
    }
}

impl ProcessId for Host {
    fn process_id(&self) -> u32 {
        PID
    }
}

impl RandomBytes for Host {
    fn random_bytes(&self) -> [u8; 8] {
        RANDOM
    }
}

impl FileIdentity for Host {
    fn file_id(&self, metadata: &Metadata) -> i128 {
        let scripted = self.script.borrow_mut().ids.pop_front();
        scripted.unwrap_or_else(|| NativeExtractHost.file_id(metadata))
    }
}

impl EmitWarning for Host {
    fn emit_warning(&self, message: &str) {
        self.warnings.borrow_mut().push(message.to_owned());
    }
}

impl EnvironmentVariable for Host {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        self.env.get(key.to_str()?).map(OsString::from)
    }
}

impl TemporaryDirectory for Host {
    fn temp_dir(&self) -> PathBuf {
        self.temp.clone()
    }
}

impl HostIdentity for Host {
    fn operating_system(&self) -> OperatingSystem {
        self.system.clone()
    }

    fn username(&self) -> Option<String> {
        None
    }
}

impl RealUserId for Host {
    fn real_user_id(&self) -> Option<u32> {
        self.uid
    }
}

fn real_uid() -> Result<u32, Box<dyn Error>> {
    Ok(NativeExtractHost.real_user_id().ok_or("no uid")?)
}

fn wrapped(message: &str) -> String {
    format!("{FAILURE_PREFIX}{message}{FAILURE_SUFFIX}")
}

fn listing(path: &Path) -> Result<Vec<String>, io::Error> {
    let mut names = Vec::new();
    for entry in fs::read_dir(path)? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key].as_str().ok_or_else(|| format!("missing {key}"))
}

fn big() -> String {
    format!("{}tail", "abcdefgh".repeat(2 * (1 << 20) / 8))
}

fn body(text: &str) -> String {
    match text {
        "@big" => big(),
        "@big_late" => format!("{}tall", &big()[..big().len() - 4]),
        other => other.to_owned(),
    }
}

struct Vars {
    root: String,
    hash: String,
    uid: u32,
}

impl Vars {
    fn resolve(&self, text: &str) -> String {
        text.replace("{root}", &self.root)
            .replace("{hash}", &self.hash)
            .replace("{fake_uid}", &(self.uid + 1).to_string())
            .replace("{uid}", &self.uid.to_string())
            .replace("{pid}", &PID.to_string())
            .replace("{random}", RANDOM_HEX)
    }
}

fn apply(step: &Value, vars: &Vars) -> TestResult {
    let target = PathBuf::from(vars.resolve(text(step, "path")?));
    let parent = target.parent().ok_or("parent")?;
    match text(step, "op")? {
        "mkdir" => {
            fs::create_dir_all(&target)?;
            let mode = u32::try_from(step["mode"].as_u64().unwrap_or(0o700))?;
            fs::set_permissions(&target, Permissions::from_mode(mode))?;
        }
        "write" => {
            fs::create_dir_all(parent)?;
            fs::write(&target, body(text(step, "content")?))?;
        }
        "symlink" => {
            fs::create_dir_all(parent)?;
            std::os::unix::fs::symlink(vars.resolve(text(step, "target")?), &target)?;
        }
        "link" => {
            fs::hard_link(vars.resolve(text(step, "target")?), &target)?;
        }
        other => return Err(format!("unknown setup op {other}").into()),
    }
    Ok(())
}

fn check_files(id: &str, case: &Value, vars: &Vars) -> TestResult {
    for (key, expected) in case["files"].as_object().ok_or("files")? {
        let path = PathBuf::from(vars.resolve(key));
        let kind = text(expected, "kind")?;
        let found = fs::symlink_metadata(&path);
        if kind == "missing" {
            assert!(found.is_err(), "{id}: {key} should be missing");
            continue;
        }
        let found = found?;
        let want_mode = u32::try_from(expected["mode"].as_u64().ok_or("mode")?)?;
        match kind {
            "symlink" => assert!(found.file_type().is_symlink(), "{id}: {key} kind"),
            "directory" => {
                assert!(found.is_dir(), "{id}: {key} kind");
                let want: Vec<String> = expected["listing"]
                    .as_array()
                    .ok_or("listing")?
                    .iter()
                    .map(|name| name.as_str().map(|name| vars.resolve(name)).ok_or("name"))
                    .collect::<Result<_, _>>()?;
                assert_eq!(listing(&path)?, want, "{id}: {key} listing");
                if want_mode == 0o700 {
                    assert_eq!(found.mode() & 0o777, want_mode, "{id}: {key} mode");
                }
            }
            _ => {
                assert!(found.is_file(), "{id}: {key} kind");
                assert_eq!(
                    found.nlink(),
                    expected["nlink"].as_u64().ok_or("nlink")?,
                    "{id}: {key} nlink"
                );
                let want = body(text(expected, "content")?).into_bytes();
                assert!(fs::read(&path)? == want, "{id}: {key} content");
                if want_mode == 0o755 {
                    assert_eq!(found.mode() & 0o777, want_mode, "{id}: {key} mode");
                }
            }
        }
    }
    Ok(())
}

fn run_case(case: &Value) -> TestResult {
    let id = text(case, "id")?;
    let dir = tempfile::tempdir()?;
    let uid = real_uid()?;
    let vars = Vars {
        root: dir.path().to_string_lossy().into_owned(),
        hash: text(case, "hash")?.to_owned(),
        uid,
    };
    let name = text(case, "name")?;
    let embedded = vars.resolve(text(case, "embedded")?);
    if embedded == format!("{}/$bunfs/root/{name}", vars.root) {
        fs::create_dir_all(Path::new(&embedded).parent().ok_or("parent")?)?;
        fs::write(&embedded, body(text(case, "content")?))?;
    }
    for step in case["setup"].as_array().ok_or("setup")? {
        apply(step, &vars)?;
    }
    let system = if text(case, "platform")? == "win32" {
        OperatingSystem::Win32
    } else {
        OperatingSystem::Linux
    };
    let uid_for_host = match text(case, "uid")? {
        "none" => None,
        "fake" => Some(uid + 1),
        _ => Some(uid),
    };
    let mut host = Host::new(system, uid_for_host);
    for (key, value) in case["env"].as_object().ok_or("env")? {
        let value = vars.resolve(value.as_str().ok_or("env value")?);
        if key == "TMPDIR" {
            host.temp = PathBuf::from(&value);
        }
        host.env.insert(key.clone(), value);
    }
    let returns = case["returns"].as_array().ok_or("returns")?;
    assert_eq!(
        returns.len() as u64,
        case["calls"].as_u64().ok_or("calls")?,
        "{id}: calls"
    );
    for want in returns {
        let got = extract_from_bunfs_with(Path::new(&embedded), &host);
        assert_eq!(
            got.to_string_lossy(),
            vars.resolve(want.as_str().ok_or("return")?),
            "{id}: return"
        );
    }
    let warnings: Vec<String> = case["warnings"]
        .as_array()
        .ok_or("warnings")?
        .iter()
        .map(|warning| warning.as_str().map(|warning| vars.resolve(warning)).ok_or("warning"))
        .collect::<Result<_, _>>()?;
    assert_eq!(*host.warnings.borrow(), warnings, "{id}: warnings");
    check_files(id, case, &vars)
}

#[test]
fn every_recorded_upstream_case_is_reproduced() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let cases = goldens["cases"].as_array().ok_or("cases")?;
    assert!(cases.len() >= 27);
    for case in cases {
        run_case(case)?;
    }
    Ok(())
}

#[test]
fn directories_are_named_after_the_first_sixteen_hex_digits_of_the_sha256() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    assert_eq!(text(&goldens, "hash")?, HASH);
    assert_eq!(hex(Sha256::digest(CONTENT).iter().copied().take(8)), HASH);
    assert_eq!(
        hex(Sha256::digest(big().as_bytes()).iter().copied().take(8)),
        text(&goldens, "big_hash")?
    );
    Ok(())
}

#[test]
fn the_errno_table_carries_libuv_text_for_each_code() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let errors = goldens["errors"].as_object().ok_or("errors")?;
    assert_eq!(errors.len(), ERRNO_TABLE.len());
    for (_, name, message) in ERRNO_TABLE {
        assert_eq!(errors[*name].as_str(), Some(*message), "{name}");
    }
    Ok(())
}

struct Sandbox {
    dir: tempfile::TempDir,
    host: Host,
    embedded: PathBuf,
    uid: u32,
}

impl Sandbox {
    fn new() -> Result<Self, Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let uid = real_uid()?;
        let embedded = dir.path().join("$bunfs/root/claude");
        fs::create_dir_all(embedded.parent().ok_or("parent")?)?;
        fs::write(&embedded, CONTENT)?;
        let mut host = Host::new(OperatingSystem::Linux, Some(uid));
        host.env.insert(
            "CLAUDE_CODE_TMPDIR".to_owned(),
            dir.path().join("tmp").to_string_lossy().into_owned(),
        );
        Ok(Self {
            dir,
            host,
            embedded,
            uid,
        })
    }

    fn base(&self) -> PathBuf {
        self.dir.path().join(format!("tmp/claude-agent-sdk-{}", self.uid))
    }

    fn hash_dir(&self) -> PathBuf {
        self.base().join(format!("claude-agent-sdk-{HASH}"))
    }

    fn target(&self) -> PathBuf {
        self.hash_dir().join("claude")
    }

    fn temporary(&self) -> PathBuf {
        self.hash_dir().join(format!("claude.tmp.{PID}.{RANDOM_HEX}"))
    }

    fn run(&self) -> PathBuf {
        extract_from_bunfs_with(&self.embedded, &self.host)
    }

    fn warnings(&self) -> Vec<String> {
        self.host.warnings.borrow().clone()
    }

    fn script(&self, steps: Vec<Step>) {
        self.host.script.borrow_mut().renames = steps.into();
    }
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn a_rename_failure_that_is_not_a_name_clash_is_reported_and_the_temporary_file_removed() -> TestResult {
    let sandbox = Sandbox::new()?;
    sandbox.script(vec![Step::Fail(libc::EXDEV)]);
    assert_eq!(sandbox.run(), sandbox.embedded);
    assert_eq!(
        sandbox.warnings(),
        vec![wrapped(&format!(
            "EXDEV: cross-device link not permitted, rename '{}' -> '{}'",
            display(&sandbox.temporary()),
            display(&sandbox.target())
        ))]
    );
    assert!(listing(&sandbox.hash_dir())?.is_empty());
    Ok(())
}

#[test]
fn a_name_clash_removes_the_other_copy_and_renames_again() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::create_dir_all(sandbox.hash_dir())?;
    fs::write(sandbox.target(), "short")?;
    sandbox.script(vec![Step::Fail(libc::EEXIST)]);
    assert_eq!(sandbox.run(), sandbox.target());
    assert!(sandbox.warnings().is_empty());
    assert_eq!(fs::read(sandbox.target())?, CONTENT);
    assert_eq!(listing(&sandbox.hash_dir())?, ["claude"]);
    Ok(())
}

#[test]
fn a_name_clash_with_nothing_to_remove_still_renames_again() -> TestResult {
    let sandbox = Sandbox::new()?;
    sandbox.script(vec![Step::Fail(libc::EPERM)]);
    assert_eq!(sandbox.run(), sandbox.target());
    assert!(sandbox.warnings().is_empty());
    assert_eq!(fs::read(sandbox.target())?, CONTENT);
    Ok(())
}

#[test]
fn a_name_clash_whose_other_copy_cannot_be_removed_reports_the_unlink() -> TestResult {
    let sandbox = Sandbox::new()?;
    sandbox.script(vec![Step::Fail(libc::EBUSY)]);
    sandbox.host.script.borrow_mut().removes = VecDeque::from([Some(libc::EACCES)]);
    assert_eq!(sandbox.run(), sandbox.embedded);
    assert_eq!(
        sandbox.warnings(),
        vec![wrapped(&format!(
            "EACCES: permission denied, unlink '{}'",
            display(&sandbox.target())
        ))]
    );
    assert!(listing(&sandbox.hash_dir())?.is_empty());
    Ok(())
}

#[test]
fn a_second_rename_failure_reports_the_first_one() -> TestResult {
    let sandbox = Sandbox::new()?;
    sandbox.script(vec![Step::Fail(libc::EEXIST), Step::Fail(libc::EACCES)]);
    assert_eq!(sandbox.run(), sandbox.embedded);
    assert_eq!(
        sandbox.warnings(),
        vec![wrapped(&format!(
            "EEXIST: file already exists, rename '{}' -> '{}'",
            display(&sandbox.temporary()),
            display(&sandbox.target())
        ))]
    );
    Ok(())
}

#[test]
fn an_identical_copy_that_appears_during_the_rename_is_reused() -> TestResult {
    let sandbox = Sandbox::new()?;
    sandbox.script(vec![Step::FailAndPlant(
        libc::EEXIST,
        sandbox.target(),
        CONTENT.to_vec(),
    )]);
    assert_eq!(sandbox.run(), sandbox.target());
    assert!(sandbox.warnings().is_empty());
    assert_eq!(listing(&sandbox.hash_dir())?, ["claude"]);
    Ok(())
}

#[test]
fn an_identical_copy_that_wins_the_second_rename_is_reused() -> TestResult {
    let sandbox = Sandbox::new()?;
    sandbox.script(vec![
        Step::Fail(libc::EEXIST),
        Step::FailAndPlant(libc::EACCES, sandbox.target(), CONTENT.to_vec()),
    ]);
    assert_eq!(sandbox.run(), sandbox.target());
    assert!(sandbox.warnings().is_empty());
    assert_eq!(listing(&sandbox.hash_dir())?, ["claude"]);
    Ok(())
}

#[test]
fn a_real_rename_step_is_a_plain_rename() -> TestResult {
    let sandbox = Sandbox::new()?;
    sandbox.script(vec![Step::Real]);
    assert_eq!(sandbox.run(), sandbox.target());
    Ok(())
}

#[test]
fn a_failed_permission_change_on_the_new_file_is_reported() -> TestResult {
    let mut sandbox = Sandbox::new()?;
    sandbox.host.file_mode_error = Some(libc::EPERM);
    assert_eq!(sandbox.run(), sandbox.embedded);
    assert_eq!(
        sandbox.warnings(),
        vec![wrapped("EPERM: operation not permitted, fchmod")]
    );
    assert!(listing(&sandbox.hash_dir())?.is_empty());
    Ok(())
}

#[test]
fn writing_to_a_file_that_is_not_writable_is_reported() -> TestResult {
    let sandbox = Sandbox::new()?;
    let readonly = File::open(&sandbox.embedded)?;
    let target = sandbox.target();
    let message = place(&sandbox.host, readonly, CONTENT, &sandbox.temporary(), &target).err();
    let message = message.ok_or("expected a failure")?;
    assert!(message.contains("os error 9"), "{message}");
    assert!(message.ends_with(", write"), "{message}");
    Ok(())
}

#[test]
fn a_hash_directory_name_taken_by_a_file_is_reported_as_a_failed_mkdir() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::create_dir_all(sandbox.base())?;
    fs::write(sandbox.hash_dir(), "file")?;
    assert_eq!(sandbox.run(), sandbox.embedded);
    assert_eq!(
        sandbox.warnings(),
        vec![wrapped(&format!(
            "EEXIST: file already exists, mkdir '{}'",
            display(&sandbox.hash_dir())
        ))]
    );
    Ok(())
}

#[test]
fn a_failed_lstat_of_the_hash_directory_is_reported() -> TestResult {
    let mut sandbox = Sandbox::new()?;
    sandbox.host.fail_stat = Some(sandbox.hash_dir());
    assert_eq!(sandbox.run(), sandbox.embedded);
    assert_eq!(
        sandbox.warnings(),
        vec![wrapped(&format!(
            "EACCES: permission denied, lstat '{}'",
            display(&sandbox.hash_dir())
        ))]
    );
    Ok(())
}

#[test]
fn a_taken_temporary_name_is_never_written_through() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::create_dir_all(sandbox.hash_dir())?;
    fs::write(sandbox.temporary(), "planted")?;
    assert_eq!(sandbox.run(), sandbox.embedded);
    assert_eq!(
        sandbox.warnings(),
        vec![wrapped(&format!(
            "EEXIST: file already exists, open '{}'",
            display(&sandbox.temporary())
        ))]
    );
    assert_eq!(fs::read(sandbox.temporary())?, b"planted");
    Ok(())
}

#[test]
fn the_temporary_directory_comes_from_the_environment_then_the_platform() {
    let mut host = Host::new(OperatingSystem::Darwin, Some(1));
    host.temp = PathBuf::from("/os/temp");
    assert_eq!(temp_root(&host), "/tmp");
    host.system = OperatingSystem::Linux;
    assert_eq!(temp_root(&host), "/os/temp");
    host.env.insert("CLAUDE_CODE_TMPDIR".to_owned(), String::new());
    assert_eq!(temp_root(&host), "/os/temp");
    host.env.insert("CLAUDE_CODE_TMPDIR".to_owned(), "/custom".to_owned());
    host.system = OperatingSystem::Darwin;
    assert_eq!(temp_root(&host), "/custom");
}

#[test]
fn the_extraction_root_is_plain_on_windows_and_falls_back_to_uid_zero_elsewhere() -> TestResult {
    let dir = tempfile::tempdir()?;
    let tmp = display(&dir.path().join("tmp"));
    let mut windows = Host::new(OperatingSystem::Win32, None);
    windows.env.insert("CLAUDE_CODE_TMPDIR".to_owned(), tmp.clone());
    assert_eq!(
        safe_temp_base(&windows, PathStyle::Posix),
        Ok(format!("{tmp}/claude-agent-sdk"))
    );
    assert!(dir.path().join("tmp/claude-agent-sdk").is_dir());
    let mut other = Host::new(OperatingSystem::Linux, None);
    other.env.insert("CLAUDE_CODE_TMPDIR".to_owned(), tmp.clone());
    assert_eq!(
        safe_temp_base(&other, PathStyle::Posix),
        Ok(format!("{tmp}/claude-agent-sdk-0"))
    );
    Ok(())
}

#[test]
fn a_failed_lstat_of_the_extraction_root_is_reported() -> TestResult {
    let mut sandbox = Sandbox::new()?;
    sandbox.host.fail_stat = Some(sandbox.base());
    let base = display(&sandbox.base());
    assert_eq!(
        safe_temp_base(&sandbox.host, PathStyle::Posix),
        Err(format!("EACCES: permission denied, lstat '{base}'"))
    );
    Ok(())
}

#[test]
fn a_failed_chmod_of_a_loose_extraction_root_is_reported() -> TestResult {
    let mut sandbox = Sandbox::new()?;
    fs::create_dir_all(sandbox.base())?;
    fs::set_permissions(sandbox.base(), Permissions::from_mode(0o755))?;
    sandbox.host.mode_error = Some(libc::EPERM);
    let base = display(&sandbox.base());
    assert_eq!(
        safe_temp_base(&sandbox.host, PathStyle::Posix),
        Err(format!("EPERM: operation not permitted, chmod '{base}'"))
    );
    Ok(())
}

#[test]
fn without_user_ids_the_copy_is_checked_by_comparing_file_ids_around_the_open() -> TestResult {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("copy");
    fs::write(&file, CONTENT)?;
    let host = Host::new(OperatingSystem::Win32, None);
    assert!(matches_content(&host, &file, CONTENT)?);
    host.script.borrow_mut().ids = VecDeque::from([7, 8]);
    assert!(!matches_content(&host, &file, CONTENT)?);
    host.script.borrow_mut().ids = VecDeque::from([7, 7]);
    assert!(matches_content(&host, &file, CONTENT)?);
    host.script.borrow_mut().ids = VecDeque::from([0, 8]);
    assert!(matches_content(&host, &file, CONTENT)?);
    host.script.borrow_mut().ids = VecDeque::from([7, -1]);
    assert!(matches_content(&host, &file, CONTENT)?);
    assert!(!matches_content(&host, &dir.path().join("."), CONTENT)?);
    assert!(matches_content(&host, &dir.path().join("missing"), CONTENT).is_err());
    assert!(!is_extracted_copy(&host, &dir.path().join("missing"), CONTENT));
    Ok(())
}

#[test]
fn a_file_id_that_names_no_object_is_ignored() {
    for id in [0, -1, i128::from(u64::MAX), 1_i128 << 64] {
        assert!(has_no_file_id(id), "{id}");
    }
    for id in [1, 42, i128::from(u64::MAX) - 1] {
        assert!(!has_no_file_id(id), "{id}");
    }
}

#[test]
fn messages_read_like_node_with_or_without_an_errno_name() {
    let known = io::Error::from_raw_os_error(libc::ENOENT);
    assert_eq!(error_code(&known), Some("ENOENT"));
    assert_eq!(
        fs_message(&known, "open", Some(Path::new("/a")), None),
        "ENOENT: no such file or directory, open '/a'"
    );
    let unknown = io::Error::other("boom");
    assert_eq!(error_code(&unknown), None);
    assert_eq!(fs_message(&unknown, "read", None, None), "boom, read");
    assert_eq!(
        fs_message(&unknown, "rename", Some(Path::new("/a")), Some(Path::new("/b"))),
        "boom, rename '/a' -> '/b'"
    );
}

#[test]
fn the_native_host_reaches_the_real_system() -> TestResult {
    let host = NativeExtractHost;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("file");
    fs::write(&file, "bytes")?;
    assert_eq!(host.read_file(&file)?, b"bytes");
    let nested = dir.path().join("nested");
    host.create_directory(&DirBuilder::new(), &nested)?;
    assert!(host.stat_link(&nested)?.is_dir());
    let mut options = OpenOptions::new();
    options.read(true);
    let mut opened = host.open(&options, &file)?;
    let mut read = String::new();
    opened.read_to_string(&mut read)?;
    assert_eq!(read, "bytes");
    host.change_file_mode(&opened, 0o640)?;
    assert_eq!(fs::metadata(&file)?.mode() & 0o777, 0o640);
    host.change_mode(&file, 0o600)?;
    assert_eq!(fs::metadata(&file)?.mode() & 0o777, 0o600);
    assert_eq!(
        host.file_id(&fs::metadata(&file)?),
        i128::from(fs::metadata(&file)?.ino())
    );
    let moved = dir.path().join("moved");
    host.rename_path(&file, &moved)?;
    host.remove_file(&moved)?;
    assert!(host.stat_link(&moved).is_err());
    assert_eq!(host.process_id(), std::process::id());
    assert_ne!(host.random_bytes(), host.random_bytes());
    assert_eq!(host.var_os(OsStr::new("TROGON_EXTRACT_UNSET_VARIABLE")), None);
    assert_eq!(host.temp_dir(), std::env::temp_dir());
    assert_eq!(host.operating_system(), NativeHost.operating_system());
    assert_eq!(host.username(), NativeHost.username());
    assert_eq!(host.real_user_id(), NativeHost.real_user_id());
    host.emit_warning("native host warning");
    Ok(())
}

#[test]
fn the_default_entry_point_passes_plain_paths_through_and_returns_the_input_on_failure() {
    let plain = Path::new("/usr/local/bin/claude");
    assert_eq!(extract_from_bunfs(plain), plain);
    let missing = Path::new("/$bunfs/root/definitely-missing");
    assert_eq!(extract_from_bunfs(missing), missing);
}
