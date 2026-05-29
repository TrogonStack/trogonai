use std::collections::HashMap;
use std::sync::RwLock;

use super::audit::RegionAuditSink;
use super::errors::RegionRouteError;
use super::health::RegionHealth;
use super::region_id::RegionId;
use super::topology::RegionTopology;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteDecision {
    pub region: RegionId,
    pub reason: RouteReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteReason {
    SessionPin,
    HomeRegion,
    Failover { from: RegionId },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestContext {
    pub session_id: Option<String>,
}

#[derive(Debug)]
pub struct RegionRouter<A: RegionAuditSink> {
    topology: RegionTopology,
    health: RegionHealth,
    audit: A,
    session_pins: RwLock<HashMap<String, RegionId>>,
}

impl<A: RegionAuditSink> RegionRouter<A> {
    pub fn new(topology: RegionTopology, health: RegionHealth, audit: A) -> Self {
        Self {
            topology,
            health,
            audit,
            session_pins: RwLock::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn topology(&self) -> &RegionTopology {
        &self.topology
    }

    #[must_use]
    pub fn health(&self) -> &RegionHealth {
        &self.health
    }

    pub fn session_pin(&self, session_id: &str) -> Option<RegionId> {
        self.session_pins
            .read()
            .expect("pin lock")
            .get(session_id)
            .cloned()
    }

    pub fn clear_session_pin(&self, session_id: &str) -> bool {
        self.session_pins
            .write()
            .expect("pin lock")
            .remove(session_id)
            .is_some()
    }

    pub fn route(
        &self,
        session_pin: Option<&RegionId>,
        request_ctx: &RequestContext,
    ) -> Result<RouteDecision, RegionRouteError> {
        let session_key = request_ctx.session_id.as_deref();
        let pinned = session_pin
            .cloned()
            .or_else(|| session_key.and_then(|id| self.session_pin(id)));

        if let Some(region) = pinned {
            if self.health.is_healthy(&region) {
                return Ok(RouteDecision {
                    region,
                    reason: RouteReason::SessionPin,
                });
            }
        }

        let home = self.topology.home_region().clone();
        let mut attempted = Vec::new();
        let mut last_unhealthy: Option<RegionId> = None;

        for candidate in self.topology.route_candidates() {
            attempted.push(candidate.clone());
            if self.health.is_healthy(&candidate) {
                let reason = if candidate == home {
                    RouteReason::HomeRegion
                } else {
                    let from = last_unhealthy.clone().unwrap_or_else(|| home.clone());
                    if from != candidate {
                        self.audit.region_failed_over(
                            &from,
                            &candidate,
                            "home_or_pin_unreachable",
                        );
                    }
                    RouteReason::Failover { from }
                };
                if let Some(session_id) = session_key {
                    self.session_pins
                        .write()
                        .expect("pin lock")
                        .insert(session_id.to_string(), candidate.clone());
                }
                return Ok(RouteDecision { region: candidate, reason });
            }
            last_unhealthy = Some(candidate);
        }

        Err(RegionRouteError::all_regions_unreachable(attempted))
    }
}

impl<A: RegionAuditSink> RegionRouter<A> {
    pub fn mark_region_recovered(&self, region: &RegionId) {
        let was_unreachable = !self.health.is_healthy(region);
        self.health.force_healthy(region);
        if was_unreachable {
            self.audit.region_recovered(region);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::multi_region::audit::RecordingRegionAuditSink;
    use crate::multi_region::errors::RegionRouteErrorKind;
    use crate::multi_region::health::{RegionHealth, RegionHealthConfig};
    use crate::multi_region::topology::{RegionEndpoint, RegionTopology};

    fn two_region_topo() -> RegionTopology {
        let home = RegionId::new("us-east").unwrap();
        let west = RegionId::new("us-west").unwrap();
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            home.clone(),
            RegionEndpoint {
                nats_url: "nats://east".into(),
                creds_ref: None,
            },
        );
        endpoints.insert(
            west.clone(),
            RegionEndpoint {
                nats_url: "nats://west".into(),
                creds_ref: None,
            },
        );
        RegionTopology::new(home, vec![west], endpoints).unwrap()
    }

    fn router() -> RegionRouter<RecordingRegionAuditSink> {
        let topo = two_region_topo();
        let health = RegionHealth::new(&topo, RegionHealthConfig::default());
        RegionRouter::new(topo, health, RecordingRegionAuditSink::default())
    }

    #[test]
    fn happy_path_selects_home() {
        let router = router();
        let decision = router
            .route(None, &RequestContext::default())
            .expect("route");
        assert_eq!(decision.region.as_str(), "us-east");
        assert_eq!(decision.reason, RouteReason::HomeRegion);
    }

    #[test]
    fn failover_when_home_unreachable() {
        let router = router();
        let home = RegionId::new("us-east").unwrap();
        router.health.force_unreachable(&home);
        let decision = router
            .route(None, &RequestContext::default())
            .expect("route");
        assert_eq!(decision.region.as_str(), "us-west");
        assert!(matches!(decision.reason, RouteReason::Failover { .. }));
        let events = router.audit.failed_overs.lock().expect("lock");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0.as_str(), "us-east");
        assert_eq!(events[0].1.as_str(), "us-west");
    }

    #[test]
    fn all_unreachable_returns_error_with_attempt_list() {
        let router = router();
        router.health.force_unreachable(&RegionId::new("us-east").unwrap());
        router.health.force_unreachable(&RegionId::new("us-west").unwrap());
        let err = router
            .route(None, &RequestContext::default())
            .expect_err("route");
        assert_eq!(err.kind, RegionRouteErrorKind::AllRegionsUnreachable);
        assert_eq!(err.attempted.len(), 2);
    }

    #[test]
    fn session_pin_sticky_across_calls_while_healthy() {
        let router = router();
        let ctx = RequestContext {
            session_id: Some("sess-1".into()),
        };
        let first = router.route(None, &ctx).expect("first");
        for _ in 0..4 {
            let next = router.route(None, &ctx).expect("next");
            assert_eq!(next.region, first.region);
            assert_eq!(next.reason, RouteReason::SessionPin);
        }
    }

    #[test]
    fn clear_session_pin_allows_failover_when_home_down() {
        let router = router();
        let ctx = RequestContext {
            session_id: Some("sess-2".into()),
        };
        router.route(None, &ctx).expect("pin home");
        assert!(router.clear_session_pin("sess-2"));
        router.health.force_unreachable(&RegionId::new("us-east").unwrap());
        let after_clear = router.route(None, &ctx).expect("failover west");
        assert_eq!(after_clear.region.as_str(), "us-west");
    }
}
