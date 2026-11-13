#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::cell::RefCell;

use serde_json::{Map, Value, json};

use super::{
    Absent, LinkStat, Located, Platform, VetError, Vetted, Vetter, VettingHost, VettingState, Walked, collapse_at_dots,
    collapse_posix, firmlink_path, has_dotted_component, has_parent_component, in_dot_namespace, is_device_namespace,
    is_net_root, is_object_namespace, is_within, is_wsl_share, on_network_mount, read_failure, share_mount,
    share_server, strip_device_prefix,
};
use crate::node_path::{JsString, NodeError, NodePathDialect, NodeProcess, js, rust};

/// The fake host the probe harness builds: every read is traced the way the harness traces it, and answers follow
/// the harness's rules for its fake file system.
pub(crate) struct FakeHost {
    win32: bool,
    tree: Vec<(String, Value)>,
    opendir: Vec<(String, Value)>,
    readlink: Vec<(String, Value)>,
    readdir: Vec<(String, Value)>,
    realpath: Vec<(String, Value)>,
    files: Map<String, Value>,
    stat: Map<String, Value>,
    homedir: Option<String>,
    cwd: Option<String>,
    env: Vec<(String, String)>,
    pub(crate) trace: RefCell<Vec<Value>>,
}

fn errno(code: &str) -> Option<String> {
    Some(code.to_owned())
}

/// A truthy JS string.
fn truthy(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|text| !text.is_empty())
}

/// `key.replace(/[\\/][^\\/]*$/, "")`.
fn parent(key: &str) -> &str {
    key.rfind(['/', '\\']).and_then(|index| key.get(..index)).unwrap_or(key)
}

/// The harness's `fsKey`: on Windows every entry is keyed by its backslash spelling with an upper case drive letter.
fn fs_key(win32: bool, path: &str) -> String {
    if !win32 {
        return path.to_owned();
    }
    let key = path.replace('/', "\\");
    match key.as_bytes() {
        [drive, b':', ..] if drive.is_ascii_lowercase() => {
            format!("{}{}", drive.to_ascii_uppercase() as char, &key[1..])
        }
        _ => key,
    }
}

impl FakeHost {
    fn new(probe: &Value) -> Self {
        let win32 = probe.get("platform").and_then(Value::as_str) == Some("win32");
        let keyed = |field: &str| {
            let mut entries: Vec<(String, Value)> = Vec::new();
            if let Some(object) = probe.get(field).and_then(Value::as_object) {
                for (key, value) in object {
                    let key = fs_key(win32, key);
                    match entries.iter_mut().find(|(existing, _)| *existing == key) {
                        Some(entry) => entry.1 = value.clone(),
                        None => entries.push((key, value.clone())),
                    }
                }
            }
            entries
        };
        let object = |field: &str| probe.get(field).and_then(Value::as_object).cloned().unwrap_or_default();
        let text = |field: &str| probe.get(field).and_then(Value::as_str).map(str::to_owned);
        Self {
            win32,
            tree: keyed("fs"),
            opendir: keyed("opendir"),
            readlink: keyed("readlink"),
            readdir: keyed("readdir"),
            realpath: keyed("realpath"),
            files: object("files"),
            stat: object("stat"),
            homedir: text("homedir"),
            cwd: text("cwd"),
            env: object("env")
                .into_iter()
                .map(|(name, value)| (name, value.as_str().unwrap_or_default().to_owned()))
                .collect(),
            trace: RefCell::new(Vec::new()),
        }
    }

    fn key(&self, path: &str) -> String {
        fs_key(self.win32, path)
    }

    /// The harness's `missing`: ENOTDIR when a file sits above `path`, ENOENT otherwise.
    fn missing(&self, path: &str) -> Option<String> {
        let mut below = self.key(path);
        loop {
            let above = parent(&below).to_owned();
            if above == below {
                return errno("ENOENT");
            }
            if Self::find(&self.tree, &above).and_then(kind) == Some("file") {
                return errno("ENOTDIR");
            }
            below = above;
        }
    }

    fn find<'v>(entries: &'v [(String, Value)], key: &str) -> Option<&'v Value> {
        entries.iter().find(|(name, _)| name == key).map(|(_, value)| value)
    }

    fn entry(&self, path: &str) -> Option<&Value> {
        Self::find(&self.tree, &self.key(path))
    }

    fn record(&self, call: Value) {
        self.trace.borrow_mut().push(call);
    }
}

fn kind(found: &Value) -> Option<&str> {
    found.get("kind").and_then(Value::as_str)
}

impl NodeProcess for FakeHost {
    fn cwd(&self) -> Result<JsString, NodeError> {
        self.record(json!(["cwd"]));
        self.cwd
            .as_deref()
            .map(js)
            .ok_or_else(|| NodeError { code: errno("ENOENT") })
    }

    fn env(&self, key: &[u16]) -> Option<JsString> {
        let key = rust(key);
        self.record(json!(["env", key]));
        self.env
            .iter()
            .find(|(name, _)| {
                if self.win32 {
                    name.to_uppercase() == key.to_uppercase()
                } else {
                    *name == key
                }
            })
            .map(|(_, value)| js(value))
    }
}

impl VettingHost for FakeHost {
    fn lstat(&self, path: &[u16]) -> Result<LinkStat, Option<String>> {
        let path = rust(path);
        self.record(json!(["lstat", path]));
        let found = self.entry(&path).ok_or_else(|| self.missing(&path))?;
        if let Some(code) = truthy(found.get("errno")) {
            return Err(errno(code));
        }
        let content = found.get("content").and_then(Value::as_str).unwrap_or_default();
        Ok(LinkStat {
            symlink: kind(found) == Some("symlink"),
            file: kind(found) == Some("file"),
            size: found
                .get("size")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| u64::try_from(content.len()).unwrap()),
        })
    }

    fn open_dir_no_follow(&self, path: &[u16]) -> Result<(), Option<String>> {
        let path = rust(path);
        self.record(json!(["opendir", path]));
        let forced = Self::find(&self.opendir, &self.key(&path));
        if forced.and_then(Value::as_str) == Some("ok") {
            return Ok(());
        }
        if let Some(code) = truthy(forced) {
            return Err(errno(code));
        }
        let found = self.entry(&path).ok_or_else(|| self.missing(&path))?;
        if let Some(code) = truthy(found.get("errno")) {
            return Err(errno(code));
        }
        match kind(found) {
            Some("dir") => Ok(()),
            Some("symlink") => Err(errno("ELOOP")),
            _ => Err(errno("ENOTDIR")),
        }
    }

    fn read_link(&self, path: &[u16]) -> Result<JsString, Option<String>> {
        let path = rust(path);
        self.record(json!(["readlink", path]));
        if let Some(code) = truthy(Self::find(&self.readlink, &self.key(&path))) {
            return Err(errno(code));
        }
        let found = self.entry(&path).ok_or_else(|| self.missing(&path))?;
        if kind(found) != Some("symlink") {
            return Err(errno("EINVAL"));
        }
        found.get("target").and_then(Value::as_str).map(js).ok_or(None)
    }

    fn read_dir_names(&self, path: &[u16]) -> Result<Vec<JsString>, Option<String>> {
        let path = rust(path);
        self.record(json!(["readdir", path]));
        let folder = self.key(&path);
        let listed = Self::find(&self.readdir, &folder);
        if let Some(code) = listed.and_then(Value::as_str) {
            return Err(errno(code));
        }
        if let Some(names) = listed.and_then(Value::as_array) {
            return Ok(names.iter().map(|name| js(name.as_str().unwrap_or_default())).collect());
        }
        let found = self.entry(&path).ok_or_else(|| self.missing(&path))?;
        if let Some(code) = truthy(found.get("errno")) {
            return Err(errno(code));
        }
        if kind(found) != Some("dir") {
            return Err(errno("ENOTDIR"));
        }
        let folder_units = js(&folder).len();
        Ok(self
            .tree
            .iter()
            .filter(|(candidate, _)| *candidate != folder && parent(candidate) == folder)
            .map(|(candidate, _)| js(candidate).split_off(folder_units + 1))
            .collect())
    }

    fn read_head(&self, path: &[u16], limit: usize) -> Result<Vec<u8>, Option<String>> {
        let path = rust(path);
        self.record(json!(["open", path]));
        let found = self.entry(&path).ok_or_else(|| self.missing(&path))?;
        if let Some(code) = truthy(found.get("openErrno")) {
            return Err(errno(code));
        }
        let content = found
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .as_bytes();
        Ok(content.iter().copied().take(limit).collect())
    }

    fn read_auto_master(&self) -> Result<String, Option<String>> {
        self.record(json!(["readfile", "/etc/auto_master"]));
        match self.files.get("/etc/auto_master") {
            None => Err(errno("ENOENT")),
            Some(Value::String(text)) => Ok(text.clone()),
            Some(other) => Err(truthy(other.get("errno")).map(str::to_owned)),
        }
    }

    fn stat_device(&self, path: &str) -> Result<u64, Option<String>> {
        self.record(json!(["stat", path]));
        match self.stat.get(path) {
            None => Err(errno("ENOENT")),
            Some(Value::String(code)) => Err(errno(code)),
            Some(found) => Ok(found.get("dev").and_then(Value::as_u64).unwrap_or_default()),
        }
    }

    fn home_directory(&self) -> Result<JsString, Option<String>> {
        self.record(json!(["homedir"]));
        self.homedir.as_deref().map(js).ok_or_else(|| errno("ENOENT"))
    }

    fn real_path(&self, path: &[u16]) -> Result<JsString, Option<String>> {
        let path = rust(path);
        self.record(json!(["realpath", path]));
        if let Some(forced) = Self::find(&self.realpath, &self.key(&path)) {
            if let Some(code) = truthy(forced.get("errno")) {
                return Err(errno(code));
            }
            return forced.get("path").and_then(Value::as_str).map(js).ok_or(None);
        }
        let found = self.entry(&path).ok_or_else(|| self.missing(&path))?;
        if let Some(code) = truthy(found.get("errno")) {
            return Err(errno(code));
        }
        Ok(js(&path))
    }
}

/// Everything one probe runs against.
pub(crate) struct Setup {
    pub(crate) host: FakeHost,
    pub(crate) platform: Platform,
    pub(crate) dialect: NodePathDialect,
    pub(crate) launch: Option<JsString>,
}

pub(crate) fn setup(probe: &Value) -> Setup {
    let platform = match probe.get("platform").and_then(Value::as_str) {
        Some("darwin") => Platform::Darwin,
        Some("win32") => Platform::Win32,
        _ => Platform::Other,
    };
    let dialect = match probe.get("node").and_then(Value::as_str).unwrap_or("24.13.1") {
        "24.13.1" => Some(NodePathDialect::Current),
        "20.20.2" => Some(NodePathDialect::Node20),
        "22.14.0" => Some(NodePathDialect::BeforeReservedNames),
        _ => None,
    }
    .expect("a recorded node release");
    Setup {
        host: FakeHost::new(probe),
        platform,
        dialect,
        launch: probe.get("launch").and_then(Value::as_str).map(js),
    }
}

/// The recorded probes of one kind.
pub(crate) fn probes(kind: &str) -> Vec<Value> {
    let document: Value =
        serde_json::from_str(include_str!("../tests/fixtures/connector_vetting/probes.json")).expect("probes parse");
    let probes: Vec<Value> = document
        .get("probes")
        .and_then(Value::as_array)
        .expect("probes array")
        .iter()
        .filter(|probe| probe.get("kind").and_then(Value::as_str) == Some(kind))
        .cloned()
        .collect();
    assert!(!probes.is_empty(), "no {kind} probes recorded");
    probes
}

pub(crate) fn name(probe: &Value) -> &str {
    probe.get("name").and_then(Value::as_str).unwrap_or("?")
}

/// The `paths` a vet, folder, path or name probe asks about.
pub(crate) fn inputs(probe: &Value) -> Vec<JsString> {
    probe
        .get("paths")
        .and_then(Value::as_array)
        .expect("paths")
        .iter()
        .map(|path| js(path.as_str().expect("path text")))
        .collect()
}

/// Compares an answer and the host reads with what the harness recorded, adding any mismatch to `failures`.
pub(crate) fn compare(probe: &Value, actual: &Value, host: &FakeHost, failures: &mut Vec<String>) {
    let name = name(probe);
    let expected = probe.get("expect").cloned().unwrap_or(Value::Null);
    if *actual != expected {
        failures.push(format!("{name}: answer {actual} != expected {expected}"));
    }
    let trace = Value::Array(host.trace.borrow().clone());
    let wanted = probe.get("calls").cloned().unwrap_or_else(|| json!([]));
    if trace != wanted {
        failures.push(format!("{name}: reads {trace} != expected {wanted}"));
    }
}

fn absent_json(absent: &Absent) -> Value {
    let mut out = json!({
        "at": rust(&absent.at),
        "remaining": absent.remaining.iter().map(|part| rust(part)).collect::<Vec<_>>(),
        "fromLinkTarget": absent.from_link_target,
    });
    if absent.not_a_folder {
        out["notAFolder"] = json!(true);
    }
    out
}

fn vetted_json(answer: Result<Vetted, VetError>) -> Value {
    match answer {
        Ok(vetted) => json!({
            "result": "ok",
            "absentAt": vetted.absent_at.as_ref().map(absent_json),
            "linksResolvedPath": vetted.links_resolved_path.as_deref().map(rust),
        }),
        Err(error) => json!({
            "result": "refused",
            "reason": error.reason().map(|reason| reason.as_str()),
            "code": error.code(),
        }),
    }
}

fn value(answer: impl Into<Value>) -> Value {
    json!({ "value": answer.into() })
}

fn text(answer: &[u16]) -> Value {
    value(rust(answer))
}

fn maybe_text(answer: Option<JsString>) -> Value {
    value(answer.as_deref().map(rust))
}

fn thrown(error: &NodeError) -> Value {
    json!({ "thrown": { "code": error.code, "reason": null } })
}

/// Dispatches one `call` probe to the port of the bundle function it names.
fn call(vetter: &Vetter<'_>, probe: &Value) -> Value {
    let args = probe.get("args").and_then(Value::as_array).cloned().unwrap_or_default();
    let arg = |index: usize| js(args.get(index).and_then(Value::as_str).unwrap_or_default());
    let list = |index: usize| {
        args.get(index).and_then(Value::as_array).map(|items| {
            items
                .iter()
                .map(|item| js(item.as_str().unwrap_or_default()))
                .collect::<Vec<JsString>>()
        })
    };
    let (a, b) = (arg(0), arg(1));
    let anchors = list(1);
    let anchors = anchors.as_deref();
    match probe.get("fn").and_then(Value::as_str).unwrap_or_default() {
        "Pn" => value(vetter.is_unc(&a)),
        "Wm" => value(vetter.is_object_spelling(&a)),
        "nee" => text(&vetter.win32_normalize(&a)),
        "Or" => value(is_wsl_share(&a)),
        "Yn" => value(has_dotted_component(&a)),
        "vl" => value(has_parent_component(&a)),
        "go" => value(on_network_mount(&a)),
        "dd" => maybe_text(share_mount(&a)),
        "ud" => maybe_text(vetter.share_name(&a)),
        "Rr" => value(vetter.is_share_automount(&a)),
        "Kr" => value(share_mount(&a).is_some()),
        "Xo" => value(in_dot_namespace(&a)),
        "Qo" => value(is_net_root(&a)),
        "es" => text(&strip_device_prefix(&a)),
        "OI" => maybe_text(share_server(&a)),
        "Jo" => value(is_device_namespace(&a)),
        "Gb" => value(is_object_namespace(&a)),
        "Kee" => text(&vetter.collapse(&a)),
        "IC" => text(&collapse_at_dots(&a)),
        "Bee" => text(&vetter.collapse_win32(&a)),
        "jee" => text(&collapse_posix(&a)),
        "XI" => text(&firmlink_path(&a)),
        "Wee" => value(vetter.is_opaque_link_text(&a)),
        "LC" => vetter
            .is_launch_ancestor(&a, &b)
            .map_or_else(|error| thrown(&error), value),
        "MC" => value(is_within(&a, &b)),
        "$I" => value(vetter.leaves_share(&a, &b)),
        "ig" => text(&vetter.landing(&a, anchors.unwrap_or_default())),
        "sM" => vetter
            .share_landing(&a, anchors)
            .map_or_else(|| json!({ "undefined": true }), |landed| text(&landed)),
        "BI" => value(vetter.home_is_automounted()),
        "MQ" => value(vetter.leaves_automount(&a, &b)),
        "Wb" => value(vetter.leaves_anchor(&a, &b)),
        "FQ" => value(vetter.reaches_automount(&a, vetter.darwin() && vetter.home_is_automounted())),
        "HQ" => maybe_text(vetter.home_folder()),
        "sg" => value(vetter.stays_on_anchor(&a, anchors)),
        "rM" => value(vetter.holds_anchor(&a, anchors)),
        "ag" => value(vetter.leaves_every_anchor(&a, anchors)),
        "oM" => value(vetter.leaves_every_share(&a, anchors)),
        "cg" => maybe_text(vetter.launch_ancestry()),
        "nM" => {
            let code = args
                .first()
                .and_then(|first| first.get("errno"))
                .and_then(Value::as_str);
            let unverified = args
                .get(1)
                .and_then(|options| options.get("unreadableAncestry"))
                .and_then(Value::as_str)
                == Some("unverified");
            let flag = |index: usize| args.get(index).and_then(Value::as_bool).unwrap_or_default();
            match read_failure(code, unverified, flag(2), flag(3), flag(4)) {
                Walked::Done => json!({ "undefined": true }),
                _ => value("\u{0}unverified-ancestry"),
            }
        }
        other => json!({ "unported": other }),
    }
}

#[test]
fn replays_every_helper_probe_recorded_from_the_bundle() {
    let mut failures = Vec::new();
    for probe in probes("call") {
        let setup = setup(&probe);
        let state = VettingState::default();
        let vetter = Vetter::new(
            &setup.host,
            setup.platform,
            setup.dialect,
            setup.launch.as_deref(),
            &state,
        );
        let actual = call(&vetter, &probe);
        compare(&probe, &actual, &setup.host, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn replays_every_vet_probe_recorded_from_the_bundle() {
    let mut failures = Vec::new();
    for probe in probes("vet") {
        let setup = setup(&probe);
        let state = VettingState::default();
        let vetter = Vetter::new(
            &setup.host,
            setup.platform,
            setup.dialect,
            setup.launch.as_deref(),
            &state,
        );
        let actual = Value::Array(
            inputs(&probe)
                .iter()
                .map(|path| vetted_json(vetter.vet(path)))
                .collect(),
        );
        compare(&probe, &actual, &setup.host, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn located_json(located: Located) -> Value {
    match located {
        Located::Gone => json!("gone"),
        Located::Unverified => json!("unverified"),
        Located::Network(path) => json!({ "network": rust(&path) }),
        Located::Real(path) => json!({ "real": rust(&path) }),
    }
}

#[test]
fn replays_every_locate_probe_recorded_from_the_bundle() {
    let mut failures = Vec::new();
    for probe in probes("locate") {
        let setup = setup(&probe);
        let state = VettingState::default();
        let vetter = Vetter::new(
            &setup.host,
            setup.platform,
            setup.dialect,
            setup.launch.as_deref(),
            &state,
        );
        let actual = Value::Array(
            inputs(&probe)
                .iter()
                .map(|path| located_json(vetter.locate(path)))
                .collect(),
        );
        compare(&probe, &actual, &setup.host, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn remembers_the_home_automount_answer_once_it_was_read() {
    let probe = json!({
        "platform": "darwin",
        "files": { "/etc/auto_master": "/home auto_home\n" },
    });
    let setup = setup(&probe);
    let state = VettingState::default();
    let vetter = Vetter::new(&setup.host, setup.platform, setup.dialect, None, &state);
    assert!(vetter.home_is_automounted());
    assert!(vetter.home_is_automounted());
    assert_eq!(
        *setup.host.trace.borrow(),
        vec![json!(["readfile", "/etc/auto_master"])]
    );
}

#[test]
fn asks_again_when_the_home_automount_could_not_be_read() {
    let probe = json!({
        "platform": "darwin",
        "files": { "/etc/auto_master": { "errno": "EACCES" } },
    });
    let setup = setup(&probe);
    let state = VettingState::default();
    let vetter = Vetter::new(&setup.host, setup.platform, setup.dialect, None, &state);
    assert!(vetter.home_is_automounted());
    assert!(vetter.home_is_automounted());
    assert_eq!(setup.host.trace.borrow().len(), 2);
}
