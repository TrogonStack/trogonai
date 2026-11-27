use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use crate::ate_client::ResourceRevision;
use crate::ate_client::{AteError, AteResult, Control};
use crate::proto;

type CreateScript = Box<dyn Fn(usize) -> AteResult<()> + Send + Sync>;
type GetScript = Box<dyn Fn(usize, &proto::ObjectRef) -> AteResult<proto::ActorTemplate> + Send + Sync>;
type DeleteScript = Box<dyn Fn(usize) -> AteResult<()> + Send + Sync>;

/// A `Control` whose template calls follow a script. Each script receives the zero-based
/// number of the call, so a test can say "the second create collapses to `AlreadyExists`".
pub(in crate::template) struct ScriptedControl {
    create: CreateScript,
    get: GetScript,
    delete: DeleteScript,
    creates: AtomicUsize,
    gets: AtomicUsize,
    deletes: AtomicUsize,
}

pub(in crate::template) fn template(
    reference: &proto::ObjectRef,
    golden: proto::GoldenSnapshotStatus,
) -> proto::ActorTemplate {
    proto::ActorTemplate {
        metadata: Some(proto::ResourceMetadata {
            atespace: reference.atespace.clone(),
            name: reference.name.clone(),
            uid: "template-uid".into(),
            version: 1,
            ..Default::default()
        }),
        status: Some(proto::ActorTemplateStatus {
            golden_snapshot_status: Some(golden),
        }),
        ..Default::default()
    }
}

pub(in crate::template) fn built(reference: &proto::ObjectRef) -> proto::GoldenSnapshotStatus {
    proto::GoldenSnapshotStatus {
        golden_tag: Some(proto::ObjectRef {
            atespace: reference.atespace.clone(),
            name: format!("golden-{}", reference.name),
        }),
        ..Default::default()
    }
}

pub(in crate::template) fn crashed(message: &str) -> proto::GoldenSnapshotStatus {
    proto::GoldenSnapshotStatus {
        error_message: message.to_string(),
        ..Default::default()
    }
}

impl ScriptedControl {
    /// Every create succeeds, every delete succeeds, and the golden snapshot exists from
    /// the `ready_after`-th read on.
    pub(in crate::template) fn ready_after(ready_after: usize) -> Self {
        Self::scripted(
            |_| Ok(()),
            move |seen, reference| {
                let golden = if seen >= ready_after {
                    built(reference)
                } else {
                    proto::GoldenSnapshotStatus::default()
                };
                Ok(template(reference, golden))
            },
            |_| Ok(()),
        )
    }

    pub(in crate::template) fn scripted(
        create: impl Fn(usize) -> AteResult<()> + Send + Sync + 'static,
        get: impl Fn(usize, &proto::ObjectRef) -> AteResult<proto::ActorTemplate> + Send + Sync + 'static,
        delete: impl Fn(usize) -> AteResult<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            create: Box::new(create),
            get: Box::new(get),
            delete: Box::new(delete),
            creates: AtomicUsize::new(0),
            gets: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
        }
    }

    pub(in crate::template) fn creates(&self) -> usize {
        self.creates.load(Ordering::SeqCst)
    }

    pub(in crate::template) fn gets(&self) -> usize {
        self.gets.load(Ordering::SeqCst)
    }

    pub(in crate::template) fn deletes(&self) -> usize {
        self.deletes.load(Ordering::SeqCst)
    }
}

fn outside_the_template_surface<T>() -> AteResult<T> {
    Err(AteError::Other {
        message: "not part of the template surface".into(),
    })
}

#[async_trait]
impl Control for ScriptedControl {
    async fn create_atespace(&self, _: &str) -> AteResult<proto::Atespace> {
        outside_the_template_surface()
    }

    async fn get_actor(&self, _: proto::ObjectRef) -> AteResult<proto::Actor> {
        outside_the_template_surface()
    }

    async fn create_actor(&self, _: proto::Actor) -> AteResult<proto::Actor> {
        outside_the_template_surface()
    }

    async fn resume_actor(&self, _: proto::ObjectRef) -> AteResult<proto::Actor> {
        outside_the_template_surface()
    }

    async fn suspend_actor(&self, _: proto::ObjectRef) -> AteResult<proto::Actor> {
        outside_the_template_surface()
    }

    async fn delete_actor(&self, _: ResourceRevision) -> AteResult<proto::Actor> {
        outside_the_template_surface()
    }

    async fn list_actors(&self, _: &str) -> AteResult<Vec<proto::Actor>> {
        outside_the_template_surface()
    }

    async fn create_actor_template(&self, template: proto::ActorTemplate) -> AteResult<proto::ActorTemplate> {
        let seen = self.creates.fetch_add(1, Ordering::SeqCst);
        (self.create)(seen)?;
        Ok(template)
    }

    async fn get_actor_template(&self, template: proto::ObjectRef) -> AteResult<proto::ActorTemplate> {
        let seen = self.gets.fetch_add(1, Ordering::SeqCst);
        (self.get)(seen, &template)
    }

    async fn delete_actor_template(&self, _: ResourceRevision) -> AteResult<()> {
        let seen = self.deletes.fetch_add(1, Ordering::SeqCst);
        (self.delete)(seen)
    }
}

#[tokio::test]
async fn the_actor_half_of_the_control_surface_is_not_scripted() -> Result<(), Box<dyn std::error::Error>> {
    let control = ScriptedControl::ready_after(0);
    let reference = proto::ObjectRef::default();
    let revision = ResourceRevision::from_metadata(
        template(
            &proto::ObjectRef {
                atespace: "a".into(),
                name: "n".into(),
            },
            proto::GoldenSnapshotStatus::default(),
        )
        .metadata
        .as_ref(),
    )?;

    assert!(control.create_atespace("a").await.is_err());
    assert!(control.get_actor(reference.clone()).await.is_err());
    assert!(control.create_actor(proto::Actor::default()).await.is_err());
    assert!(control.resume_actor(reference.clone()).await.is_err());
    assert!(control.suspend_actor(reference).await.is_err());
    assert!(control.delete_actor(revision).await.is_err());
    assert!(control.list_actors("a").await.is_err());
    Ok(())
}
