use crate::access::Access;
use crate::exclusion_conflict::ExclusionConflict;
use crate::exclusion_kind::ExclusionKind;
use crate::relation::Relation;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

/// The refusal raised for conflicts, with nothing granted.
pub fn exclusion_conflict_error(conflicts: &[ExclusionConflict]) -> WindowsSandboxError {
    let describe = |conflict: &ExclusionConflict| -> String {
        let list = match conflict.access {
            Access::Write => "allowWrite",
            Access::Read => "allowRead",
        };
        let exclusion = &conflict.exclusion;
        let what = match (conflict.kind, conflict.relation) {
            (ExclusionKind::UngrantedOnly, relation) => format!(
                "{} {exclusion}, which is not granted as a whole (grant a directory inside it \
                 instead)",
                if relation == Relation::Equal { "is" } else { "contains" }
            ),
            (_, Relation::Equal) => format!("is the excluded {exclusion}"),
            (_, Relation::Inside) => format!("lies inside the excluded {exclusion}"),
            (_, Relation::Above) => match &conflict.unexamined {
                Some(reason) => format!(
                    "may contain the excluded {exclusion}: {reason}, and a path is granted only \
                     once it has been looked at"
                ),
                None => format!("contains the excluded {exclusion}"),
            },
        };
        let kept = if conflict.access == Access::Write && conflict.relation == Relation::Above {
            ": a write grant on it is kept and inherited, so that path would be writable and \
             readable if it is ever created again - choose a narrower working directory"
        } else {
            ""
        };
        format!("{list} {} {what}{kept}", conflict.path)
    };
    WindowsSandboxError::new(
        Code::ProfileExclusionReadGrant,
        format!(
            "The session would grant a path that reaches one excluded from reads of your user \
             profile: {}. Nothing was granted. Remove the entry, or name a path that neither is \
             nor contains an excluded one.",
            conflicts.iter().map(describe).collect::<Vec<_>>().join("; ")
        ),
    )
}
