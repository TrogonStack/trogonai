#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Value, json};

use super::{
    ConnectorFolders, folder_ignored, is_connector_name, is_download_temp_name, names_connector_folder, normalized_name,
};
use crate::connector_vetting::tests::{compare, inputs, probes, setup};
use crate::connector_vetting::{Vetter, VettingState};
use crate::node_path::{js, rust};

/// Replays every probe of `kind`, answering each of its paths with `answer`.
fn replay(kind: &str, answer: impl Fn(&Vetter<'_>, &ConnectorFolders, &[u16]) -> Value) {
    let mut failures = Vec::new();
    for probe in probes(kind) {
        let setup = setup(&probe);
        let state = VettingState::default();
        let folders = ConnectorFolders::default();
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
                .map(|path| answer(&vetter, &folders, path))
                .collect(),
        );
        compare(&probe, &actual, &setup.host, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn replays_every_folder_probe_recorded_from_the_bundle() {
    replay(
        "folder",
        |vetter, _, path| json!({ "ignored": folder_ignored(vetter, path) }),
    );
}

#[test]
fn replays_every_path_probe_recorded_from_the_bundle() {
    replay("path", |vetter, folders, path| match folders.is_ignored(vetter, path) {
        Ok(ignored) => json!({ "ignored": ignored }),
        Err(error) => json!({ "thrown": { "code": error.code, "reason": null } }),
    });
}

#[test]
fn replays_every_name_probe_recorded_from_the_bundle() {
    replay("name", |_, _, name| {
        json!({
            "connector": is_connector_name(name),
            "normalized": rust(&normalized_name(name)),
            "names": names_connector_folder(name),
        })
    });
}

#[test]
fn matches_the_download_names_the_bundle_treats_as_in_progress() {
    let matches = |name: &str| is_download_temp_name(&js(name));
    for name in [
        "download-0123abcd",
        "download-0123abcd-2",
        "download-0123abcd.pdf",
        "download-0123abcd-15.",
        ".download-0123abcd.part",
        ".download-0123abcd.from.json",
        ".download-0123abcd-3.pdf.part",
        ".download-0123abcd..part",
    ] {
        assert!(matches(name), "{name}");
    }
    for name in [
        "download-0123ABCD",
        "download-0123abc",
        "download-0123abcd-",
        "download-0123abcd.a.b",
        "download-0123abcdx",
        ".download-0123abcd",
        ".download-0123ABCD.part",
        ".download-0123abc.part",
        ".download-0123abcd.a.b.part",
        ".download-0123abcd.part.x",
        "xdownload-0123abcd",
    ] {
        assert!(!matches(name), "{name}");
    }
}
