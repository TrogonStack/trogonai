use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;
use trogon_sandbox_contract::Host;
use trogon_sandbox_contract::{AddressDenialClass, DecideMethod, EgressViolationReason, NetworkPolicy};
use trogon_sandbox_contract::{DenyReason, NetworkOptions};

use super::admission::Admission;
use super::refused::Refused;
use super::token::ProxyToken;
use super::{ConnectTarget, IngressLimits};
use crate::decider::{Decider, Judgment};
use crate::deny_explanation::{deny_explanation, standard_reason};
use crate::dial;
use crate::host_kind::{HostClassification, HostKind};
use crate::policy::{Ruling, allows_by_address, refuses_address, rule, rule_host};
use crate::resolve::{AllowRanges, ResolveError, VettedAddr, VettingResolver, carries_sentinel};
use crate::tunnel_gate::TunnelGate;
use crate::violation_store::resolved_to_words;
use trogon_sandbox_protocol::{StandardDenyReason, unlisted_destinations_refuse};

/// The explicit proxy: owns the token, the resolver and the tunnel cap.
pub struct Ingress {
    pub(super) token: Option<ProxyToken>,
    pub(super) open_socks: bool,
    pub(super) resolver: Arc<VettingResolver>,
    pub(super) limits: IngressLimits,
    pub(super) decider: Option<Arc<Decider>>,
    pub(super) options: Arc<NetworkOptions>,
    pub(super) gate: Arc<TunnelGate>,
    pub(super) audience: u64,
    pub(super) crl: Option<Arc<[u8]>>,
}

impl Ingress {
    pub fn new(token: ProxyToken, resolver: Arc<VettingResolver>, limits: IngressLimits) -> Self {
        Self::with_token(Some(token), resolver, limits)
    }

    pub(super) fn with_token(token: Option<ProxyToken>, resolver: Arc<VettingResolver>, limits: IngressLimits) -> Self {
        Self {
            token,
            open_socks: false,
            resolver,
            limits,
            decider: None,
            options: Arc::default(),
            gate: Arc::default(),
            audience: 0,
            crl: None,
        }
    }

    /// An ingress that serves HTTP clients without proxy credentials. SOCKS
    /// and any other first byte is answered with a 400 and closed.
    pub fn unauthenticated(resolver: Arc<VettingResolver>, limits: IngressLimits) -> Self {
        Self::with_token(None, resolver, limits)
    }

    /// An ingress that serves HTTP and SOCKS5 clients without proxy
    /// credentials.
    pub fn open(resolver: Arc<VettingResolver>, limits: IngressLimits) -> Self {
        Self {
            open_socks: true,
            ..Self::unauthenticated(resolver, limits)
        }
    }

    /// Asks `decider` about destinations the policy neither allows nor
    /// denies instead of denying them.
    pub fn with_decider(mut self, decider: Arc<Decider>) -> Self {
        self.decider = Some(decider);
        self
    }

    /// Answers a plain `GET` of the revocation list path with `crl`, before
    /// any proxy credentials are asked for.
    pub fn with_crl(mut self, crl: Arc<[u8]>) -> Self {
        self.crl = Some(crl);
        self
    }

    /// Applies the optional network behaviour: reasons for refusals, strict
    /// allowlisting and a configured tunnel cap.
    pub fn with_network_options(mut self, options: Arc<NetworkOptions>) -> Self {
        self.gate = Arc::new(TunnelGate::from_options(&options));
        self.audience = self.gate.audience();
        self.options = options;
        self
    }

    /// Shares the terminated-tunnel count, cap and handshake timeout with
    /// every other holder of `gate`, so a limit changed on it reaches tunnels
    /// opened afterwards.
    pub fn with_tunnel_gate(mut self, gate: Arc<TunnelGate>) -> Self {
        self.audience = gate.audience();
        self.gate = gate;
        self
    }

    /// Evaluates the named destination, then the resolved address. The name
    /// is screened before any DNS lookup. The resolver looks the name up once
    /// and returns the addresses that passed the SSRF floor, unless the
    /// policy's `allow_ranges` carve them out; only those are dialed, in
    /// order, and an address a rule would deny is dropped. A destination the policy leaves unlisted goes to the decider
    /// when there is one: a name before it is resolved, unless an allow range
    /// open on its port might still cover it, and everything else once its
    /// address is known. An explicit deny and the SSRF floor are never put to
    /// the decider. A name is resolved only when a rule allows it or an allow
    /// range open on its port might, so a name about to be refused never
    /// reaches DNS, and a name carrying a secret sentinel is refused before
    /// anything else.
    pub(super) async fn admit(
        &self,
        policy: &NetworkPolicy,
        target: &ConnectTarget,
        method: DecideMethod,
    ) -> Admission {
        let port = target.port().get();
        let literal_ip = match target.host().kind() {
            HostKind::Ip(ip) => Some(ip),
            HostKind::Domain(_) => None,
        };
        let by_name = self.options.names_matched_by_name.is_enabled() && literal_ip.is_none();
        let routed = self.options.reasoned_refusals.is_enabled()
            && dial::carries_name(
                &self.options,
                target.host(),
                !matches!(method, DecideMethod::Http(_)),
                port,
            );
        if let HostKind::Domain(name) = target.host().kind()
            && carries_sentinel(name.as_str())
        {
            return Admission::Denied {
                dest_ip: None,
                reason: EgressViolationReason::PolicyDenied,
                explanation: None,
            };
        }
        let mut decided = false;
        match rule_host(policy, target.host(), port) {
            Ruling::Deny => {
                return Admission::Denied {
                    dest_ip: literal_ip,
                    reason: EgressViolationReason::PolicyDenied,
                    explanation: deny_explanation(&self.options, policy, target.host(), port),
                };
            }
            Ruling::Allow(_) => {}
            Ruling::Unlisted if literal_ip.is_none() && !by_name && allows_by_address(policy, port) => {}
            Ruling::Unlisted => {
                if let Err(refused) = self.consult(target, method.clone(), routed).await {
                    return Admission::Denied {
                        dest_ip: literal_ip,
                        reason: refused.reason,
                        explanation: refused.explanation,
                    };
                }
                decided = true;
            }
        }

        let exceptions = AllowRanges::from_policy(policy).for_port(target.port().get());
        let mut vetted = if routed {
            vec![VettedAddr::unresolved(literal_ip)].into_iter()
        } else {
            let resolved = timeout(
                self.limits.resolve_timeout,
                self.resolver.resolve_all(target.host(), target.port(), &exceptions),
            )
            .await;
            match resolved {
                Ok(Ok(vetted)) => vetted.into_iter(),
                Ok(Err(ResolveError::Denied {
                    class,
                    addr,
                    classes,
                    refused,
                })) => {
                    let deny_listed = refused
                        .iter()
                        .find(|candidate| by_name && refuses_address(policy, **candidate, port));
                    let (class, addr, classes) = match deny_listed {
                        Some(listed) => {
                            let mut classes = classes;
                            if !classes.contains(&AddressDenialClass::DenyListed) {
                                classes.push(AddressDenialClass::DenyListed);
                            }
                            (AddressDenialClass::DenyListed, *listed, classes)
                        }
                        None => (class, addr, classes),
                    };
                    return Admission::Denied {
                        dest_ip: Some(addr),
                        reason: EgressViolationReason::DestinationAddressDenied(class),
                        explanation: DenyReason::new(&resolved_to_words(&classes)).ok(),
                    };
                }
                Ok(Err(ResolveError::Unresolved(err))) => return Admission::Unresolved(err),
                Ok(Err(ResolveError::CarriesSentinel)) => {
                    return Admission::Denied {
                        dest_ip: None,
                        reason: EgressViolationReason::PolicyDenied,
                        explanation: None,
                    };
                }
                Err(_) => return unresolved(io::ErrorKind::TimedOut),
            }
        };
        // The resolver never answers with an empty list, so this stays on one line.
        #[rustfmt::skip]
        let Some(dest) = vetted.next() else { return unresolved(io::ErrorKind::NotFound) };
        let mut rest: Vec<VettedAddr> = vetted.collect();
        let mut dest = dest;
        if by_name && !routed {
            let mut candidates = std::iter::once(dest).chain(rest).collect::<Vec<_>>();
            let refused = candidates
                .iter()
                .find(|addr| refuses_address(policy, addr.ip(), port))
                .copied();
            candidates.retain(|addr| !refuses_address(policy, addr.ip(), port));
            let mut kept = candidates.into_iter();
            match (kept.next(), refused) {
                (Some(first), _) => {
                    dest = first;
                    rest = kept.collect();
                }
                (None, refused) => {
                    return Admission::Denied {
                        dest_ip: refused.map(|addr| addr.ip()),
                        reason: EgressViolationReason::DestinationAddressDenied(AddressDenialClass::DenyListed),
                        explanation: DenyReason::new(&resolved_to_words(&[AddressDenialClass::DenyListed])).ok(),
                    };
                }
            }
        }

        let sni = match target.host().kind() {
            HostKind::Domain(name) => Some(name.as_str()),
            HostKind::Ip(_) => None,
        };
        let denied_at = |refused: Refused| Admission::Denied {
            dest_ip: dest.reported_ip(),
            reason: refused.reason,
            explanation: refused.explanation,
        };
        let ruling_of = |addr: &VettedAddr| {
            if by_name {
                rule_host(policy, target.host(), port)
            } else {
                rule(policy, sni, addr.ip(), port)
            }
        };
        let ruling = ruling_of(&dest);
        let fallbacks: Vec<VettedAddr> = rest
            .into_iter()
            .filter(|addr| {
                let other = ruling_of(addr);
                !matches!(other, Ruling::Deny)
                    && matches!(other, Ruling::Allow(_)) == matches!(ruling, Ruling::Allow(_))
            })
            .collect();
        match ruling {
            Ruling::Deny => denied_at(Refused {
                reason: EgressViolationReason::PolicyDenied,
                explanation: deny_explanation(&self.options, policy, target.host(), port)
                    .or_else(|| deny_explanation(&self.options, policy, &Host::from(dest.ip()), port)),
            }),
            Ruling::Allow(action) => Admission::Allowed {
                dest,
                fallbacks,
                action,
            },
            Ruling::Unlisted if decided => Admission::Allowed {
                dest,
                fallbacks,
                action: None,
            },
            Ruling::Unlisted => match self.consult(target, method, routed).await {
                Ok(()) => Admission::Allowed {
                    dest,
                    fallbacks,
                    action: None,
                },
                Err(reason) => denied_at(reason),
            },
        }
    }

    /// How long the workload's HTTP stack leaves a connection idle between requests.
    pub(super) fn idle_wait(&self) -> Duration {
        if self.options.reasoned_refusals.is_enabled() {
            self.limits.keep_alive_timeout + super::KEEP_ALIVE_GRACE
        } else {
            self.limits.keep_alive_timeout
        }
    }

    /// Puts an unlisted destination to the decider. An IP literal the SSRF
    /// floor refuses is denied here without asking, unless a MITM socket or
    /// parent proxy carries it, which no floor judges under the reasoned-refusal settings.
    async fn consult(&self, target: &ConnectTarget, method: DecideMethod, routed: bool) -> Result<(), Refused> {
        if let HostKind::Ip(ip) = target.host().kind()
            && self.decider.is_some()
            && !routed
        {
            let vetted = self.resolver.floor().vet(ip, &AllowRanges::default());
            if let Err(class) = vetted {
                return Err(EgressViolationReason::DestinationAddressDenied(class).into());
            }
        }
        if unlisted_destinations_refuse(self.options.unlisted) {
            return Err(Refused {
                reason: EgressViolationReason::PolicyDenied,
                explanation: standard_reason(StandardDenyReason::NotOnAllowList),
            });
        }
        let Some(decider) = &self.decider else {
            return Err(EgressViolationReason::PolicyDenied.into());
        };
        match decider.decide(target.host(), target.port(), Some(method)).await {
            Judgment::Allow => Ok(()),
            Judgment::Deny(reason) => Err(reason.into()),
        }
    }
}

fn unresolved(kind: io::ErrorKind) -> Admission {
    Admission::Unresolved(kind.into())
}
