#![cfg(unix)]

mod support;

use std::time::Duration;

use support::{Fixture, TestResult};

use trogon_sandbox_contract::{FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, LeaseId};
use trogon_sandbox_e2b_worker::{E2bExec, Envd};

fn request(op: FileOp) -> FileOpRequest {
    FileOpRequest {
        lease_id: LeaseId::new("lease-paths"),
        as_user: None,
        op,
    }
}

#[tokio::test]
async fn path_mutations_honor_parents_overwrite_and_nonrecursive_intent() -> TestResult {
    let fixture = Fixture::new().await?;
    let tree = fixture.path("tree");
    let directory = tree.join("child");
    let name = |path: &std::path::Path| path.to_string_lossy().into_owned();
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let mkdir = |parents: bool| {
        request(FileOp::Mkdir {
            path: name(&directory),
            parents: parents.into(),
        })
    };
    assert!(matches!(
        exec.handle_file_op(&fixture.sandbox, &mkdir(false)).await,
        FileOpResponse::NotFound { .. }
    ));
    assert!(!tree.exists());
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &mkdir(true)).await,
        FileOpResponse::Created {
            already_existed: false.into()
        }
    );
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &mkdir(false)).await,
        FileOpResponse::Created {
            already_existed: true.into()
        }
    );

    let source = directory.join("source");
    let target = directory.join("target");
    std::fs::write(&source, b"source")?;
    std::fs::write(&target, b"keep")?;
    let rename = |overwrite: bool| {
        request(FileOp::Rename {
            from: name(&source),
            to: name(&target),
            overwrite: overwrite.into(),
        })
    };
    assert!(matches!(
        exec.handle_file_op(&fixture.sandbox, &rename(false)).await,
        FileOpResponse::Failed {
            kind: FileOpErrorKind::AlreadyExists,
            ..
        }
    ));
    assert_eq!(std::fs::read(&source)?, b"source");
    assert_eq!(std::fs::read(&target)?, b"keep");
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &rename(true)).await,
        FileOpResponse::Renamed
    );
    assert!(!source.exists());
    assert_eq!(std::fs::read(&target)?, b"source");
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &rename(false)).await,
        FileOpResponse::NotFound { path: name(&source) }
    );

    let remove = |recursive: bool| {
        request(FileOp::Remove {
            path: name(&tree),
            recursive: recursive.into(),
        })
    };
    assert!(matches!(
        exec.handle_file_op(&fixture.sandbox, &remove(false)).await,
        FileOpResponse::Failed {
            kind: FileOpErrorKind::DirectoryNotEmpty,
            ..
        }
    ));
    assert!(target.exists());
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &remove(true)).await,
        FileOpResponse::Removed { existed: true.into() }
    );
    assert!(!tree.exists());
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &remove(false)).await,
        FileOpResponse::Removed { existed: false.into() }
    );
    Ok(())
}

#[tokio::test]
async fn remove_unlinks_a_final_directory_symlink_without_removing_its_target() -> TestResult {
    let fixture = Fixture::new().await?;
    let target = fixture.path("target");
    let link = fixture.path("link");
    std::fs::create_dir(&target)?;
    std::fs::write(target.join("keep"), b"keep")?;
    std::os::unix::fs::symlink(&target, &link)?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(FileOp::Remove {
                path: link.to_string_lossy().into_owned(),
                recursive: true.into(),
            }),
        )
        .await;
    assert_eq!(reply, FileOpResponse::Removed { existed: true.into() });
    assert!(std::fs::symlink_metadata(link).is_err());
    assert_eq!(std::fs::read(target.join("keep"))?, b"keep");
    Ok(())
}

#[tokio::test]
async fn destructive_root_and_nul_paths_are_refused_before_guest_calls() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.disable_shell();
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    for op in [
        FileOp::Remove {
            path: "/".into(),
            recursive: true.into(),
        },
        FileOp::Remove {
            path: "/work/..".into(),
            recursive: true.into(),
        },
        FileOp::Mkdir {
            path: "bad\0path".into(),
            parents: true.into(),
        },
        FileOp::Rename {
            from: "/".into(),
            to: fixture.path("root").to_string_lossy().into_owned(),
            overwrite: true.into(),
        },
        FileOp::Rename {
            from: fixture.path("source").to_string_lossy().into_owned(),
            to: "/./".into(),
            overwrite: true.into(),
        },
    ] {
        let reply = exec.handle_file_op(&fixture.sandbox, &request(op)).await;
        assert!(
            matches!(
                reply,
                FileOpResponse::Failed {
                    kind: FileOpErrorKind::InvalidPath,
                    ..
                }
            ),
            "{reply:?}"
        );
    }
    assert_eq!(fixture.calls(), 0);
    assert_eq!(fixture.shell_calls(), 0);
    Ok(())
}
