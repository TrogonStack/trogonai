use crate::action::Action;
use crate::chain::Chain;
use crate::phase::Phase;
use crate::subject::Subject;

/// Why a [`Request`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequestError {
    /// The lineage is the whole delegation chain and so contains the
    /// dispatching Agent's own chain. A lineage that does not would let a
    /// ceiling or hold on the caller's own chain go unread.
    #[error("lineage does not contain {missing} from the own chain")]
    LineageMissesOwnChain {
        /// The first subject of the own chain that the lineage lacks.
        missing: Subject,
    },
}

/// One thing to decide: who is asking, over which lineage, for what, and when
/// in the Session's life.
///
/// Permits read the own chain. Ceilings and holds read the lineage
/// (ADR#0071 Decision 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    own_chain: Chain,
    lineage: Chain,
    action: Action,
    phase: Phase,
}

impl Request {
    /// Builds a request, refusing a lineage that omits part of the own chain.
    pub fn new(own_chain: Chain, lineage: Chain, action: Action, phase: Phase) -> Result<Self, RequestError> {
        if let Some(missing) = own_chain.iter().find(|subject| !lineage.contains(subject)) {
            return Err(RequestError::LineageMissesOwnChain {
                missing: missing.clone(),
            });
        }
        Ok(Self {
            own_chain,
            lineage,
            action,
            phase,
        })
    }

    /// Builds a request with no delegation behind it, where the lineage is the
    /// own chain.
    pub fn without_delegation(own_chain: Chain, action: Action, phase: Phase) -> Self {
        Self {
            lineage: own_chain.clone(),
            own_chain,
            action,
            phase,
        }
    }

    /// The dispatching Agent followed by its ancestor nodes. Permits read this.
    pub fn own_chain(&self) -> &Chain {
        &self.own_chain
    }

    /// The delegation lineage. Ceilings and holds read this.
    pub fn lineage(&self) -> &Chain {
        &self.lineage
    }

    /// What is asked for.
    pub fn action(&self) -> &Action {
        &self.action
    }

    /// Where in the Session's life it is asked.
    pub fn phase(&self) -> Phase {
        self.phase
    }
}
