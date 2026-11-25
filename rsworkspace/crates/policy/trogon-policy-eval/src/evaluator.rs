use std::collections::BTreeSet;
use std::time::SystemTime;

use crate::action::Action;
use crate::decision::Decision;
use crate::enforcement_mode::EnforcementMode;
use crate::evaluator_error::EvaluatorError;
use crate::hold::Hold;
use crate::narrowing::Narrowing;
use crate::refusal::Refusal;
use crate::request::Request;
use crate::request_rule_engine::RequestRuleEngine;
use crate::rule_kind::RuleKind;
use crate::rule_name::RuleName;
use crate::rule_ref::RuleRef;
use crate::snapshot::PolicySnapshot;
use crate::staleness_budget::StalenessBudget;
use crate::subject::Subject;
use crate::view::PolicyView;

/// Decides one request against one snapshot of the bindings.
///
/// The decision is the conjunction of ADR#0071 Decision 2: some permit on the
/// dispatching Agent's own chain covers the action, every ceiling on the
/// lineage admits it, no hold on the lineage refuses it in this phase, and no
/// request rule refuses it. There is no precedence between bindings, so the
/// result does not depend on the order of anything the caller supplies.
///
/// Evaluation is pure: it reads the snapshot and the arguments, takes `now` from
/// the caller, and touches no clock, file, or network.
#[derive(Debug, Clone, Copy)]
pub struct Evaluator {
    mode: EnforcementMode,
    budget: StalenessBudget,
}

/// The histories read so far and the refusals found, collected before the one
/// to report is chosen.
#[derive(Debug, Default)]
struct Findings {
    consulted: BTreeSet<RuleRef>,
    refusals: Vec<Refusal>,
}

impl Evaluator {
    /// Creates an evaluator. The mode has no default and the budget cannot
    /// exceed the ceiling of ADR#0071 Decision 7.
    pub const fn new(mode: EnforcementMode, staleness_budget: StalenessBudget) -> Self {
        Self {
            mode,
            budget: staleness_budget,
        }
    }

    /// The enforcement mode.
    pub const fn mode(&self) -> EnforcementMode {
        self.mode
    }

    /// The staleness budget.
    pub const fn staleness_budget(&self) -> StalenessBudget {
        self.budget
    }

    /// Decides a request, with the results of the request rules the caller has
    /// already run.
    ///
    /// Rules that were not run because the bindings already refuse are not an
    /// error: a binding refusal is always reported before a request-rule
    /// refusal, so running them could not change the answer.
    pub fn evaluate(
        &self,
        view: &dyn PolicyView,
        request: &Request,
        narrowing: &[Narrowing],
        now: SystemTime,
    ) -> Decision {
        self.run(view, request, now, || {
            narrowing.iter().filter_map(Narrowing::denial).cloned().collect()
        })
    }

    /// Decides a request and runs a request-rule engine only when the bindings
    /// admit it.
    ///
    /// This is the composition entry point: bindings admission AND the engine's
    /// result, with the engine unable to widen anything.
    pub fn evaluate_with<E>(
        &self,
        view: &dyn PolicyView,
        request: &Request,
        engine: &E,
        facts: &E::Facts,
        now: SystemTime,
    ) -> Decision
    where
        E: RequestRuleEngine + ?Sized,
    {
        self.run(view, request, now, || {
            engine.narrow(facts).denial().cloned().into_iter().collect()
        })
    }

    fn run(
        &self,
        view: &dyn PolicyView,
        request: &Request,
        now: SystemTime,
        denying_rules: impl FnOnce() -> Vec<RuleName>,
    ) -> Decision {
        let snapshot = view.snapshot();
        if let Some(refusal) = self.check_freshness(&snapshot, now) {
            return Decision::new(self.mode, Some(refusal), Vec::new());
        }

        let mut findings = Findings::default();
        check_permit(&snapshot, request, &mut findings);
        check_ceilings(&snapshot, request, &mut findings);
        check_holds(&snapshot, request, &mut findings);
        if findings.refusals.is_empty() {
            findings
                .refusals
                .extend(denying_rules().into_iter().map(Refusal::RequestRuleDenied));
        }

        let refusal = findings.refusals.into_iter().min();
        Decision::new(self.mode, refusal, findings.consulted.into_iter().collect())
    }

    fn check_freshness(&self, snapshot: &PolicySnapshot, now: SystemTime) -> Option<Refusal> {
        match now.duration_since(snapshot.as_of()) {
            Ok(age) if age > self.budget.get() => Some(Refusal::StaleSnapshot {
                age,
                budget: self.budget.get(),
            }),
            Ok(_) => None,
            Err(ahead) => Some(Refusal::EvaluatorError(EvaluatorError::SnapshotFromTheFuture {
                by: ahead.duration(),
            })),
        }
    }
}

fn check_permit(snapshot: &PolicySnapshot, request: &Request, findings: &mut Findings) {
    match request.action() {
        Action::EgressConnect { host, port } => {
            require_permit(request, RuleKind::EgressPermission, snapshot, findings, |subject| {
                snapshot
                    .egress_permission(subject)
                    .is_some_and(|permission| permission.covers(host, *port))
            })
        }
        Action::Delegate { callee } => {
            require_permit(request, RuleKind::DelegationPermission, snapshot, findings, |subject| {
                snapshot
                    .delegation_permission(subject)
                    .is_some_and(|permission| permission.covers(callee))
            })
        }
        // Model selection has no permit here: ADR#0032 authorizes models
        // through a connection's allowed models and ADR#0071 Decision 5 makes
        // the ceiling a narrowing of that grant, not a second copy of it. The
        // permit for a model selection is the connection's own list, which is
        // checked where the connection is resolved.
        Action::ModelUse { .. } => {}
    }
}

fn require_permit(
    request: &Request,
    kind: RuleKind,
    snapshot: &PolicySnapshot,
    findings: &mut Findings,
    covers: impl Fn(&Subject) -> bool,
) {
    let mut covered = false;
    for subject in request.own_chain() {
        findings.consulted.insert(snapshot.rule_ref(kind, subject));
        covered |= covers(subject);
    }
    if !covered {
        findings.refusals.push(Refusal::NoPermit(kind));
    }
}

fn check_ceilings(snapshot: &PolicySnapshot, request: &Request, findings: &mut Findings) {
    let Action::ModelUse { model } = request.action() else {
        return;
    };
    for subject in request.lineage() {
        let rule = snapshot.rule_ref(RuleKind::ModelCeiling, subject);
        findings.consulted.insert(rule.clone());
        if let Some(ceiling) = snapshot.model_ceiling(subject)
            && !ceiling.admits(model)
        {
            findings.refusals.push(Refusal::CeilingExcludes(rule));
        }
    }
}

fn check_holds(snapshot: &PolicySnapshot, request: &Request, findings: &mut Findings) {
    for subject in request.lineage() {
        consult_holds(snapshot, subject, findings, |hold| hold.refuses_in(request.phase()));
    }
    // A delegation starts new work in the callee, so every hold on the callee's
    // chain refuses it, whatever scope the hold has and whichever phase the
    // dispatching Session is in.
    if let Action::Delegate { callee } = request.action() {
        for subject in callee.chain() {
            consult_holds(snapshot, subject, findings, |_| true);
        }
    }
}

fn consult_holds(
    snapshot: &PolicySnapshot,
    subject: &Subject,
    findings: &mut Findings,
    refuses: impl Fn(&Hold) -> bool,
) {
    let rule = snapshot.rule_ref(RuleKind::Hold, subject);
    findings.consulted.insert(rule.clone());
    for hold in snapshot.holds(subject).iter().filter(|hold| refuses(hold)) {
        findings.refusals.push(Refusal::HoldPlaced {
            rule: rule.clone(),
            hold: hold.name().clone(),
        });
    }
}
