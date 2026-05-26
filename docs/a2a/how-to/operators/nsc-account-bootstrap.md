# A2A NSC account bootstrap (operator runbook outline)

## Purpose

**Automated path:** [`scripts/a2a-nsc-bootstrap.sh`](../../../../scripts/a2a-nsc-bootstrap.sh) runs the steps below for one Account given `A2A_OPERATOR_NAME`, `A2A_ACCOUNT_NAME`, and `A2A_NSC_DIR`. ACL subjects are declared in [`scripts/acl-templates/`](../../../../scripts/acl-templates). Manual path follows for reference.

This document is an **operator-facing outline** for provisioning a **per-tenant NATS Account** under a NATS Operator, applying the default subject ACL templates from Phase 0, and bootstrapping the JetStream streams and KV bucket the A2A binding expects inside that Account.

Tenancy is **one NATS Account per tenant**. Subjects carry **no `{tenant}` segment** — the Account namespace is the tenant boundary. Cross-tenant traffic requires explicit operator-signed Account exports/imports; federation is opt-in.

Architectural context and landed decisions:

- [A2A plan](../../explanation/architecture.md) — subject topology, component layout, phased delivery.
- [A2A docs index](../../README.md) — navigation hub under `docs/`.
- [A2A per-Account JetStream assets](../../reference/jetstream-account-streams.md) — **`A2A_EVENTS`**, **`A2A_AGENT_CARDS`** KV, **`A2A_PUSH_DLQ`** reference targets.
- [Subject ACL quick reference](../../reference/subject-acl-quickref.md) — one-page Caller / Gateway / Registrar patterns (derived from this runbook).

**Engineering tracker** (Phase 0 checklist this runbook satisfies):

- [A2A TODO](../../explanation/architecture.md) — Phase 0: perimeter & catalog.

This is an **outline**, not a copy-paste script. Use your org’s naming, quotas, and key-management practices. For exact `nsc` flag syntax, follow the official references cited in each step.

---

## High-level Operator / Account / User hierarchy

NATS security in operator mode is a three-level JWT hierarchy ([NSC basics](https://docs.nats.io/using-nats/nats-tools/nsc/basics)):

1. **Operator (`[OPERATOR]`)** — owns the root of trust, runs (or delegates) `nats-server` clusters, and signs **Account JWTs**. Sets fleet-wide limits (connections, payload, JetStream tiers). Does **not** mint end-user credentials directly.
2. **Account (`[TENANT_ACCOUNT]`)** — one per tenant. Isolates subject space, JetStream assets, and KV buckets. Defines exports/imports for cross-Account traffic. Issues **User JWTs** for services and callers inside the tenant.
3. **Users** — credentials consumed by clients and services. Authorization is **per User**: publish/subscribe permissions over the Account’s subject space. A2A Phase 0 defines three primary service roles in the ACL table below:
   - **Caller User** — external or bridge-originated identity (typically minted dynamically by the auth callout; see [Not covered yet](#not-covered-yet)).
   - **Gateway User** — long-lived service identity for the `a2a-gateway` process inside the tenant Account.
   - **Registrar service User** — long-lived identity for the `a2a-nats-discovery` process (catalog KV writes plus `{prefix}.discover.*` request/reply).

Additional service Users (agent runtime, push consumers) are provisioned with tighter ACLs than the gateway; their JWT templates are derived from the same Account.

```
[OPERATOR]
    └── signs Account JWT
            └── [TENANT_ACCOUNT]  (tenant boundary)
                    ├── Caller User JWT     → a2a.gateway.>, _INBOX.{caller_id}.>
                    ├── Gateway User JWT    → a2a.agent.>, a2a.task.>, a2a.push.>
                    ├── Registrar User JWT  → a2a.catalog.register.>, a2a.discover.>
                    └── (other service Users — agents, …)
```

---

## Subject ACL templates (Phase 0)

Phase 0 requires **subject ACL inside each Account** ([A2A TODO](../../explanation/architecture.md)):

- Bound **caller User** to `a2a.gateway.>` and `_INBOX.{caller_id}.>`.
- Bound **gateway User** to `a2a.agent.>` + `a2a.task.>` + `a2a.push.>`.
- Bound **registrar service User** to `{prefix}.catalog.register.*` (write ingress) and `{prefix}.discover.*` (KV-backed discover replies); KV puts on **`A2A_AGENT_CARDS`** are registrar-only ([A2A TODO](../../explanation/architecture.md)).

Replace `{caller_id}` with the stable caller identifier encoded in the minted User JWT (auth callout maps external identity → caller id). The gateway User is a fixed service principal per tenant Account.

| NATS JWT role | Publish (allow) | Subscribe (allow) | Notes |
|---------------|-----------------|-------------------|-------|
| **Caller User** | `a2a.gateway.>` | `_INBOX.{caller_id}.>` | Request/reply to the gateway namespace; replies arrive on the caller-owned inbox prefix. Use [`--allow-pub-response`](https://nats-io.github.io/nsc/nsc_add_user.html) if the client library relies on dynamic reply subjects beyond the explicit inbox ACL. **Do not** grant publish on `{prefix}.catalog.register.*` — catalog KV writes are registrar-only in production ([A2A TODO](../../explanation/architecture.md)); callers read AgentCards via `{prefix}.discover.{agent_id}` or KV get/watch. |
| **Gateway User** | `a2a.agent.>`, `a2a.task.>`, `a2a.push.>` | `a2a.gateway.>` | Queue-group consumer on ingress; forwards to agent subjects, publishes task events and push envelopes. Deny all other publish paths not required by your gateway build. Read-only catalog access (KV get/watch or `{prefix}.discover.*`); no KV put. |
| **Registrar service User** | `{prefix}.catalog.register.*` | `{prefix}.discover.*` | Long-lived `a2a-nats-discovery` identity. **`CatalogRegistrarService`** subscribes `{prefix}.catalog.register.*` (grant **`--allow-sub`** on that pattern in `nsc`; publish column reflects the registrar-owned write ingress — deny `{prefix}.catalog.register.*` publish on caller/gateway Users). **`DiscoverService`** subscribes `{prefix}.discover.*` and replies from KV ([A2A plan](../../explanation/architecture.md)). Grant [`--allow-pub-response`](https://nats-io.github.io/nsc/nsc_add_user.html) for request/reply on both paths. **JetStream KV:** create/update keys in bucket **`A2A_AGENT_CARDS`** (registrar-only writes in production; callers use the discover read path — [A2A TODO](../../explanation/architecture.md)). Optional JetStream **read** on **`A2A_EVENTS`** / **`A2A_PUSH_DLQ`** for ops. AgentCard write subject: [KV watch](../catalog/kv-watch.md). |

**Default prefix.** Examples use the in-tree default prefix `a2a`. If the tenant uses a custom `A2A_PREFIX`, substitute consistently (e.g. `myapp.gateway.>`). Stream names derive from the uppercased prefix (`MYAPP_EVENTS`); with the default prefix the names below apply verbatim.

**Push read path (related).** For `subject:` / `jetstream:` push targets, consumer-side ACL on `a2a.push.{caller_id}.>` gates read to the caller’s own User ([A2A pending decisions](../../explanation/architecture.md)). Provision that ACL when minting caller Users even though Phase 0 lists only the gateway-side `a2a.push.>` binding.

---

## JetStream and KV assets to bootstrap

Mirror **in-tree names** (default prefix `a2a`). Each tenant Account gets its **own** copies; names are identical across Accounts because Account membership disambiguates.

| Asset | Kind | Name | Subject / key space | Operator notes |
|-------|------|------|---------------------|----------------|
| Task events | JetStream stream | `A2A_EVENTS` | `a2a.task.*.events.*` | Shared per-Account stream for all task event traffic ([pending decision §1](../../explanation/architecture.md)). In-tree provisioner: `rsworkspace/crates/a2a-nats/src/jetstream/provision.rs`, config in `nats/subjects/stream.rs`. Default `max_age` = 24h ([pending decision §2](../../explanation/architecture.md)). Target retention policy: `interest` + `discard=old` ([pending decision §5](../../explanation/architecture.md)); in-tree code currently uses `limits` retention — align server-side config with the landed decision when provisioning. |
| AgentCard catalog | JetStream KV bucket | `A2A_AGENT_CARDS` | Keys: `{agent_id}` | One bucket per Account ([A2A TODO](../../explanation/architecture.md)). In-tree config: `history = 1`, `max_value_size = 65536` (`rsworkspace/crates/a2a-nats/src/catalog/nats_kv.rs`). Only the registrar service User should hold **write** ACL; gateway and callers read via discover/KV watch. |
| Push dead-letter | JetStream stream | `A2A_PUSH_DLQ` | `a2a.push.dlq.{caller_id}.{task_id}` | Per-Account DLQ for terminal push failures ([A2A TODO](../../explanation/architecture.md), [pending decision](../../explanation/architecture.md)). `a2a-nats` `provision_streams` creates this alongside `A2A_EVENTS` (subject filter `{prefix}.push.dlq.*.*`); **`a2a-nats`** agent `Bridge` publishes JSON failures from `message/stream` (see **[push DLQ ops](push-dlq-triage.md)**); **`a2a-gateway`** does not emit DLQ. |

**Provisioning mechanism.** `nsc` configures JWT limits and User permissions; it does **not** create JetStream streams or KV buckets. After the Account JWT is pushed to the cluster, create streams/KV with:

- Application bootstrap (`a2a-nats` `provision_streams`, `a2a-nats-discovery` for KV), **or**
- `nats` CLI / JetStream management API connected with an Account admin User.

Ensure the Account JWT enables JetStream before creating assets ([Configuring JetStream — account resource limits](https://docs.nats.io/running-a-nats-service/configuration/resource_management)).

---

## Step-by-step `nsc` outline

Use placeholders **`[OPERATOR]`** and **`[TENANT_ACCOUNT]`**. Adjust directory flags (`--config-dir`, `--data-dir`, `--keystore-dir`) to your NSC layout. Commands below assume an operator context is already selected (`nsc env -o [OPERATOR]`).

1. **Confirm operator context.** List operators and select yours ([NSC basics — environment](https://docs.nats.io/using-nats/nats-tools/nsc/basics)). Ensure the resolver and `nsc push` target are configured for your fleet.

2. **Add the tenant Account** (if it does not exist):

   ```bash
   nsc add account -n [TENANT_ACCOUNT]
   ```

   Reference: [`nsc add account`](https://docs.nats.io/using-nats/nats-tools/nsc/basics).

3. **Enable JetStream limits on the Account.** Set disk/memory/stream/consumer quotas appropriate to the tenant tier. Example pattern from NATS docs:

   ```bash
   nsc edit account -n [TENANT_ACCOUNT] \
     --js-mem-storage <limit> \
     --js-disk-storage <limit> \
     --js-streams <limit> \
     --js-consumer <limit>
   ```

   References: [Configuring JetStream — operator mode limits](https://docs.nats.io/running-a-nats-service/configuration/resource_management), [`nsc edit account` flags](https://nats-io.github.io/nsc/nsc_edit_account.html).

4. **Create the gateway service User** with Phase 0 ACLs:

   ```bash
   nsc add user -a [TENANT_ACCOUNT] -n a2a-gateway \
     --allow-sub "a2a.gateway.>" \
     --allow-pub "a2a.agent.>" \
     --allow-pub "a2a.task.>" \
     --allow-pub "a2a.push.>"
   ```

   Tighten with explicit `--deny-pub ">"` / `--deny-sub ">"` if your `nsc` version supports deny-default patterns for service Users. Reference: [`nsc add user`](https://nats-io.github.io/nsc/nsc_add_user.html).

5. **Define the caller User template** (for static bootstrap or documentation; production callers are minted by the auth callout):

   ```bash
   nsc add user -a [TENANT_ACCOUNT] -n a2a-caller-template \
     --allow-pub "a2a.gateway.>" \
     --allow-sub "_INBOX.{caller_id}.>" \
     --allow-pub-response
   ```

   Replace `{caller_id}` with the literal prefix for each caller, or automate User creation in the auth callout service. Reference: [`nsc add user`](https://nats-io.github.io/nsc/nsc_add_user.html).

6. **Create the registrar service User** (`a2a-nats-discovery`):

   ```bash
   nsc add user -a [TENANT_ACCOUNT] -n a2a-registrar \
     --allow-sub "a2a.catalog.register.>" \
     --allow-sub "a2a.discover.>" \
     --allow-pub-response
   ```

   Also grant JetStream KV **put/update** on bucket **`A2A_AGENT_CARDS`** to this User only (Account admin or platform IAM). Deny KV write to gateway and caller Users. Reference: [`nsc add user`](https://nats-io.github.io/nsc/nsc_add_user.html).

7. **Create additional service Users** (agents) with least-privilege ACLs — e.g. agent subscribe on `a2a.agent.{agent_id}.>` and publish on assigned task event subjects.

8. **Push Account and User JWTs to the operator**:

   ```bash
   nsc push -a [TENANT_ACCOUNT]
   ```

   Reference: [NSC basics — publishing JWTs](https://docs.nats.io/using-nats/nats-tools/nsc/basics).

9. **Bootstrap JetStream stream `A2A_EVENTS`** inside `[TENANT_ACCOUNT]` (subject filter `a2a.task.*.events.*`, file storage, `max_age` 24h unless overridden per tenant).

10. **Bootstrap JetStream KV bucket `A2A_AGENT_CARDS`** (`history = 1`, `max_value_size = 65536`).

11. **Bootstrap JetStream stream `A2A_PUSH_DLQ`** (subject filter `{prefix}.push.dlq.*.*`, e.g. `a2a.push.dlq.*.*`, or `{prefix}.push.dlq.>` if your tooling prefers a trailing wildcard).

12. **Verify connectivity** — connect as gateway, registrar, and caller Users; confirm publish/subscribe succeeds only within the ACL rows above and fails across Account boundaries (caller cannot publish `{prefix}.catalog.register.*`; registrar can write KV; caller can `{prefix}.discover.{agent_id}` request/reply).

13. **Record credentials** — store `.creds` or seed material in your org secret manager; rotate on User compromise.

---

## Not covered yet

The following Phase 0+ items are **intentionally out of scope** for this bootstrap outline. Track implementation in [A2A TODO](../../explanation/architecture.md):

- **Auth callout service** — NATS subscriber on `$SYS.REQ.USER.AUTH`; OIDC primary, mTLS for service-to-service, API keys transitional. Mints Account-bound User JWT (`sub` external, `aud` = Account, SpiceDB principal in `data`). Subject ACL inside the Account bounds the User to `a2a.gateway.>` and `_INBOX.{caller_id}.>`.
- **SpiceDB integration** — gateway client to org-standard cluster; `BulkCheckPermission` for catalog shaping; per-method resource tuples; owner tuples on task lifecycle; ZedToken cache per session (Phase 1).
- **AgentCard JSON-Schema validation** on registrar write and gateway read (`a2a-pack` canonical schema).
- **Tier 1 declarative policies**, gateway audit decision sites, and full audit envelope parity.
- **Cross-Account federation** — operator-signed exports of `a2a.discover.>` (opt-in).
- **Automated runbook tooling** — this document is the manual operator outline; CI/CD integration is future work.

When the auth callout lands, static caller User creation (step 5) becomes a reference template only; runtime minting replaces per-caller `nsc add user` for human and OIDC identities.
