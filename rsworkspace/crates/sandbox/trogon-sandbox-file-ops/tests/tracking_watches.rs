use std::error::Error;

use trogon_sandbox_contract::{
    FileContentHashing, FileOp, FileOpErrorKind, FileOpResponse, FilePresence, FileRecursion,
};
use trogon_sandbox_file_ops::{FileOpError, Manifest, ManifestSource, Tracking};

type TestResult = Result<(), Box<dyn Error>>;

struct Empty;

impl ManifestSource for Empty {
    async fn manifest(&self, _: &str, _: FileRecursion, _: FileContentHashing) -> Result<Manifest, FileOpError> {
        Ok(Manifest::new())
    }
}

#[tokio::test]
async fn a_watch_belongs_to_its_lease_and_stops_once() -> TestResult {
    let tracking = Tracking::new();
    let started = tracking
        .serve(
            "first",
            &FileOp::WatchStart {
                path: "/w".into(),
                recursive: FileRecursion::Flat,
            },
            &Empty,
        )
        .await;
    let Some(FileOpResponse::WatchStarted { watch_id }) = started else {
        return Err(format!("expected a started watch, got {started:?}").into());
    };

    let foreign = tracking
        .serve(
            "second",
            &FileOp::WatchPoll {
                watch_id: watch_id.clone(),
                after_seq: 0,
                wait_ms: None,
            },
            &Empty,
        )
        .await;
    assert!(
        matches!(
            &foreign,
            Some(FileOpResponse::Failed {
                kind: FileOpErrorKind::UnknownWatch,
                ..
            })
        ),
        "{foreign:?}"
    );

    let foreign_stop = tracking
        .serve(
            "second",
            &FileOp::WatchStop {
                watch_id: watch_id.clone(),
            },
            &Empty,
        )
        .await;
    assert_eq!(
        foreign_stop,
        Some(FileOpResponse::WatchStopped {
            existed: FilePresence::from(false)
        })
    );
    assert_eq!(tracking.watch_ids("first"), std::slice::from_ref(&watch_id));

    let stop = FileOp::WatchStop { watch_id };
    assert_eq!(
        tracking.serve("first", &stop, &Empty).await,
        Some(FileOpResponse::WatchStopped {
            existed: FilePresence::from(true)
        })
    );
    assert_eq!(
        tracking.serve("first", &stop, &Empty).await,
        Some(FileOpResponse::WatchStopped {
            existed: FilePresence::from(false)
        })
    );
    Ok(())
}
