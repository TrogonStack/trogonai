use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use super::memory_index::MemoryIndex;
use super::scripted_control_tests::{ScriptedControl, built, crashed, template};
use super::{DEADLINE, config, key, registry};
use crate::ate_client::AteError;
use crate::template::constants::FAILURE_TTL;
use crate::template::now_secs::now_secs;
use crate::template::{TemplateError, TemplatePhase, TemplateRecord, TemplateRegistry};

type TestResult = Result<(), Box<dyn Error>>;

fn record(phase: TemplatePhase, created_at: u64, message: Option<&str>) -> Result<TemplateRecord, Box<dyn Error>> {
    Ok(TemplateRecord {
        atespace: "tenant-a".to_string(),
        name: key()?.template_name(),
        phase,
        created_at,
        message: message.map(str::to_string),
    })
}

fn expired() -> u64 {
    now_secs() - FAILURE_TTL.as_secs() - 1
}

#[tokio::test]
async fn a_cold_image_builds_once_and_then_reports_ready() -> TestResult {
    let index = Arc::new(MemoryIndex::default());
    let control = ScriptedControl::ready_after(2);

    let reference = registry(index.clone())
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(reference.name, key()?.template_name());
    assert_eq!(reference.atespace, "tenant-a");
    assert_eq!(control.creates(), 1);
    let stored = index.record(&key()?.kv_key("tenant-a")).await.ok_or("record")?;
    assert_eq!(stored.phase, TemplatePhase::Ready);
    Ok(())
}

#[tokio::test]
async fn a_warm_template_checks_native_readiness_without_another_build() -> TestResult {
    let index = Arc::new(MemoryIndex::default());
    index
        .seed(
            &key()?.kv_key("tenant-a"),
            record(TemplatePhase::Ready, now_secs(), None)?,
        )
        .await;
    let control = ScriptedControl::ready_after(0);

    registry(index)
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(control.creates(), 0);
    assert_eq!(control.gets(), 1);
    Ok(())
}

#[tokio::test]
async fn concurrent_leases_on_a_cold_image_produce_exactly_one_build() -> TestResult {
    let index = Arc::new(MemoryIndex::default());
    let control = Arc::new(ScriptedControl::ready_after(4));
    let registry = Arc::new(registry(index));
    let config = Arc::new(config()?);
    let key = Arc::new(key()?);

    let mut handles = Vec::new();
    for _ in 0..8 {
        let registry = Arc::clone(&registry);
        let control = Arc::clone(&control);
        let config = Arc::clone(&config);
        let key = Arc::clone(&key);
        handles.push(tokio::spawn(async move {
            registry
                .ensure_ready(control.as_ref(), &config, "tenant-a", &key, DEADLINE)
                .await
        }));
    }

    for handle in handles {
        let reference = handle.await??;
        assert_eq!(reference.name, key.template_name());
    }

    assert_eq!(control.creates(), 1, "the KV create must elect exactly one builder");
    Ok(())
}

#[tokio::test]
async fn losing_the_election_waits_for_the_winners_build() -> TestResult {
    let index = Arc::new(MemoryIndex::with_faults(super::memory_index::Faults {
        lose_claim: true,
        ..Default::default()
    }));
    let control = ScriptedControl::ready_after(1);

    registry(index)
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(control.creates(), 0, "someone else holds the claim");
    Ok(())
}

#[tokio::test]
async fn an_index_outage_does_not_stop_the_lease() -> TestResult {
    let index = Arc::new(MemoryIndex::with_faults(super::memory_index::Faults {
        get: true,
        create: true,
        put: true,
        ..Default::default()
    }));
    let control = ScriptedControl::ready_after(0);

    registry(index)
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(control.creates(), 1);
    Ok(())
}

#[tokio::test]
async fn a_cached_build_in_progress_is_awaited_not_rebuilt() -> TestResult {
    let index = Arc::new(MemoryIndex::default());
    index
        .seed(
            &key()?.kv_key("tenant-a"),
            record(TemplatePhase::Building, now_secs(), None)?,
        )
        .await;
    let control = ScriptedControl::ready_after(1);

    registry(index)
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(control.creates(), 0);
    Ok(())
}

#[tokio::test]
async fn a_cache_entry_for_another_identity_is_refused() -> TestResult {
    let index = Arc::new(MemoryIndex::default());
    let mut stale = record(TemplatePhase::Ready, now_secs(), None)?;
    stale.atespace = "tenant-b".to_string();
    index.seed(&key()?.kv_key("tenant-a"), stale).await;
    let control = ScriptedControl::ready_after(0);

    let error = registry(index)
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert_eq!(
        error,
        Some(TemplateError::Backend {
            message: "template cache address differs from the requested identity".into()
        })
    );
    assert_eq!(control.gets(), 0);
    Ok(())
}

#[tokio::test]
async fn a_cached_failure_without_a_reason_gets_a_generic_one() -> TestResult {
    let index = Arc::new(MemoryIndex::default());
    index
        .seed(
            &key()?.kv_key("tenant-a"),
            record(TemplatePhase::Failed, now_secs(), None)?,
        )
        .await;
    let control = ScriptedControl::ready_after(0);

    let error = registry(index)
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert_eq!(
        error,
        Some(TemplateError::NotReady {
            image: key()?.image_reference().to_string(),
            reason: "golden snapshot build failed".into(),
        })
    );
    Ok(())
}

#[tokio::test]
async fn an_expired_failure_is_retried_rather_than_inherited() -> TestResult {
    let index = Arc::new(MemoryIndex::default());
    index
        .seed(
            &key()?.kv_key("tenant-a"),
            record(TemplatePhase::Failed, expired(), Some("a transient cluster fault"))?,
        )
        .await;

    let control = ScriptedControl::ready_after(0);
    registry(index)
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;
    assert_eq!(control.creates(), 1);
    Ok(())
}

#[tokio::test]
async fn an_expired_failure_is_retried_even_when_the_index_cannot_delete_it() -> TestResult {
    let index = Arc::new(MemoryIndex::with_faults(super::memory_index::Faults {
        delete: true,
        lose_claim: true,
        ..Default::default()
    }));
    index
        .seed(
            &key()?.kv_key("tenant-a"),
            record(TemplatePhase::Failed, expired(), Some("old"))?,
        )
        .await;

    let control = ScriptedControl::ready_after(0);
    registry(index)
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_build_that_overruns_the_deadline_is_not_cached_as_a_failure() -> TestResult {
    let index = Arc::new(MemoryIndex::default());
    let control = ScriptedControl::ready_after(usize::MAX);

    let error = registry(index.clone())
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, Duration::from_millis(5))
        .await
        .err();

    assert!(
        matches!(&error, Some(TemplateError::NotReady { reason, .. }) if reason.contains("did not finish")),
        "{error:?}"
    );
    let stored = index.record(&key()?.kv_key("tenant-a")).await.ok_or("record")?;
    assert_eq!(
        stored.phase,
        TemplatePhase::Building,
        "a slow build must stay claimed, not be poisoned"
    );
    Ok(())
}

#[tokio::test]
async fn a_template_that_is_not_visible_yet_or_still_settling_is_waited_for() -> TestResult {
    let control = ScriptedControl::scripted(
        |_| Ok(()),
        |seen, reference| match seen {
            0 => Err(AteError::NotFound {
                message: "not yet".into(),
            }),
            1 => Err(AteError::FailedPrecondition {
                message: "still building".into(),
            }),
            2 => Err(AteError::Unavailable { message: "flap".into() }),
            _ => Ok(template(reference, built(reference))),
        },
        |_| Ok(()),
    );

    registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(control.gets(), 4);
    Ok(())
}

#[tokio::test]
async fn an_unretryable_read_error_ends_the_wait() -> TestResult {
    let control = ScriptedControl::scripted(
        |_| Ok(()),
        |_, _| {
            Err(AteError::PermissionDenied {
                message: "forbidden".into(),
            })
        },
        |_| Ok(()),
    );

    let error = registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert!(matches!(error, Some(TemplateError::Backend { .. })), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn a_native_template_for_another_identity_is_refused() -> TestResult {
    let control = ScriptedControl::scripted(
        |_| Ok(()),
        |_, reference| {
            let mut other = reference.clone();
            other.name = "tpl-other".into();
            Ok(template(&other, built(&other)))
        },
        |_| Ok(()),
    );

    let error = registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert_eq!(
        error,
        Some(TemplateError::Backend {
            message: "native template address differs from the requested identity".into()
        })
    );
    Ok(())
}

#[tokio::test]
async fn a_native_template_without_metadata_is_refused() -> TestResult {
    let control = ScriptedControl::scripted(
        |_| Ok(()),
        |_, _| Ok(crate::proto::ActorTemplate::default()),
        |_| Ok(()),
    );

    let error = registry(Arc::new(MemoryIndex::default()))
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert!(matches!(error, Some(TemplateError::Backend { .. })), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn a_failed_golden_snapshot_is_reported_and_cached() -> TestResult {
    let index = Arc::new(MemoryIndex::default());
    let control = ScriptedControl::scripted(
        |_| Ok(()),
        |_, reference| Ok(template(reference, crashed("pull access denied"))),
        |_| Ok(()),
    );
    let registry = registry(index.clone());

    let error = registry
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();
    assert!(
        matches!(&error, Some(TemplateError::NotReady { image, reason })
            if image == key().map(|key| key.image_reference().to_string()).unwrap_or_default().as_str()
                && reason.contains("pull access denied")),
        "{error:?}"
    );
    let stored = index.record(&key()?.kv_key("tenant-a")).await.ok_or("record")?;
    assert_eq!(stored.phase, TemplatePhase::Failed);

    // The cached verdict answers immediately instead of paying the build timeout again.
    let fresh = ScriptedControl::ready_after(0);
    let second = registry
        .ensure_ready(&fresh, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();
    assert!(matches!(second, Some(TemplateError::NotReady { .. })));
    assert_eq!(fresh.gets(), 0);
    Ok(())
}

#[tokio::test]
async fn a_failure_that_cannot_be_recorded_is_still_reported() -> TestResult {
    let index = Arc::new(MemoryIndex::with_faults(super::memory_index::Faults {
        put: true,
        ..Default::default()
    }));
    let control = ScriptedControl::scripted(
        |_| Ok(()),
        |_, reference| Ok(template(reference, crashed("boom"))),
        |_| Ok(()),
    );

    let error = registry(index)
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await
        .err();

    assert!(matches!(error, Some(TemplateError::NotReady { .. })), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn the_default_poll_interval_is_used_until_overridden() -> TestResult {
    let index = Arc::new(MemoryIndex::default());
    let control = ScriptedControl::ready_after(0);

    TemplateRegistry::new(index)
        .ensure_ready(&control, &config()?, "tenant-a", &key()?, DEADLINE)
        .await?;

    assert_eq!(control.gets(), 1, "a ready template needs no sleep");
    Ok(())
}
