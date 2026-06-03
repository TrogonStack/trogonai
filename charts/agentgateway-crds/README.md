# agentgateway-crds

CRD chart for the Trogon agentgateway.

**v0.1 status:** stub. No CRDs are bundled because the v0.1 deploy
surface does not require Gateway-API CRDs — `trogon-gateway-k8s` is
present as a library/controller but the Gateway-API integration is
deferred to v0.2 (see `docs/roadmap/agentgateway-v0.2.md` and ADR 0017).

This chart exists so the install path is stable:

```sh
helm install agentgateway-crds charts/agentgateway-crds
helm install agentgateway      charts/agentgateway
```

When CRDs land, they will be placed under `templates/` here so
operators can upgrade the workload chart without re-applying CRDs.
