//! The check a paid lane uses to prove a snapshot it created is gone must itself fail closed. These run against a
//! loopback stand-in and need no credentials.

mod hosted_support;
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hosted_support::snapshot_retirement::NativeSnapshots;
use serde_json::{Value, json};
use support::{Reply, Server};
use url::Url;

const ID: &str = "snap_aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FOREIGN_ID: &str = "snap_bbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[derive(Clone)]
struct Response {
    status: u16,
    body: Vec<u8>,
}

impl Response {
    fn snapshot(id: &str, status: &str) -> Self {
        Self::json(
            200,
            json!({"snapshot": {
                "id": id, "status": status, "sourceSessionId": "retirement-source-session",
                "createdAt": 1, "updatedAt": 2, "lastUsedAt": 1, "sizeBytes": 1024, "region": "iad1",
            }}),
        )
    }

    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            body: body.to_string().into_bytes(),
        }
    }

    fn not_json() -> Self {
        Self {
            status: 200,
            body: b"not JSON".to_vec(),
        }
    }

    fn absent() -> Self {
        Self::json(404, json!({"error": "absent"}))
    }

    fn unauthorized() -> Self {
        Self::json(401, json!({"error": "unauthorized"}))
    }

    fn reply(&self) -> Reply {
        Reply::Complete(self.status, self.body.clone())
    }
}

/// What the stand-in answers to the read before the delete, the delete, and each read after it.
struct Scenario {
    before: Response,
    deleted: Response,
    after: Vec<Response>,
}

impl Scenario {
    fn after(response: Response) -> Self {
        Self {
            before: Response::snapshot(ID, "created"),
            deleted: Response::snapshot(ID, "deleted"),
            after: vec![response],
        }
    }

    /// The first read after the delete is `response`, and a clean absence follows it, so only a verifier that trusts
    /// the later absence over the earlier invalid answer would accept it.
    fn invalid_after(response: Response) -> Self {
        Self {
            after: vec![response, Response::absent()],
            ..Self::after(Response::snapshot(ID, "deleted"))
        }
    }
}

#[derive(Default)]
struct Calls {
    before: AtomicUsize,
    deletes: AtomicUsize,
    after: AtomicUsize,
    wrong_scope: AtomicUsize,
}

fn count(counter: &AtomicUsize) -> usize {
    counter.load(Ordering::SeqCst)
}

async fn verify(scenario: Scenario) -> Result<(Result<(), hosted_support::Boxed>, Arc<Calls>), hosted_support::Boxed> {
    let calls = Arc::new(Calls::default());
    let observed = Arc::clone(&calls);
    let server = Server::start(move |request| {
        let correct = Url::parse(&format!("http://fixture{}", request.path)).is_ok_and(|url| {
            url.path() == format!("/v2/sandboxes/snapshots/{ID}")
                && url
                    .query_pairs()
                    .any(|(key, value)| key == "teamId" && value == "fixture-team")
                && url
                    .query_pairs()
                    .any(|(key, value)| key == "projectId" && value == "fixture-project")
        });
        if !correct {
            observed.wrong_scope.fetch_add(1, Ordering::SeqCst);
            return Response::json(404, json!({"error": "unverified route or scope"})).reply();
        }
        match request.method.as_str() {
            "GET" if count(&observed.deletes) == 0 => {
                observed.before.fetch_add(1, Ordering::SeqCst);
                scenario.before.reply()
            }
            "DELETE" => {
                observed.deletes.fetch_add(1, Ordering::SeqCst);
                scenario.deleted.reply()
            }
            "GET" => {
                let index = observed.after.fetch_add(1, Ordering::SeqCst);
                scenario.after[index.min(scenario.after.len() - 1)].reply()
            }
            _ => Response::json(405, json!({"error": "unexpected method"})).reply(),
        }
    })
    .await?;
    let native = NativeSnapshots::new(
        Url::parse(&server.url)?,
        "fixture-team",
        "fixture-project",
        "fixture-token",
        Duration::from_secs(2),
    )?;
    let result = match tokio::time::timeout(Duration::from_secs(25), native.retire(ID)).await {
        Ok(result) => result,
        Err(_) => Err("the retirement verifier exceeded its deadline".into()),
    };
    server.shutdown().await;
    assert_eq!(count(&calls.wrong_scope), 0);
    Ok((result, calls))
}

#[tokio::test]
async fn exact_deleted_snapshot_metadata_confirms_retirement() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario::after(Response::snapshot(ID, "deleted"))).await?;
    result?;
    assert_eq!(
        (count(&calls.before), count(&calls.deletes), count(&calls.after)),
        (1, 1, 1)
    );
    Ok(())
}

#[tokio::test]
async fn exact_positive_binding_and_delete_can_confirm_a_subsequent_not_found() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario::after(Response::absent())).await?;
    result?;
    assert_eq!(
        (count(&calls.before), count(&calls.deletes), count(&calls.after)),
        (1, 1, 1)
    );
    Ok(())
}

#[tokio::test]
async fn an_unbound_initial_not_found_cannot_confirm_retirement() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario {
        before: Response::absent(),
        ..Scenario::after(Response::snapshot(ID, "deleted"))
    })
    .await?;
    assert!(result.is_err());
    assert_eq!((count(&calls.deletes), count(&calls.after)), (0, 0));
    Ok(())
}

#[tokio::test]
async fn a_foreign_initial_identity_is_not_deleted() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario {
        before: Response::snapshot(FOREIGN_ID, "created"),
        ..Scenario::after(Response::snapshot(ID, "deleted"))
    })
    .await?;
    assert!(result.is_err());
    assert_eq!(count(&calls.deletes), 0);
    Ok(())
}

#[tokio::test]
async fn malformed_initial_metadata_is_not_deleted() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario {
        before: Response::not_json(),
        ..Scenario::after(Response::snapshot(ID, "deleted"))
    })
    .await?;
    assert!(result.is_err());
    assert_eq!(count(&calls.deletes), 0);
    Ok(())
}

#[tokio::test]
async fn a_foreign_delete_receipt_is_not_replaced_by_a_later_not_found() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario {
        deleted: Response::snapshot(FOREIGN_ID, "deleted"),
        ..Scenario::after(Response::absent())
    })
    .await?;
    assert!(result.is_err());
    assert_eq!((count(&calls.deletes), count(&calls.after)), (1, 0));
    Ok(())
}

#[tokio::test]
async fn a_malformed_delete_receipt_is_not_replaced_by_a_later_not_found() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario {
        deleted: Response::not_json(),
        ..Scenario::after(Response::absent())
    })
    .await?;
    assert!(result.is_err());
    assert_eq!(count(&calls.after), 0);
    Ok(())
}

#[tokio::test]
async fn foreign_post_delete_metadata_is_not_replaced_by_a_later_not_found() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario::invalid_after(Response::snapshot(FOREIGN_ID, "deleted"))).await?;
    assert!(result.is_err());
    assert_eq!(count(&calls.after), 1);
    Ok(())
}

#[tokio::test]
async fn malformed_post_delete_metadata_is_not_replaced_by_a_later_not_found() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario::invalid_after(Response::not_json())).await?;
    assert!(result.is_err());
    assert_eq!(count(&calls.after), 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_snapshot_status_is_not_a_retirement_receipt() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario::invalid_after(Response::snapshot(ID, "failed"))).await?;
    assert!(result.is_err());
    assert_eq!(count(&calls.after), 1);
    Ok(())
}

#[tokio::test]
async fn an_undocumented_snapshot_status_is_not_a_retirement_receipt() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario::invalid_after(Response::snapshot(ID, "removed"))).await?;
    assert!(result.is_err());
    assert_eq!(count(&calls.after), 1);
    Ok(())
}

#[tokio::test]
async fn a_still_created_snapshot_needs_another_authoritative_observation() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario {
        after: vec![
            Response::snapshot(ID, "created"),
            Response::json(503, json!({"error": "unknown"})),
        ],
        ..Scenario::after(Response::snapshot(ID, "deleted"))
    })
    .await?;
    assert!(result.is_err());
    assert_eq!(count(&calls.after), 2);
    Ok(())
}

#[tokio::test]
async fn an_unauthorized_initial_read_cannot_authorize_a_delete() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario {
        before: Response::unauthorized(),
        ..Scenario::after(Response::snapshot(ID, "deleted"))
    })
    .await?;
    assert!(result.is_err());
    assert_eq!(count(&calls.deletes), 0);
    Ok(())
}

#[tokio::test]
async fn an_unauthorized_delete_cannot_authorize_retirement() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario {
        deleted: Response::unauthorized(),
        ..Scenario::after(Response::absent())
    })
    .await?;
    assert!(result.is_err());
    assert_eq!(count(&calls.after), 0);
    Ok(())
}

#[tokio::test]
async fn an_unauthorized_post_delete_read_cannot_confirm_retirement() -> hosted_support::TestResult {
    let (result, calls) = verify(Scenario::invalid_after(Response::unauthorized())).await?;
    assert!(result.is_err());
    assert_eq!(count(&calls.after), 1);
    Ok(())
}
