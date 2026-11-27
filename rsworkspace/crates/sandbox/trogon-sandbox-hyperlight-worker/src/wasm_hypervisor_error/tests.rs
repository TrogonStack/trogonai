use std::error::Error;

use super::*;

fn refused() -> HyperlightError {
    HyperlightError::Error("refused".to_owned())
}

#[test]
fn a_backend_failure_keeps_its_message_as_a_hypervisor_failure() {
    let error = HypervisorError::from(WasmHypervisorError::LoadModule {
        path: PathBuf::from("components/echo.aot"),
        source: refused(),
    });
    assert!(matches!(error, HypervisorError::Failed(_)));
    let message = error.to_string();
    assert!(
        message.contains("loading the module components/echo.aot failed"),
        "{message}"
    );
    assert!(message.contains("refused"), "{message}");
}

#[test]
fn every_step_names_what_failed() {
    let cases = [
        (WasmHypervisorError::Build(refused()), "building the sandbox"),
        (WasmHypervisorError::LoadRuntime(refused()), "loading the wasm runtime"),
        (WasmHypervisorError::Snapshot(refused()), "snapshotting the guest"),
        (WasmHypervisorError::LayoutTag(refused()), "layout tag was refused"),
        (
            WasmHypervisorError::ReadLayout {
                path: PathBuf::from("in"),
                source: refused(),
            },
            "reading the snapshot layout in",
        ),
        (
            WasmHypervisorError::WriteLayout {
                path: PathBuf::from("out"),
                source: refused(),
            },
            "writing the snapshot layout out",
        ),
        (
            WasmHypervisorError::LoadSnapshot(refused()),
            "starting the guest from a snapshot",
        ),
        (WasmHypervisorError::Reset(refused()), "resetting the guest"),
        (WasmHypervisorError::Unload(refused()), "unloading the module"),
    ];
    for (error, expected) in cases {
        assert!(error.source().is_some(), "{error} lost its source");
        let message = error.to_string();
        assert!(message.contains(expected), "{message} lacks {expected}");
    }
}
