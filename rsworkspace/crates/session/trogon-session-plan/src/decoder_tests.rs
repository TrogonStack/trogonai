#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::{
    owner::{PlanSourceLease, RuntimePlanInterpreter},
    source,
};
use std::{
    future::pending,
    time::{Duration, Instant},
};
use trogon_session_store::SessionAdmissionLease;

#[test]
fn field_scanner_retains_bounds_when_used_without_the_public_preflight() {
    for bytes in [
        [vec![9], vec![0; 8], vec![98, 1, 7]].concat(),
        [vec![13], vec![0; 4], vec![98, 1, 7]].concat(),
    ] {
        assert_eq!(original_field_payload(&bytes, 12), Ok(Some([7].as_slice())));
    }
    assert_eq!(original_field_payload(&[8, 1], 12), Ok(None));
    for bytes in [
        vec![11],
        vec![9, 0],
        vec![13, 0],
        vec![98, 2, 0],
        vec![128],
        vec![98, 128],
    ] {
        assert_eq!(original_field_payload(&bytes, 12), Err(PlanError::Wire));
    }
}

#[test]
fn tenth_varint_byte_accepts_only_one_remaining_bit() {
    for final_byte in 0..=255u8 {
        let mut original = vec![128; 9];
        original.push(final_byte);
        let mut offset = 0;
        let expected = if final_byte <= 1 {
            Ok(u64::from(final_byte) << 63)
        } else {
            Err(PlanError::Wire)
        };
        assert_eq!(varint(&original, &mut offset), expected, "terminal byte {final_byte}");
    }
    assert_eq!(varint(&[128; 9], &mut 0), Err(PlanError::Wire));
}

struct UnreadableSource;
impl SessionAdmissionLease for UnreadableSource {
    type Error = PlanError;
    fn require_live(&self) -> Result<(), PlanError> {
        Ok(())
    }
    fn deadline(&self) -> Instant {
        Instant::now() + Duration::from_secs(1)
    }
    async fn withdrawn(&self) {
        pending::<()>().await
    }
}
impl PlanSourceLease for UnreadableSource {
    fn revision(&self) -> &agent::AgentRevisionRef {
        panic!("invalid binding must refuse before reading child authority")
    }
    fn original_configuration(&self) -> Result<&[u8], PlanError> {
        Err(PlanError::SourceUnavailable)
    }
    fn candidate(&self) -> Option<&session::CandidateRef> {
        None
    }
    fn project_id(&self) -> &str {
        "project-example"
    }
    fn interpreter(&self) -> &dyn RuntimePlanInterpreter {
        panic!("invalid binding must refuse before runtime interpretation")
    }
}

#[test]
fn selected_delegation_retains_defenses_against_invalid_internal_plan_state() {
    let decoder = RegisteredPlanDecoder::new(RegisteredPlanDecoder::compiled_contract()).unwrap();
    let revision = agent::AgentRevisionRef::default();
    let binding = session::DelegationBindingRef {
        binding_name: Some("delegate".into()),
        parent_execution_plan_digest: session::Digest {
            algorithm: "sha256".into(),
            value: vec![1; 32],
        }
        .into(),
    };
    let child = DecodedPlan {
        value: plan::SessionExecutionPlan::default(),
        original: Vec::new(),
        digest: [2; 32],
    };
    for mutation in 0..3 {
        let mut dependency = plan::ResolvedDependency {
            binding_name: Some("delegate".into()),
            declaring_resource: plan::DependencyDeclarer {
                resource: Some(plan::dependency_declarer::Resource::Agent(Box::new(revision.clone()))),
            }
            .into(),
            declaration: Some(plan::resolved_dependency::Declaration::DelegateDeclaration(
                Box::default(),
            )),
            resolution: Some(plan::resolved_dependency::Resolution::Delegate(Box::default())),
        };
        let expected = match mutation {
            0 => {
                dependency.declaring_resource.get_or_insert_default().resource =
                    Some(plan::dependency_declarer::Resource::Tool(Box::default()));
                PlanError::UnsupportedRuntime
            }
            1 => {
                dependency.declaration = Some(plan::resolved_dependency::Declaration::ToolDeclaration(Box::default()));
                PlanError::UnsupportedRuntime
            }
            _ => {
                dependency.resolution = Some(plan::resolved_dependency::Resolution::Unavailable(Box::default()));
                PlanError::OriginalSourceMismatch
            }
        };
        let value = plan::SessionExecutionPlan {
            agent_revision: revision.clone().into(),
            dependencies: vec![dependency],
            ..Default::default()
        };
        let parent = DecodedPlan {
            value,
            original: Vec::new(),
            digest: [1; 32],
        };
        assert_eq!(
            source::selected_delegate(&decoder, &parent, &child, &UnreadableSource, &binding).err(),
            Some(expected)
        );
    }
}
