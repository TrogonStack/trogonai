use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use super::memory_index::MemoryIndex;
use super::scripted_control_tests::{ScriptedControl, built, crashed, template};
use super::{DEADLINE, IMAGE, config, key, key_with, registry};
use crate::ate_client::AteError;
use crate::config::{SandboxClass, SubstrateConfig};
use crate::proto;
use crate::resources::ActorResources;
use crate::template::constants::FAILURE_TTL;
use crate::template::now_secs::now_secs;
use crate::template::{TemplateError, TemplateKey, TemplatePhase, TemplateRecord};

type TestResult = Result<(), Box<dyn Error>>;

fn backend(message: &str) -> Option<TemplateError> {
    Some(TemplateError::Backend {
        message: message.to_string(),
    })
}

/// A template that exists server-side with a permanently failed golden snapshot, the way
/// Substrate leaves it once a golden build crashes: `AlreadyExists` forever, no retry.
fn dead_then_rebuilt(deletes_to_recover: usize) -> ScriptedControl {
    let deleted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen_by_create = deleted.clone();
    let seen_by_get = deleted.clone();
    let seen_by_delete = deleted;
    ScriptedControl::scripted(
        move |_| {
            if seen_by_create.load(std::sync::atomic::Ordering::SeqCst) < deletes_to_recover {
                Err(AteError::AlreadyExists {
                    message: "template already exists".into(),
                })
            } else {
                Ok(())
            }
        },
        move |_, reference| {
            let golden = if seen_by_get.load(std::sync::atomic::Ordering::SeqCst) < deletes_to_recover {
                crashed("GoldenActorCrashed: golden actor crashed before its snapshot was taken")
            } else {
                built(reference)
            };
            Ok(template(reference, golden))
        },
        move |_| {
            seen_by_delete.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        },
    )
}

#[tokio::test]
async fn a_cluster_that_refuses_the_create_fails_the_lease_and_caches_the_failure() -> TestResult {
    let index = Arc::new(MemoryIndex::default());
    let control = ScriptedControl::scripted(
        |_| Err(AteError::Unavailable { message: "down".into() }),
        |_, _| {
            Err(AteError::Other {
                message: "unused".into(),
            })
        },
        |_| Ok(()),
    );

    let error = registry(index.clone())
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert_eq!(error, backend("control plane unavailable: down"));
    let stored = index.record(&key()?.kv_key("tenant-a")).await.ok_or("record")?;
    assert_eq!(stored.phase, TemplatePhase::Failed);
    assert!(stored.message.is_some_and(|message| message.contains("down")));
    Ok(())
}

#[tokio::test]
async fn a_permanently_failed_existing_template_is_deleted_and_rebuilt() -> TestResult {
    // `CreateActorTemplate` collapses to `AlreadyExists` forever on a dead template, so an
    // expired local failure record alone must not be trusted to mean a fresh build.
    let index = Arc::new(MemoryIndex::default());
    let control = dead_then_rebuilt(1);

    let reference = registry(index.clone())
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(reference.name, key()?.template_name());
    assert_eq!(control.deletes(), 1, "the dead template must be deleted exactly once");
    assert_eq!(
        control.creates(),
        2,
        "the first create collapses to AlreadyExists, the second is the real rebuild"
    );
    let stored = index.record(&key()?.kv_key("tenant-a")).await.ok_or("record")?;
    assert_eq!(stored.phase, TemplatePhase::Ready);
    Ok(())
}

#[tokio::test]
async fn a_deterministically_broken_template_is_rebuilt_once_then_bounded_by_the_ttl() -> TestResult {
    // The rebuild fails the same way as the original, so without a bound every lease would
    // trigger another delete-and-rebuild forever. FAILURE_TTL is the bound.
    let index = Arc::new(MemoryIndex::default());
    let control = dead_then_rebuilt(usize::MAX);
    let registry = registry(index.clone());
    let kv_key = key()?.kv_key("tenant-a");

    let first = registry
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();
    assert!(matches!(first, Some(TemplateError::NotReady { .. })), "{first:?}");
    assert_eq!(control.deletes(), 1);
    assert_eq!(control.creates(), 2);
    let gets_after_first = control.gets();

    registry
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();
    assert_eq!(
        control.deletes(),
        1,
        "a cached failure must not delete and rebuild again"
    );
    assert_eq!(control.creates(), 2);
    assert_eq!(
        control.gets(),
        gets_after_first,
        "a cached failure answers locally, without asking the control plane again"
    );

    // Once the local record expires, exactly one more attempt is made.
    index
        .seed(
            &kv_key,
            TemplateRecord {
                atespace: "tenant-a".to_string(),
                name: key()?.template_name(),
                phase: TemplatePhase::Failed,
                created_at: now_secs() - FAILURE_TTL.as_secs() - 1,
                message: Some("GoldenActorCrashed".to_string()),
            },
        )
        .await;
    registry
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();
    assert_eq!(control.deletes(), 2, "the ttl re-arms exactly one more attempt");
    assert_eq!(control.creates(), 4);
    Ok(())
}

#[tokio::test]
async fn an_existing_template_that_is_still_building_is_awaited_not_deleted() -> TestResult {
    let control = ScriptedControl::scripted(
        |_| {
            Err(AteError::AlreadyExists {
                message: "exists".into(),
            })
        },
        |seen, reference| {
            let golden = if seen < 2 {
                proto::GoldenSnapshotStatus::default()
            } else {
                built(reference)
            };
            Ok(template(reference, golden))
        },
        |_| Ok(()),
    );

    registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(control.deletes(), 0);
    assert_eq!(control.creates(), 1);
    Ok(())
}

#[tokio::test]
async fn an_existing_template_that_cannot_be_read_is_left_to_the_wait_loop() -> TestResult {
    let control = ScriptedControl::scripted(
        |_| {
            Err(AteError::AlreadyExists {
                message: "exists".into(),
            })
        },
        |seen, reference| {
            if seen == 0 {
                Err(AteError::Unavailable { message: "flap".into() })
            } else {
                Ok(template(reference, built(reference)))
            }
        },
        |_| Ok(()),
    );

    registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(control.deletes(), 0);
    Ok(())
}

#[tokio::test]
async fn a_dead_template_for_another_identity_is_never_deleted() -> TestResult {
    let control = ScriptedControl::scripted(
        |_| {
            Err(AteError::AlreadyExists {
                message: "exists".into(),
            })
        },
        |_, reference| {
            let mut other = reference.clone();
            other.name = "tpl-other".into();
            Ok(template(&other, crashed("crashed")))
        },
        |_| Ok(()),
    );

    let error = registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert_eq!(
        error,
        backend("failed template address does not match the requested identity")
    );
    assert_eq!(control.deletes(), 0);
    Ok(())
}

#[tokio::test]
async fn a_dead_template_without_a_deletable_revision_is_an_error() -> TestResult {
    let control = ScriptedControl::scripted(
        |_| {
            Err(AteError::AlreadyExists {
                message: "exists".into(),
            })
        },
        |_, reference| {
            let mut dead = template(reference, crashed("crashed"));
            if let Some(metadata) = dead.metadata.as_mut() {
                metadata.uid.clear();
            }
            Ok(dead)
        },
        |_| Ok(()),
    );

    let error = registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert!(matches!(error, Some(TemplateError::Backend { .. })), "{error:?}");
    assert_eq!(control.deletes(), 0);
    Ok(())
}

#[tokio::test]
async fn a_dead_template_already_deleted_by_someone_else_is_rebuilt() -> TestResult {
    let control = ScriptedControl::scripted(
        |seen| {
            if seen == 0 {
                Err(AteError::AlreadyExists {
                    message: "exists".into(),
                })
            } else {
                Err(AteError::AlreadyExists {
                    message: "built by a peer meanwhile".into(),
                })
            }
        },
        |seen, reference| {
            let golden = if seen == 0 {
                crashed("crashed")
            } else {
                built(reference)
            };
            Ok(template(reference, golden))
        },
        |_| Err(AteError::NotFound { message: "gone".into() }),
    );

    registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(control.creates(), 2);
    Ok(())
}

#[tokio::test]
async fn a_failed_delete_of_a_dead_template_is_an_error() -> TestResult {
    let control = ScriptedControl::scripted(
        |_| {
            Err(AteError::AlreadyExists {
                message: "exists".into(),
            })
        },
        |_, reference| Ok(template(reference, crashed("crashed"))),
        |_| {
            Err(AteError::FailedPrecondition {
                message: "revision changed".into(),
            })
        },
    );

    let error = registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert_eq!(error, backend("failed precondition: revision changed"));
    Ok(())
}

#[tokio::test]
async fn a_failed_rebuild_of_a_dead_template_is_an_error() -> TestResult {
    let control = ScriptedControl::scripted(
        |seen| {
            if seen == 0 {
                Err(AteError::AlreadyExists {
                    message: "exists".into(),
                })
            } else {
                Err(AteError::ResourceExhausted { message: "full".into() })
            }
        },
        |_, reference| Ok(template(reference, crashed("crashed"))),
        |_| Ok(()),
    );

    let error = registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert_eq!(error, backend("resource exhausted: full"));
    Ok(())
}

#[tokio::test]
async fn an_existing_template_with_a_golden_tag_is_not_rebuilt() -> TestResult {
    let control = ScriptedControl::scripted(
        |_| {
            Err(AteError::AlreadyExists {
                message: "exists".into(),
            })
        },
        |_, reference| Ok(template(reference, built(reference))),
        |_| Ok(()),
    );

    registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(control.deletes(), 0);
    Ok(())
}

#[tokio::test]
async fn an_invalid_agent_config_is_refused_before_any_work() -> TestResult {
    let config = SubstrateConfig {
        agent_port: 0,
        ..config()?
    };
    let control = ScriptedControl::ready_after(0);

    let error = registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert!(matches!(error, Some(TemplateError::Backend { .. })), "{error:?}");
    assert_eq!(control.gets() + control.creates(), 0);
    Ok(())
}

#[tokio::test]
async fn an_unpinned_image_is_refused_before_any_work() -> TestResult {
    let unpinned = TemplateKey::new(
        "registry.example/img:latest",
        SandboxClass::Gvisor,
        &config()?,
        "small",
        ActorResources::new(1000, 2048)?,
    );
    let control = ScriptedControl::ready_after(0);

    let error = registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &unpinned, DEADLINE)
        .await
        .err();

    assert!(matches!(error, Some(TemplateError::Backend { .. })), "{error:?}");
    assert_eq!(control.gets() + control.creates(), 0);
    Ok(())
}

#[tokio::test]
async fn a_microvm_below_its_memory_floor_is_refused() -> TestResult {
    let small = TemplateKey::new(
        IMAGE,
        SandboxClass::MicroVm,
        &config()?,
        "small",
        ActorResources::new(1000, 64)?,
    );
    let control = ScriptedControl::ready_after(0);

    let error = registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &small, DEADLINE)
        .await
        .err();

    assert_eq!(error, backend("microvm actors require at least 128 MiB"));
    Ok(())
}

#[tokio::test]
async fn a_microvm_at_its_memory_floor_is_built() -> TestResult {
    let at_floor = key_with(IMAGE, SandboxClass::MicroVm, &config()?.agent_image, 8110, "small")?;
    let control = ScriptedControl::ready_after(0);

    registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &at_floor, Duration::from_secs(5))
        .await?;

    assert_eq!(control.creates(), 1);
    Ok(())
}
