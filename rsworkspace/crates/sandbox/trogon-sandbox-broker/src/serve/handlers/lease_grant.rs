use std::sync::Arc;

use trogon_sandbox_contract::{LeaseForkRequest, LeaseRequest, LeaseResponse, TenantId};

use crate::serve::route_context::RouteContext;

/// Remembers the tenant of a granted lease, so later evidence about the lease can be attributed.
async fn remember_grant(context: &RouteContext, tenant: TenantId, response: &LeaseResponse) {
    if let LeaseResponse::Granted(granted) = response {
        context
            .lease_tenants
            .write()
            .await
            .insert(granted.lease_id.clone(), tenant);
    }
}

pub(crate) async fn lease(context: Arc<RouteContext>, request: LeaseRequest) -> LeaseResponse {
    let tenant = request.tenant.clone();
    let response = context.broker.handle_lease_request(request).await;
    remember_grant(&context, tenant, &response).await;
    response
}

pub(crate) async fn fork(context: Arc<RouteContext>, request: LeaseForkRequest) -> LeaseResponse {
    let tenant = request.tenant.clone();
    let response = context.broker.handle_fork_request(request).await;
    remember_grant(&context, tenant, &response).await;
    response
}
