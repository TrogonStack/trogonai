//! Authorization for command execution.
//!
//! Per [ADR#0026](https://github.com/TrogonStack/trogonai/blob/main/docs/adr/0026-command-authorization-principal.md),
//! this module is a seam, not a policy. It owns the [`CommandPrincipal`] value
//! object, the [`CommandAuthorizer`] hook, the [`UnauthorizedError`] rejection,
//! and the no-op default. It deliberately does not own a policy language, a
//! claim vocabulary, or any rule about which principal may run which command:
//! those are application facts, and no two consumers of this crate would agree
//! on them.
//!
//! Execution is therefore unauthorized unless a caller opts in with
//! `with_authorizer`. What the seam adds over a check in each caller's own
//! dispatch loop is that the guarantee becomes a property of the execution
//! rather than of the caller: an execution carrying [`WithoutAuthorization`]
//! was never checked, and one carrying anything else was, whoever built it and
//! whichever path it took.
//!
//! # The principal is only as trustworthy as its source
//!
//! Nothing here verifies an identity. A [`CommandPrincipal`] means "the
//! boundary that built this claims it verified this actor", and this crate has
//! no way to tell a principal an authenticating gateway minted from one a
//! caller typed by hand. Constructing principals from verified credentials is
//! the boundary's job, and ADR#0026's Decision 4 covers it; it is not
//! implemented anywhere yet.

use std::borrow::{Borrow, Cow};
use std::collections::BTreeSet;
use std::collections::btree_set;
use std::str::FromStr;
use std::sync::Arc;

/// What kind of actor a principal represents.
///
/// Kept closed rather than open: an authorizer that must branch on the actor
/// type can only do so exhaustively if the set is known, and a fourth kind is
/// a decision this ADR should be revisited for rather than a value a caller
/// invents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrincipalKind {
    /// An autonomous agent acting on its own behalf.
    Agent,
    /// A human, whether acting directly or through a client.
    Person,
    /// A non-agent workload, such as a scheduler or a background processor.
    Service,
}

impl PrincipalKind {
    /// Returns the stable wire name of this kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Person => "person",
            Self::Service => "service",
        }
    }
}

impl std::fmt::Display for PrincipalKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}

/// Stable identifier of the actor a command is executed on behalf of.
///
/// Stable is the operative word: an authorizer that grants on an identifier
/// that can be reassigned grants to whoever holds it next. What makes an
/// identifier stable is the issuing boundary's problem, not this type's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrincipalId(String);

impl PrincipalId {
    /// Creates a principal identifier after rejecting invalid input.
    pub fn new(value: impl Into<String>) -> Result<Self, PrincipalIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PrincipalIdError::Empty);
        }
        if value.chars().any(char::is_control) {
            return Err(PrincipalIdError::ContainsControlCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the identifier as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for PrincipalId {
    type Err = PrincipalIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for PrincipalId {
    type Error = PrincipalIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for PrincipalId {
    type Error = PrincipalIdError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for PrincipalId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for PrincipalId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when constructing an invalid [`PrincipalId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PrincipalIdError {
    /// Principal identifiers cannot be empty.
    #[error("principal id cannot be empty")]
    Empty,
    /// Principal identifiers cannot contain control characters.
    #[error("principal id cannot contain control characters")]
    ContainsControlCharacter,
}

/// One opaque capability, scope, or role a principal was granted.
///
/// The string is not interpreted here. This crate defines only that claims are
/// non-empty, comparable, and set-like; what any given claim authorizes is the
/// authorizer's vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrincipalClaim(String);

impl PrincipalClaim {
    /// Creates a claim after rejecting invalid input.
    pub fn new(value: impl Into<String>) -> Result<Self, PrincipalClaimError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PrincipalClaimError::Empty);
        }
        if value.chars().any(char::is_control) {
            return Err(PrincipalClaimError::ContainsControlCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the claim as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for PrincipalClaim {
    type Err = PrincipalClaimError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for PrincipalClaim {
    type Error = PrincipalClaimError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for PrincipalClaim {
    type Error = PrincipalClaimError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for PrincipalClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for PrincipalClaim {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for PrincipalClaim {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when constructing an invalid [`PrincipalClaim`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PrincipalClaimError {
    /// Claims cannot be empty.
    #[error("principal claim cannot be empty")]
    Empty,
    /// Claims cannot contain control characters.
    #[error("principal claim cannot contain control characters")]
    ContainsControlCharacter,
}

/// The set of claims a principal carries.
///
/// A set rather than a list: a claim granted twice grants nothing more, and an
/// authorizer asking whether a claim is present should not have to care what
/// order the issuing boundary emitted them in.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrincipalClaims(BTreeSet<PrincipalClaim>);

impl PrincipalClaims {
    /// Returns a principal with no claims at all.
    pub const fn empty() -> Self {
        Self(BTreeSet::new())
    }

    /// Adds a claim, returning whether it was not already present.
    pub fn insert(&mut self, claim: PrincipalClaim) -> bool {
        self.0.insert(claim)
    }

    /// Returns whether this set contains the named claim.
    pub fn contains(&self, claim: &str) -> bool {
        self.0.contains(claim)
    }

    /// Iterates the claims in sorted order.
    pub fn iter(&self) -> btree_set::Iter<'_, PrincipalClaim> {
        self.0.iter()
    }

    /// Returns how many distinct claims are present.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether this principal carries no claims.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<PrincipalClaim> for PrincipalClaims {
    fn from_iter<I: IntoIterator<Item = PrincipalClaim>>(claims: I) -> Self {
        Self(claims.into_iter().collect())
    }
}

impl<'a> IntoIterator for &'a PrincipalClaims {
    type Item = &'a PrincipalClaim;
    type IntoIter = btree_set::Iter<'a, PrincipalClaim>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for PrincipalClaims {
    type Item = PrincipalClaim;
    type IntoIter = btree_set::IntoIter<PrincipalClaim>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// The user an agent says it is acting for, as carried by AAuth's
/// `aa-auth+jwt` `principal` claim.
///
/// Held apart from [`PrincipalClaims`] because it is not a claim: it is an
/// optional, unstructured string on the wire that names a directed user
/// without proving anything about them. Treat it as a hint for audit and
/// correlation. An authorizer that grants on it grants on an assertion nobody
/// verified.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DirectedPrincipal(String);

impl DirectedPrincipal {
    /// Creates a directed-principal hint after rejecting invalid input.
    pub fn new(value: impl Into<String>) -> Result<Self, DirectedPrincipalError> {
        let value = value.into();
        if value.is_empty() {
            return Err(DirectedPrincipalError::Empty);
        }
        if value.chars().any(char::is_control) {
            return Err(DirectedPrincipalError::ContainsControlCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the hint as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for DirectedPrincipal {
    type Err = DirectedPrincipalError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for DirectedPrincipal {
    type Error = DirectedPrincipalError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for DirectedPrincipal {
    type Error = DirectedPrincipalError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for DirectedPrincipal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for DirectedPrincipal {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when constructing an invalid [`DirectedPrincipal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DirectedPrincipalError {
    /// A present directed principal cannot be the empty string.
    ///
    /// Absent and empty are different answers, and the wire shape can express
    /// both. Absence is [`None`]; an empty string is a boundary that lost the
    /// value somewhere.
    #[error("directed principal cannot be empty")]
    Empty,
    /// Directed principals cannot contain control characters.
    #[error("directed principal cannot contain control characters")]
    ContainsControlCharacter,
}

/// Who a command is being executed on behalf of.
///
/// Distinct from [`Headers`](crate::Headers), which is envelope metadata for
/// the stored event. A principal is an authorization-time input evaluated
/// before `decide` runs and is not persisted by this crate. An application
/// that wants an audit trail of who acted derives its own header from the
/// principal, the same way it derives every other header it requires.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandPrincipal {
    kind: PrincipalKind,
    id: PrincipalId,
    claims: PrincipalClaims,
    directed_principal: Option<DirectedPrincipal>,
}

impl CommandPrincipal {
    /// Names an actor with no claims and no directed-principal hint.
    pub const fn new(kind: PrincipalKind, id: PrincipalId) -> Self {
        Self {
            kind,
            id,
            claims: PrincipalClaims::empty(),
            directed_principal: None,
        }
    }

    /// Attaches the claim set the issuing boundary verified.
    pub fn with_claims(mut self, claims: PrincipalClaims) -> Self {
        self.claims = claims;
        self
    }

    /// Attaches the unverified directed-principal hint.
    pub fn with_directed_principal<D>(mut self, directed_principal: D) -> Self
    where
        D: Into<Option<DirectedPrincipal>>,
    {
        self.directed_principal = directed_principal.into();
        self
    }

    /// Returns what kind of actor this is.
    pub const fn kind(&self) -> PrincipalKind {
        self.kind
    }

    /// Returns the stable identifier of this actor.
    pub const fn id(&self) -> &PrincipalId {
        &self.id
    }

    /// Returns the claims this actor carries.
    pub const fn claims(&self) -> &PrincipalClaims {
        &self.claims
    }

    /// Returns whether this actor carries the named claim.
    pub fn has_claim(&self, claim: &str) -> bool {
        self.claims.contains(claim)
    }

    /// Returns the unverified directed-principal hint, if the boundary
    /// supplied one.
    pub const fn directed_principal(&self) -> Option<&DirectedPrincipal> {
        self.directed_principal.as_ref()
    }
}

impl std::fmt::Display for CommandPrincipal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.kind, self.id)
    }
}

/// An authorizer's answer when a principal may not run a command.
///
/// The reason is a human-readable explanation for logs and error responses,
/// not a machine-readable code: this crate would have to own a vocabulary of
/// denial codes to offer one, and that vocabulary is the policy language
/// ADR#0026 declares a Non-Goal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("command denied for this principal: {reason}")]
pub struct AuthorizationDenied {
    reason: Cow<'static, str>,
}

impl AuthorizationDenied {
    /// Denies a command with the given explanation.
    pub fn new(reason: impl Into<Cow<'static, str>>) -> Self {
        Self { reason: reason.into() }
    }

    /// Returns why the command was denied.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// Why an execution was not authorized.
///
/// Separate from [`AuthorizationDenied`] because the two failures have
/// different causes and different fixes. A denial is a policy answer about a
/// known actor; a missing principal is a caller that configured an authorizer
/// and then did not say who was acting, which is a wiring bug at the boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnauthorizedError {
    /// An authorizer is configured but the execution carries no principal.
    ///
    /// Never treated as an anonymous or default-trust principal: an authorizer
    /// that was asked to decide and was given nobody to decide about has no
    /// safe answer other than no.
    #[error("command execution has an authorizer configured but no principal")]
    MissingPrincipal,
    /// The authorizer refused the command for this principal.
    #[error("{0}")]
    Denied(#[source] AuthorizationDenied),
}

/// Decides whether a principal may run a command, before anything is read,
/// replayed, decided, or appended.
///
/// Consulted once per execution, outside the conflict-retry loop, so a command
/// that re-reads and decides again is still authorized exactly once.
///
/// [`authorize`](Self::authorize) is deliberately synchronous. An authorizer
/// that awaits a remote policy engine puts a network round trip in front of
/// every command, including the ones that would have been decided in
/// microseconds, and it does so on a call this crate offers no way to bound.
/// A host that needs a remote decision fetches it in its own dispatch loop and
/// hands the result down as claims on the principal.
pub trait CommandAuthorizer<C: ?Sized> {
    /// Decides whether this principal may run this command.
    ///
    /// The command is whatever the execution path carries: the decider command
    /// itself on the native path, and the command envelope on the WASM path.
    /// Neither path supplies the target stream or the replayed state, so a
    /// rule that needs to inspect stream state belongs in `decide`, where a
    /// rejection is already a first-class outcome.
    fn authorize(&self, principal: &CommandPrincipal, command: &C) -> Result<(), AuthorizationDenied>;

    /// Applies this authorizer to an execution that may carry no principal.
    ///
    /// This is what the runtime calls, and the default is fail-closed: an
    /// absent principal is rejected before [`authorize`](Self::authorize) is
    /// consulted, so an implementation gets that guarantee by writing nothing.
    /// Override it only to make an authorizer that genuinely permits anonymous
    /// execution, such as [`WithoutAuthorization`].
    fn authorize_execution(&self, principal: Option<&CommandPrincipal>, command: &C) -> Result<(), UnauthorizedError> {
        match principal {
            Some(principal) => self.authorize(principal, command).map_err(UnauthorizedError::Denied),
            None => Err(UnauthorizedError::MissingPrincipal),
        }
    }
}

impl<A, C> CommandAuthorizer<C> for &A
where
    A: CommandAuthorizer<C> + ?Sized,
    C: ?Sized,
{
    fn authorize(&self, principal: &CommandPrincipal, command: &C) -> Result<(), AuthorizationDenied> {
        (*self).authorize(principal, command)
    }

    fn authorize_execution(&self, principal: Option<&CommandPrincipal>, command: &C) -> Result<(), UnauthorizedError> {
        (*self).authorize_execution(principal, command)
    }
}

impl<A, C> CommandAuthorizer<C> for Arc<A>
where
    A: CommandAuthorizer<C> + ?Sized,
    C: ?Sized,
{
    fn authorize(&self, principal: &CommandPrincipal, command: &C) -> Result<(), AuthorizationDenied> {
        self.as_ref().authorize(principal, command)
    }

    fn authorize_execution(&self, principal: Option<&CommandPrincipal>, command: &C) -> Result<(), UnauthorizedError> {
        self.as_ref().authorize_execution(principal, command)
    }
}

/// The default: no authorization phase at all, exactly as it was before this
/// module existed.
///
/// Not the same thing as an allow-all policy, though it behaves identically.
/// An allow-all authorizer is a decision someone made and can be audited;
/// this is the absence of one, and it is visible in the execution's type so
/// the difference stays legible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutAuthorization;

impl<C> CommandAuthorizer<C> for WithoutAuthorization
where
    C: ?Sized,
{
    fn authorize(&self, _principal: &CommandPrincipal, _command: &C) -> Result<(), AuthorizationDenied> {
        Ok(())
    }

    fn authorize_execution(
        &self,
        _principal: Option<&CommandPrincipal>,
        _command: &C,
    ) -> Result<(), UnauthorizedError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
