use super::*;

fn reference() -> ModuleReference {
    "scheduler.schedules@0.1.0".parse().expect("a well-formed reference")
}

#[test]
fn a_file_source_resolves_the_reference_and_not_a_caller_supplied_path() {
    let source = FileModuleSource::new("/srv/modules");

    assert_eq!(
        source.path_for(&reference()),
        Path::new("/srv/modules/scheduler.schedules@0.1.0.wasm")
    );
}

#[tokio::test]
async fn a_missing_component_names_the_path_it_looked_for() {
    let source = FileModuleSource::new("/nonexistent-module-root");

    let error = source
        .fetch(&reference())
        .await
        .expect_err("nothing is published there");

    let FileModuleSourceError::Read { path, .. } = error;
    assert_eq!(
        path,
        source.path_for(&reference()),
        "an operator has to be told which file was missing, not just that one was"
    );
}

#[tokio::test]
async fn a_published_component_comes_back_verbatim() {
    let root = tempfile::tempdir().expect("a temp dir");
    let reference = reference();
    std::fs::write(root.path().join(reference.file_name()), b"\0asm not really").expect("the fixture writes");

    let source = FileModuleSource::new(root.path());

    assert_eq!(
        source.fetch(&reference).await.expect("the fixture is readable"),
        b"\0asm not really"
    );
}

#[test]
fn a_source_describes_the_store_it_searched() {
    assert_eq!(
        FileModuleSource::new("/srv/modules").describe(),
        "directory /srv/modules"
    );
}
