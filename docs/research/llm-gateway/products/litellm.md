# LiteLLM Proxy: a substitution-first model gateway with a database-owned tenancy model

Part of LLM Gateway Research.
Evidence snapshot retrieved 2026-10-13 from the LiteLLM documentation at
[docs.litellm.ai](https://docs.litellm.ai/docs/), read through the markdown
rendition the site publishes for each page (the index is
[llms.txt](https://docs.litellm.ai/llms.txt)). The pages declare
`last_updated: "2026-10-12"`; the release they describe is pinned only weakly,
see [Source anchors](#source-anchors). Text in quotation marks is verbatim from
the cited page; unquoted prose paraphrases it; anything that goes beyond what
the documentation says is labelled as our inference or listed under
[Open questions](#open-questions). Each area below records what the record
owns, where its configuration binds (`config.yaml`, the database, the Admin UI,
or the request itself), and the rationale the documentation states.

## Summary

LiteLLM Proxy puts one OpenAI-compatible endpoint in front of many provider
deployments. A request authenticates with a virtual key, passes rate limit and
budget checks, and is handed to the Router, which picks a deployment inside a
model group and owns "Load balancing, Fallbacks, Retries for LLM API
deployments"
([architecture](https://docs.litellm.ai/docs/proxy/architecture)). Tenancy,
spend, and most runtime settings live in Postgres; `config.yaml` is described
as "bootstrap" once the database stores settings
([configs](https://docs.litellm.ai/docs/proxy/configs)). The reliability model
is built on substitution: aliases, wildcards, ordered deployments, cooldowns,
and fallbacks across model groups and providers all exist to answer a request
with something other than the exact deployment first chosen.

Our inference: that substitution model is the point where LiteLLM and this
repository part ways. Draft
[ADR#0032](../../../adr/0032-model-route-and-credential-binding.md) forbids
failover, alias resolution, and credential rotation inside a managed-model
Session. What transfers is the spend and admission side: hierarchical spend
attribution, budget reservation before the provider call, explicit fail-closed
switches for Redis outages, and an unusually candid record of which identity
fields a caller can forge.

## Problem as framed by the documentation

The multi-tenant page frames the gateway as the answer to several groups
sharing provider access: "Cost has to be attributed to the right business unit
rather than pooled. Access has to differ per tenant, since teams need different
models, budgets, and rate limits. Administration has to be delegated, so a team
lead can manage their own team without platform-wide admin rights."
([multi-tenant architecture](https://docs.litellm.ai/docs/proxy/multi_tenant_architecture)).
Pass-through endpoints add a migration motive: existing SDK users "just need
to change the base url to get cost tracking/logging/budgets/etc."
([pass-through intro](https://docs.litellm.ai/docs/pass_through/intro)).

## Request path

The documented order for a proxied call
([architecture](https://docs.litellm.ai/docs/proxy/architecture)):

1. The bearer token is checked as a virtual key, from Redis or in-memory cache
   first and the database on a miss.
2. The parallel request limiter checks rpm and tpm for the global server, the
   virtual key, the user, and the team.
3. `proxy_server.py` hands chat and embedding calls to the Router.
4. The Router's `function_with_fallbacks` wraps `function_with_retries`, which
   wraps the SDK call that translates to the provider format.
5. After the response, logging callbacks, limiter counters, and the database
   spend writer run asynchronously.

The FAQ is explicit that persistence is off the request path: "No, a db
transaction is not tied to the lifecycle of a request." and "All other DB
transactions are async in background tasks"
([architecture](https://docs.litellm.ai/docs/proxy/architecture)).

## Model catalog: `model_list`, aliases, wildcards

### What the record owns

A deployment is one `model_list` entry: a user-facing `model_name`, the
`litellm_params` sent to the provider (provider-prefixed model, `api_base`,
`api_key`, sampling defaults), and router-only metadata under `model_info`.
"In the `config.yaml` the model_name parameter is the user-facing name to use
for your deployment."
([configs](https://docs.litellm.ai/docs/proxy/configs)). Entries sharing a
`model_name` form a model group, "a set of LLM API deployments that share the
same `model_name` and can be load balanced across."
([architecture](https://docs.litellm.ai/docs/proxy/architecture)). Identity is
derived rather than assigned: "LiteLLM generates a unique `model_id` for each
deployment by creating a deterministic hash of all the `litellm_params`."
([routing](https://docs.litellm.ai/docs/routing)).

Router-only settings must stay out of `litellm_params` because "unlike
`model_info`, `litellm_params` is copied into the actual request sent to the
LLM provider, so a router-only setting placed there would leak into that
request." ([routing](https://docs.litellm.ai/docs/routing)).

Named credentials can be shared across deployments. "Models and vector stores
reference a credential by `credential_name`, so it can't change after
creation." and "Credentials in `credential_list` can only be changed in
`config.yaml`." ([configs](https://docs.litellm.ai/docs/proxy/configs)).

### Aliases and wildcards

Several alias layers exist, owned by different records:

- `router_settings.model_group_alias` lets operators "Expose an 'alias' for a
  'model_name' on the proxy server." The stated uses include minor model
  version changes
  ([load balancing](https://docs.litellm.ai/docs/proxy/load_balancing)).
- Key-level aliases let an operator upgrade or downgrade the model a key's
  callers reach by changing "the alias mapping"
  ([virtual keys](https://docs.litellm.ai/docs/proxy/virtual_keys)).
- Team rows once persisted `model_aliases`; the docs warn this legacy rewrite
  "collapses to one deployment and can make newer sibling deployments
  unreachable." ([load balancing](https://docs.litellm.ai/docs/proxy/load_balancing)).
- Provider wildcards (`anthropic/*`) let operators "proxy all models from a
  specific provider without defining them on the config.yaml"
  ([wildcard routing](https://docs.litellm.ai/docs/wildcard_routing)).
- One OpenAI definition can fan out across organizations: "LiteLLM will
  automatically create separate deployments for each org."
  ([configs](https://docs.litellm.ai/docs/proxy/configs)).

### Where configuration binds

With `store_model_in_db` off, "the database is never read for config, so your
`config.yaml` is fully authoritative". With it on, the database overlays the
YAML for `general_settings`, `router_settings`, `litellm_settings`, and
`environment_variables`, and the database wins on any shared key: "Treat the
YAML as bootstrap for these four sections, and change UI-written keys in the UI
(or delete the row from `LiteLLM_Config`) rather than in the file"
([configs](https://docs.litellm.ai/docs/proxy/configs)).

Models merge differently. A database model "never replaces a same-named YAML
entry but becomes an additional deployment that gets load balanced with it"
([configs](https://docs.litellm.ai/docs/proxy/configs)). File models "are owned
by the file, so they cannot be edited or deleted from the UI; change them in the
config and reload." The page warns that running both "creates two sources of
truth" and asks operators to pick one
([model management](https://docs.litellm.ai/docs/proxy/model_management)).
Database-stored provider credentials are encrypted with `LITELLM_SALT_KEY`;
operators must "never change it once models exist, since credentials encrypted
with the old value cannot be decrypted with a new one."
([model management](https://docs.litellm.ai/docs/proxy/model_management)).

Pods converge by polling: "There is no cross-pod push; a pod converges within
one polling interval of a change." The interval defaults to 30 seconds
([production checklist](https://docs.litellm.ai/docs/proxy/prod)).

Stated rationale: day-2 model changes "all with no config edits and no proxy
restart" ([model management](https://docs.litellm.ai/docs/proxy/model_management)).

## Router: strategies, retries, cooldowns, fallbacks

### Routing strategies

Strategies are `simple-shuffle` (weighted pick by `weight` or by declared
tpm/rpm), `least-busy`, `usage-based-routing`, `latency-based-routing`, and
`cost-based-routing`. The docs say "We recommend using `simple-shuffle`
(default) for best performance in production." and warn that "Usage-based
routing adds significant latency due to Redis operations for tracking usage
across deployments." ([routing](https://docs.litellm.ai/docs/routing)).
Routing groups let one Router apply a different strategy per model group.
Session affinity can "Pin every request of a conversation to the deployment
that served its first request." ([routing](https://docs.litellm.ai/docs/routing)).

Deployment `order` adds priority tiers. "Each order level gets its own set of
retries before escalating to the next.", and exhausting every tier falls
through to model-level fallbacks
([load balancing](https://docs.litellm.ai/docs/proxy/load_balancing)).
`model_info.access_windows` can "reserve it for specific teams during a daily
local-time window." ([model access](https://docs.litellm.ai/docs/proxy/model_access)).

Per-deployment `rpm` and `tpm` are routing hints by default and become hard
limits only with `enforce_model_rate_limits`; tpm stays best effort because
"Token count is unknown until the LLM responds."
([load balancing](https://docs.litellm.ai/docs/proxy/load_balancing)).
`max_parallel_requests` on a deployment rejects excess calls immediately with
a 429: "There is no wait queue and nothing to configure for one"
([routing](https://docs.litellm.ai/docs/routing)).

### Retries

"For RateLimitError we implement exponential backoffs" and "For generic
errors, we retry immediately". `num_retries` resolves from the
`x-litellm-num-retries` header first, then the request body, then the
deployment, then `litellm_settings`, "So a caller can always raise or lower the
retry count for one request". For routed calls, "LiteLLM owns retries and pins
the provider client to `max_retries: 0`" to avoid compounding retries
([routing](https://docs.litellm.ai/docs/routing)).

### Cooldowns

"Cooldowns apply to individual deployments, not entire model groups."
([routing](https://docs.litellm.ai/docs/routing)). The documented triggers are
an immediate 429, more than half the calls failing in the current minute, and
401, 404, or 408 responses, each with a 5 second default. A cooled-down
deployment leaves the pool and other deployments in the group keep serving.
`allowed_fails` and `cooldown_time` can be set per deployment under
`model_info`. A request that targets a specific deployment id as a fallback
"skips any cooldown check for the fallback model."
([reliability](https://docs.litellm.ai/docs/proxy/reliability)).

Upstream inconsistency: the same routing page annotates `allowed_fails: 3`
with "cooldown model if it fails > 1 call in a minute" while its trigger table
says ">50% failures in current minute"
([routing](https://docs.litellm.ai/docs/routing)). We do not resolve which rule
the code applies.

### Fallbacks

"If a call fails after num_retries, LiteLLM falls back to another model group,
so a failing model or provider automatically fails over to a healthy backup."
([reliability](https://docs.litellm.ai/docs/proxy/reliability)). Fallback
lists are ordered and typed: `content_policy_fallbacks`,
`context_window_fallbacks`, and generic `fallbacks`, plus `default_fallbacks`
for misconfigured groups. Clients can also send `fallbacks` per request.

Fallbacks are not checked against the caller's model access by default: "By
default a fallback configured in `router_settings` runs for every request,
even when the calling key is not allowed to call the fallback model directly."
The opt-in `general_settings.enforce_fallback_model_access` applies key, team,
and project access checks to every fallback target and skips targets the
caller may not use
([reliability](https://docs.litellm.ai/docs/proxy/reliability)).

Two consequences are documented. Encrypted reasoning from the failed
deployment cannot be decrypted by another provider or key, so "The fallback
drops what the target cannot decrypt and keeps each Responses reasoning item's
readable summary". Spend logs record `attempted_fallbacks` and
`original_model_group`, and "Both keys are set by the proxy and overwrite any
client-supplied values of the same name."
([reliability](https://docs.litellm.ai/docs/proxy/reliability)).

Our inference: every one of these mechanisms changes which provider account,
endpoint, or model serves a request after the caller asked for a name. The
caller-visible contract is the model group name, not a deployment.

## Virtual keys

### What the record owns

A virtual key is the credential a caller presents and the row that carries
most per-caller policy: allowed `models`, `max_budget` and `budget_duration`,
`tpm_limit`, `rpm_limit`, `max_parallel_requests`, aliases, metadata such as
tags and priority, guardrail and cache settings, `allowed_routes`, and
optional MCP permissions
([virtual keys](https://docs.litellm.ai/docs/proxy/virtual_keys)). Keys need
Postgres and a master key, which is "your Proxy Admin key" and must start with
`sk-` ([virtual keys](https://docs.litellm.ai/docs/proxy/virtual_keys)).
Spend logs store the key's sha256 token hash as `user_api_key_hash`.

Ownership is a field, not the creator. "A key's owner is whoever its
`user_id` points at, and that is not always the person who created it."
Inheritance from that owner varies by surface: "Inheritance is not uniform
across permission surfaces. Model access and MCP access are always evaluated
against the key row itself, while management-route access is decided by the
*role* of the owning user, which is why a key owned by a proxy admin can call
admin endpoints." ([virtual keys](https://docs.litellm.ai/docs/proxy/virtual_keys)).
Service account keys belong to a team rather than a person and "apply budgets
and rate limits at the **team level**, not per user or per key member."
([service accounts](https://docs.litellm.ai/docs/proxy/service_accounts)).

### Policy hooks and rotation

`custom_key_generate` only runs on `/key/generate`, so "a user can create a
compliant key and then edit it out of compliance"; `custom_key_update` and the
newer `custom_key_policy`, which sees both the stored and the effective key
row, close that gap
([virtual keys](https://docs.litellm.ai/docs/proxy/virtual_keys)). Rotation
can keep the old secret alive: with `grace_period`, "Both old and new keys work
until the grace period elapses". Manual rotation is marked as an enterprise
feature. Scheduled rotation is a background job driven by `auto_rotate` and
`rotation_interval` on the key and requires a database
([virtual keys](https://docs.litellm.ai/docs/proxy/virtual_keys)).

### Caller-supplied identity

The key page is candid that the OpenAI `user` field is a caller claim:
"Because that field is set by the caller, a client can change it to dissociate
its activity from a given identity." The opt-in `overwrite_user_with_key_hash`
replaces the outgoing `user` with the key hash; "The flag is off by default"
([virtual keys](https://docs.litellm.ai/docs/proxy/virtual_keys)).

### Where configuration binds

Keys are database rows managed through `/key/*` and the Admin UI. Team-level
callback settings can be declared in `config.yaml`, but "There is no
`config.yaml` surface for declaring individual virtual keys"
([team logging](https://docs.litellm.ai/docs/proxy/team_logging)).

## Tenancy: organizations, teams, users, end users

### What the records own

"LiteLLM models tenancy as four nested levels: Organizations contain Teams,
Teams contain Users, and Users and Teams own Keys. Each level is a boundary for
isolation and for spend attribution."
([multi-tenant architecture](https://docs.litellm.ai/docs/proxy/multi_tenant_architecture)).
An organization carries its own budget and allowed-model list and is
administered by org admins; "Organizations are an enterprise feature." In open
source, "Teams alone can serve as your top-level tenant boundary"
([multi-tenant architecture](https://docs.litellm.ai/docs/proxy/multi_tenant_architecture)).
A team owns a budget, rate limits, admins, model access, and service account
keys, and when it sits in an organization it inherits that organization's
constraints.

Customers, also called end users, are a separate axis. They are not
authenticated principals; they are identifiers attached to requests made with
someone else's key. The proxy checks, first match wins, the
`x-litellm-customer-id` header, the `x-litellm-end-user-id` header, configured
custom headers, the body `user` field, `litellm_metadata.user`,
`metadata.user_id`, and `safety_identifier`. With JWT auth and
`end_user_id_jwt_field`, the verified claim wins, and "callers cannot override
it with `x-litellm-end-user-id`, `metadata.user_id`, etc."
([customers](https://docs.litellm.ai/docs/proxy/customers)).
`validate_end_user_id_in_db` drops identifiers with no matching customer row.

### Rationale and enforcement

"Spend flows up the hierarchy, so every request's cost is attributed to its
key, its user, its team, and its organization at once."
([multi-tenant architecture](https://docs.litellm.ai/docs/proxy/multi_tenant_architecture)).
The same page says budgets are "enforced inward" so that "a user budget cannot
exceed the team's, and a request is blocked once any level along its path is
over budget"
([multi-tenant architecture](https://docs.litellm.ai/docs/proxy/multi_tenant_architecture)).
The cross-page tension this creates is recorded under
[Upstream contradictions](#upstream-contradictions).

## Admin UI, management API, SSO, and roles

The Admin UI and the management REST API (`/key/*`, `/user/*`, `/team/*`,
`/organization/*`, `/model/*`, `/guardrails/*` and others) both write to the
same Postgres tables, and the UI "Requires proxy master key to be set"
([admin UI](https://docs.litellm.ai/docs/proxy/ui)). In microservice mode the
management API and UI backend are a separate `backend` service from the
`gateway` that serves model traffic
([deploy](https://docs.litellm.ai/docs/proxy/deploy)).

The bootstrap login is the environment pair `UI_USERNAME` and `UI_PASSWORD`,
falling back to the master key; "Anyone who signs in this way is a proxy
admin." The page names the risk: "This login path stores a permanent, shared,
cleartext admin credential in your environment, cannot be rotated per person,
and leaves no way to tell which admin did what."
`disable_env_credential_login` turns it off once database users exist
([admin UI](https://docs.litellm.ai/docs/proxy/ui)).

SSO: "From v1.76.0, SSO is free for up to 5 users. Beyond that, an enterprise
license is required." ([admin UI SSO](https://docs.litellm.ai/docs/proxy/admin_ui_sso)).
OIDC JWT authentication of admins, users, and projects into the proxy is an
enterprise feature ([JWT auth](https://docs.litellm.ai/docs/proxy/token_auth)).

Roles are global (`proxy_admin`, `proxy_admin_viewer`, `internal_user`) or
scoped (`org_admin`, `team_admin`), with the scoped roles marked as a premium
feature ([access control](https://docs.litellm.ai/docs/proxy/access_control)).
By default a team admin cannot change team settings; a proxy admin chooses
which fields team admins may edit through
`team_admin_editable_team_fields`.

## Budgets and spend

### What the records own

Budgets attach to the proxy as a whole (`litellm_settings.max_budget`),
organizations, teams, team members (`max_budget_in_team`), internal users,
keys, customers, tags, and per model on a key (`model_max_budget`)
([users and budgets](https://docs.litellm.ai/docs/proxy/users),
[tag budgets](https://docs.litellm.ai/docs/proxy/tag_budgets)). Each carries
`max_budget` and an optional `budget_duration`; "If not set, budget is never
reset." `budget_limits` stacks several windows on one entity, for example a
daily and a monthly cap, because "A single `budget_duration` window can't
prevent a bad day from burning your entire month."
([users and budgets](https://docs.litellm.ai/docs/proxy/users)). Cost per call
comes from a published model price map and is written to the spend log.

Precedence for team keys is stated in bold on the budgets page: "If a key
belongs to a team, only the team (and team-member) budgets are enforced; the
key owner's personal budget does not apply."
([users and budgets](https://docs.litellm.ai/docs/proxy/users)).

### Enforcement model

"Budget reservation is enabled by default. It helps enforce budgets during
concurrent traffic by accounting for a request before the provider processes
it." The proxy estimates a maximum cost from the request and pricing, reserves
it, rejects before the provider call if the reservation would exceed the
budget, and replaces the reservation with the priced cost afterwards. Routes
without token pricing and batch submissions cannot be reserved and fall back to
recorded spend ([users and budgets](https://docs.litellm.ai/docs/proxy/users)).

The hot-path state lives in Redis: "The counter is the source of truth on the
hot path, and the database is reconciled in the background." A Redis restart
from an older snapshot can under-report spend. `fail_closed_budget_enforcement`
makes every budgeted request validate against the database before admission,
covering "key, team, user, organization, end-user, tag, and per-window
budgets"; it is off by default
([users and budgets](https://docs.litellm.ai/docs/proxy/users)).

Budgets need the database. "Every budget on this page is enforced against
spend read from the database", so on a deployment without one no budget caps
anything, and the global `max_budget` "fails open there rather than erroring"
([users and budgets](https://docs.litellm.ai/docs/proxy/users)).

Tag budgets reject with HTTP 422 when exceeded. Tags "can be set on each
request (in `metadata` or via `x-litellm-tags`), or attached to a virtual key"
([tag budgets](https://docs.litellm.ai/docs/proxy/tag_budgets)), and "When
tags are provided via header or request body, they override any tags
configured in the model deployment."
([request tags](https://docs.litellm.ai/docs/proxy/request_tags)). The opt-in
`reject_clientside_metadata_tags` exists so tags "cannot be overridden by users
to potentially influence budget tracking or routing decisions"
([reject client-side tags](https://docs.litellm.ai/docs/proxy/reject_clientside_metadata_tags)).

Provider budgets are a routing feature rather than an admission feature:
`provider_budget_config` tracks spend per provider in Redis and the router
"Skips providers that have exceeded their budget"
([budget routing](https://docs.litellm.ai/docs/proxy/provider_budget_routing)).

## Rate limits, priority, and dynamic limits

Keys, users, teams, team members, end users, and tags carry `tpm_limit`,
`rpm_limit`, and `max_parallel_requests`, plus per-model rpm and tpm on a key
or team. TPM counts total tokens unless `token_rate_limit_type` selects input or
output only, and the setting is global
([users and budgets](https://docs.litellm.ai/docs/proxy/users)).

Counters live in Redis. Without Redis, a multi-instance proxy enforces limits
per process. `fail_closed_rate_limit_enforcement` rejects with a `503` rather
than "being admitted against a per-instance in-memory counter, which admits up
to N times the limit across N instances"; the default is `false`
([config settings](https://docs.litellm.ai/docs/proxy/config_settings)).

The dynamic rate limiter shares one model's capacity across keys and teams.
"while recorded usage is below a configurable saturation threshold, any key
may use idle capacity; once usage crosses the threshold, each priority level is
held to its reserved share."
([dynamic rate limits](https://docs.litellm.ai/docs/proxy/dynamic_rate_limit)).
`priority_reservation` maps priority names to shares, and the priority label
is read from team or key `metadata.priority`, with "Team-level priority takes
precedence over key-level priority." Reserving capacity by priority is an
enterprise feature. All keys without a label share one default pool, and "it
is not a per-key allocation"
([dynamic rate limits](https://docs.litellm.ai/docs/proxy/dynamic_rate_limit)).

Separately, the beta scheduler accepts a numeric `priority` on the request body
and queues requests when no deployment is healthy; "The lower the number, the
higher the priority" and the page says "Beta feature. Use for testing only."
([scheduler](https://docs.litellm.ai/docs/scheduler)). We found no statement on
whether the caller-supplied request `priority` is restricted per key; that is
a gap.

## Guardrails

### What the record owns

A guardrail is a named entry under `guardrails` in `config.yaml`, or a database
row created in the UI, that binds an integration (Presidio, Bedrock, Lakera, a
generic HTTP guardrail API, or custom Python) to one or more modes:
`pre_call` before the provider call, `during_call` in parallel with it,
`post_call` on input and output, and `logging_only`, which scans logged
content "without changing the client response"
([guardrails quick start](https://docs.litellm.ai/docs/proxy/guardrails/quick_start)).
`stream_scope` limits a guardrail to streaming or non-streaming requests.

Activation has several sources. `default_on: true` runs a guardrail "on every
request without the user having to specify it". Otherwise a client can name
guardrails in the request body under "Using Guardrails Client Side". Guardrail
policies group guardrails and "control which ones run for specific teams,
keys, or models", with inheritance between policies
([guardrails quick start](https://docs.litellm.ai/docs/proxy/guardrails/quick_start),
[guardrail policies](https://docs.litellm.ai/docs/proxy/guardrails/guardrail_policies)).

### The guardrail interface

"You only have to implement one method: `apply_guardrail`. LiteLLM pulls the
content out of a request (or a response), hands it to you as `inputs`, and
writes back whatever you return. Raise an exception to block the call."
([custom guardrail](https://docs.litellm.ai/docs/proxy/guardrails/custom_guardrail)).
Finer control uses the event hooks `async_pre_call_hook`,
`async_moderation_hook`, and `async_post_call_success_hook`. Two documented
limits matter for policy. `during_call` "reliably *blocks* a bad request, but
your edits may not land before the request leaves", so masking belongs in
`pre_call`. For streams, "This makes `post_call` guardrails on streaming
**audit-only**" for custom guardrails, while built-in guardrails that implement
`apply_guardrail` buffer the stream until moderation passes
([custom guardrail](https://docs.litellm.ai/docs/proxy/guardrails/custom_guardrail)).

### PII masking

The Presidio integration supports `MASK` and `BLOCK` per entity type and needs
operator-deployed Presidio analyzer and anonymizer containers
([PII masking](https://docs.litellm.ai/docs/proxy/guardrails/pii_masking_v2)).

### Delegated registration

Developers can `POST /guardrails/register` with a team-scoped key. "The
submission is stored with status `pending_review`." An admin approves or
rejects it in the UI, and only generic guardrail API guardrails can be
registered this way
([team-based guardrails](https://docs.litellm.ai/docs/proxy/guardrails/team_based_guardrails)).

## Caching

Caching is configured under `litellm_settings.cache` and `cache_params` in
`config.yaml`. "Exact-match caches (`redis`, `s3`, `gcs`, `local`, `disk`)
key on a hash of the whole request", while semantic caches embed the prompt
and serve the closest match above a similarity threshold
([caching](https://docs.litellm.ai/docs/proxy/caching)). An in-memory cache is
per worker, which is why the page calls Redis "the right default for anything
past a single worker" ([caching](https://docs.litellm.ai/docs/proxy/caching)).

Semantic caching carries an explicit tenancy warning. "A semantic cache key
deliberately leaves the prompt out, so the only thing keeping two callers apart
is the tenant scope: the virtual key, its team and its organization." End users
behind one key therefore share a bucket unless `semantic_cache_scope:
end_user` is set, and that end-user id is the caller-supplied identifier
described under tenancy. The documentation also advises against using it for
agents: "The recommendation is to keep semantic caching for single-shot traffic
and exclude agentic traffic."
([semantic caching](https://docs.litellm.ai/docs/proxy/caching_semantic)).

Per-request controls (`ttl`, `no-cache`, `no-store`) come from the client.
Putting a `cache` object in a key's metadata applies controls "to every
request made with that key, so clients need no changes."
([cache controls](https://docs.litellm.ai/docs/proxy/caching_controls)).

## Prompt management

Prompts bind in more than one place. In `config.yaml`, a `prompts` list binds a
`prompt_id` to a `prompt_integration`; the documented values are dotprompt
files, BitBucket, GitLab, a generic prompt management API, and Arize Phoenix,
and other values "fails proxy startup"
([prompt management](https://docs.litellm.ai/docs/proxy/prompt_management)).
The stated purpose is to "Run experiments or change the specific model" from
the prompt tool "instead of making changes in the application."
([prompt management](https://docs.litellm.ai/docs/proxy/prompt_management)).

The UI prompt studio stores prompts in the database and "automatically
versions your prompts each time you update them". Restoring an old version
creates a new one. "By default, API calls use the latest version of a prompt.
To use a specific version, pass the `prompt_version` parameter"
([LiteLLM prompt management](https://docs.litellm.ai/docs/proxy/litellm_prompt_management)).
A `CustomPromptManagement` class lets operators plug in their own source
([custom prompt management](https://docs.litellm.ai/docs/proxy/custom_prompt_management)).

Our inference: a prompt record can change the model that serves a request, so
prompt management is another path by which the served model is decided after
the caller chose a name.

## Logging, callbacks, and tracing

Callbacks are registered by name in `litellm_settings` (`callbacks`,
`success_callback`, `failure_callback`) and run after the response, off the
request path ([logging](https://docs.litellm.ai/docs/proxy/logging),
[architecture](https://docs.litellm.ai/docs/proxy/architecture)). Every
response carries `x-litellm-call-id`, which the docs call "the one that is
most useful for tracking a request across components in your system,
including in logging tools." ([logging](https://docs.litellm.ai/docs/proxy/logging)).

Redaction is opt-in. `turn_off_message_logging` stops message and response
content from reaching logging providers while spend metadata is still
tracked, and `redact_user_api_key_info` removes key, user, and team
identifiers from logs. A beta request header,
`x-litellm-enable-message-redaction`, turns redaction on per request
([logging](https://docs.litellm.ai/docs/proxy/logging)). The reverse also
exists: with an enterprise license, "you may want to disable specific
callbacks for a request" by sending `x-litellm-disable-callbacks`
([logging](https://docs.litellm.ai/docs/proxy/logging)). Callback destinations
can be set per team, and per-key callbacks are provisioned through the API,
both marked as enterprise
([team logging](https://docs.litellm.ai/docs/proxy/team_logging)).

For OpenTelemetry, "LiteLLM honors the W3C Trace Context header." The parent of
the proxy's server span is chosen in order: an explicit
`litellm_parent_otel_span` in request `metadata`, then an inbound
`traceparent`, then the active span, else a new root.
`OTEL_IGNORE_CONTEXT_PROPAGATION` forces every trace to be a root. An opt-in
OpenTelemetry v2 integration produces one trace per request covering auth,
guardrails, the provider call, and database writes, and follows the GenAI
semantic conventions
([OpenTelemetry](https://docs.litellm.ai/docs/observability/opentelemetry_integration)).
The Langfuse integration takes trace and session identifiers such as
`trace_id`, `session_id`, and `existing_trace_id` from request `metadata`
([Langfuse](https://docs.litellm.ai/docs/observability/langfuse_integration)).

Our inference: trace parentage, the Langfuse trace and session, and whether a
callback runs at all are all settable by the caller. The record of a request in
an observability backend is therefore partly a caller claim.

## Pass-through endpoints

Pass-through routes expose provider-native APIs under the proxy. "The request
is passed through to the provider's endpoint. The response is then passed back
to the client. **No translation is done.**" The caller presents a LiteLLM key,
which is "validated and mapped to the provider's API key", and the stated uses
are migrating existing SDK users and reaching provider-specific endpoints
([pass-through intro](https://docs.litellm.ai/docs/pass_through/intro)).

Custom pass-through routes are declared in `config.yaml` under
`pass_through_endpoints` or in the UI, with a target URL, static headers that
can reference provider secrets, optional `forward_headers`, and
`include_subpath`; the `auth` flag that turns on LiteLLM key authentication for
a custom route is marked "(Enterprise)"
([pass-through](https://docs.litellm.ai/docs/proxy/pass_through)). Guardrails
"are **disabled** on pass-through endpoints unless explicitly enabled", and
when enabled inherit from org, team, and key levels
([pass-through guardrails](https://docs.litellm.ai/docs/proxy/pass_through_guardrails)).
Cost tracking for pass-through traffic has its own page
([pass-through cost tracking](https://docs.litellm.ai/docs/proxy/pass_through_cost_tracking)).

## Self-hosting and data stores

The proxy ships as a monolithic image or as microservices: "a `gateway` (LLM
traffic, port 4000), `backend` (management APIs and UI backend, port 4001),
and `ui` (port 3000), each deployed and scaled independently."
([deploy](https://docs.litellm.ai/docs/proxy/deploy)). Postgres holds "Keys,
teams, users, spend logs, config", and Redis holds "Rate limiting, router
state, caching across instances" and is "Required once you run more than one
instance". Schema migrations run as a separate job, and proxy instances set
`DISABLE_SCHEMA_UPDATE=true`
([deploy](https://docs.litellm.ai/docs/proxy/deploy)). Without a database
"there is no key management, spend tracking, or UI persistence, so
authentication falls back to the master key alone."
([deploy](https://docs.litellm.ai/docs/proxy/deploy)).

The Redis requirements page lists what degrades without Redis: rate limits and
budgets are counted per worker, cooldowns stay local, caches split, and
database-stored config reaches other workers only on the next reload
([Redis requirements](https://docs.litellm.ai/docs/proxy/redis_requirements)).
Spend writes can go through a Redis transaction buffer rather than straight to
the database ([production checklist](https://docs.litellm.ai/docs/proxy/prod)).
During a database outage, keys already in the in-memory auth cache keep
authenticating until the cache TTL, 60 seconds by default, expires
([production checklist](https://docs.litellm.ai/docs/proxy/prod)).

Releases follow a weekly cycle. Since 1.84.0 "weekly scheduled releases bump
the **MINOR** component, and **PATCH** is reserved for hotfixes", and "In
production, we recommend pinning to the latest stable `1.x.x` release."
([release cycle](https://docs.litellm.ai/docs/proxy/release_cycle)).

## Relation to this repository's ADRs

Where LiteLLM's model and this repository's ADRs differ, the ADR is
authoritative. Draft ADRs are named as drafts; they record a proposed
direction, not an accepted one.

### Route and credential binding (ADR#0032, draft)

[ADR#0032](../../../adr/0032-model-route-and-credential-binding.md) resolves
every managed-model route before a Session becomes runnable and holds it
fixed. An allowed-model entry binds an exact model version to the provider
model identifier, and a mutable alias, `latest`, `default`, or `auto` cannot
establish that binding. Resolution never probes a list of credentials or falls
through to the first one that works. Section 6 projects the admitted route
without substitution or fallback, and the consequences state that provider and
model failover are unavailable unless a later ADR defines an explicit,
ledgered Session transition. Section 9 allows automatic retry of provider
throttling on the same route only, and of provider unavailability on the same
route only when safe.

Against that record, LiteLLM's central reliability features are the things
the ADR excludes:

- Model groups, `model_group_alias`, key and team aliases, and wildcards let
  one caller-visible name resolve to different deployments and credentials.
- Weighted shuffle, `order` tiers, cooldowns, and provider budget routing
  change which deployment serves a call based on live state.
- `fallbacks`, `context_window_fallbacks`, `content_policy_fallbacks`, and
  `default_fallbacks` move a failed call to another model group, and by default
  without checking the caller's access to the target.
- A caller can set `num_retries` and `fallbacks` on the request.

We infer that LiteLLM could only sit behind ADR#0032 as a route executor if it
were configured with one deployment per model group, no aliases or wildcards,
no fallbacks, and caller retry overrides disabled. Whether that configuration
is supported and verifiable is listed under [Open questions](#open-questions).

What does align: ADR#0032 places budget and rate enforcement at the model
access service, the one component that calls providers, and leaves product
policy for that enforcement outside the ADR. LiteLLM's budget reservation
before the provider call and its explicit fail-closed switches for Redis
outages are concrete prior art for that open follow-up. Our inference: the
fail-open defaults (DB-less budgets, per-instance rate counters, Redis counter
trusted on the hot path) are the defaults this repository would need to
invert.

### Who the caller is (ADR#0069 draft, ADR#0050, ADR#0026)

[ADR#0069](../../../adr/0069-event-envelope-records-issuer-and-times.md)
records the actor from the identity the platform authenticated and treats any
requester-written value as a claim, not a record. LiteLLM's documentation
admits many requester-written values into spend attribution, budget
selection, caching scope, and observability: end-user ids from headers and
body fields, the OpenAI `user` field, request tags, Langfuse trace and session
ids, OpenTelemetry parents from request `metadata`, request `priority`, and
`x-litellm-disable-callbacks`. Its mitigations (JWT end-user claims,
`overwrite_user_with_key_hash`, `reject_clientside_metadata_tags`) are
opt-in. The exception is fallback bookkeeping, where the proxy overwrites
client-supplied `attempted_fallbacks` and `original_model_group`. Our
inference: under ADR#0069 these fields could be carried as labelled claims,
never as the attribution record.

Virtual keys are bearer tokens. [ADR#0050](../../../adr/0050-signed-first-caller-authentication.md)
makes signed proof-of-possession the recommended caller authentication and
labels bearer keys as a compatibility tier.
[ADR#0026](../../../adr/0026-command-authorization-principal.md) makes the
authorization principal explicit on command execution. LiteLLM's rule that a
key's management rights follow the owning user's role, not the key, is the
kind of implicit delegation an explicit principal is meant to surface.

### Tenancy and quota ownership (ADR#0046)

[ADR#0046](../../../adr/0046-project-anchored-resource-hierarchy.md) fixes a
two-level hierarchy, organization over project, with the project owning
credentials, API keyspaces, and quotas. LiteLLM's hierarchy is organization,
team, user, key, with the team as the open-source tenant boundary and keys
owned by users, teams, or both. Our inference: a LiteLLM team is closest to a
project, and LiteLLM's per-user and per-key budgets have no direct counterpart
in the accepted hierarchy.

### Configuration authority (ADR#0007)

[ADR#0007](../../../adr/0007-configuration-sources.md) resolves configuration
as built-in defaults, then TOML, then environment variables, then CLI
arguments, with higher sources overriding lower ones. LiteLLM with
`store_model_in_db` makes database rows written from the UI override the file
for whole settings sections, and adds database models alongside file models.
The ADR has no database overlay layer; where they differ, the ADR governs.

### Secret custody (ADR#0023, ADR#0032)

[ADR#0023](../../../adr/0023-secret-management-and-key-custody-direction.md)
puts secrets in OpenBao behind a platform secrets service, and ADR#0032 keeps
that service the only OpenBao client. LiteLLM stores database-managed provider
credentials in Postgres, encrypted with a single `LITELLM_SALT_KEY` that can
never change once models exist, and resolves `os.environ/` references in
`config.yaml`. Our inference: the salt key is a non-rotatable root secret,
which the custody ADRs are written to avoid.

### Pass-through (ADR#0021, by analogy)

[ADR#0021](../../../adr/0021-typed-decode-over-passthrough-forwarding.md)
decides typed decode over lossless passthrough for the ACP-over-NATS bridge,
to keep validation at the boundary. It is about a different bridge. By
analogy only, LiteLLM's pass-through endpoints are the opposite trade:
provider-native payloads are forwarded unvalidated, with guardrails off unless
enabled.

### Admission and tracing (ADR#0028, ADR#0008, ADR#0042 draft)

[ADR#0028](../../../adr/0028-decider-admission-control-and-backpressure.md)
adds a bounded admission hook at command execution. LiteLLM's
`max_parallel_requests` per deployment, which rejects at once with no wait
queue, is the same shape at the provider edge.
[ADR#0008](../../../adr/0008-opentelemetry-observability.md) commits the
platform to OpenTelemetry, and
[ADR#0042](../../../adr/0042-nats-trace-context-and-message-path-tracing.md)
covers trace context on NATS. LiteLLM honours W3C `traceparent`, so its spans
can join a platform trace; the caller-settable parent in request `metadata`
is the part that ADR#0069 would treat as a claim.

### Usage settlement

The [usage settlement ledger](../../../architecture/usage-settlement-ledger.md)
keeps settlement in a billing consumer's own domain, outside the Session
stream, and writes intent before a charge. LiteLLM writes spend to the same
Postgres that holds keys and budgets, asynchronously, and reconciles a Redis
counter in the background. Our inference: LiteLLM's spend log is an admission
input, not a settlement record, and the ledger's separation would still be
needed downstream of anything like it.

## Upstream contradictions

Recorded as found, with both pages cited, and not resolved.

- Team keys and personal budgets. The multi-tenant page says "a user budget
  cannot exceed the team's, and a request is blocked once any level along its
  path is over budget"
  ([multi-tenant architecture](https://docs.litellm.ai/docs/proxy/multi_tenant_architecture)).
  The budgets page says "If a key belongs to a team, only the team (and
  team-member) budgets are enforced; the key owner's personal budget does not
  apply." ([users and budgets](https://docs.litellm.ai/docs/proxy/users)).
- Key ownership. The hierarchy page says "Keys can belong to either a team or
  a user." ([user management hierarchy](https://docs.litellm.ai/docs/proxy/user_management_heirarchy)).
  The multi-tenant page says keys "belong to a user, a team, or both."
  ([multi-tenant architecture](https://docs.litellm.ai/docs/proxy/multi_tenant_architecture)).
- Budget source of truth. The budgets page says "Every budget on this page is
  enforced against spend read from the database", and the same page later says
  "The counter is the source of truth on the hot path, and the database is
  reconciled in the background."
  ([users and budgets](https://docs.litellm.ai/docs/proxy/users)).
- Cooldown threshold. The routing page annotates `allowed_fails: 3` with
  "cooldown model if it fails > 1 call in a minute" and its trigger table says
  ">50% failures in current minute"
  ([routing](https://docs.litellm.ai/docs/routing)).
- Pass-through translation. The pass-through page says "**No translation is
  done.**" and its forwarding steps include "Request is reformatted for the
  target provider's API"
  ([pass-through intro](https://docs.litellm.ai/docs/pass_through/intro)).

## Open questions

- Single-route configuration. Whether a LiteLLM deployment with one
  deployment per model group, no aliases, wildcards, or fallbacks, and caller
  overrides of `num_retries` and `fallbacks` blocked is supported, and whether
  that effective configuration can be read back and verified, is not
  documented in the pages we read.
- Retry target. Whether a retry inside a model group may land on a different
  deployment of that group is not stated directly; the `order` tiers suggest
  it, but we found no explicit statement.
- Cooldown rule. Which of the documented thresholds the code applies (see
  [Upstream contradictions](#upstream-contradictions)).
- Team key budget precedence. Which of the conflicting statements reflects
  current behaviour (see [Upstream contradictions](#upstream-contradictions)).
- Request `priority`. Whether a key can be barred from setting the scheduler
  `priority` in the request body is not stated.
- Client-named guardrails. Whether a guardrail list in the request body can
  only add guardrails or can also suppress ones that `default_on` or a policy
  would run is not stated in the pages we read.
- Fail-closed coverage. Whether the fail-closed budget mode also covers
  provider budget routing, which tracks spend in Redis, is not stated.
- Version pin. The documentation is a rolling site; which release each
  statement describes is not declared per page (see
  [Source anchors](#source-anchors)).

## Source anchors

Retrieval and pinning:

- All pages were retrieved on 2026-10-13 from the markdown rendition that
  docs.litellm.ai serves for each documentation page; each declares
  `last_updated: "2026-10-12"`. The repository source for the same pages
  (`docs/my-website/docs/` in `BerriAI/litellm`) was not reachable as raw
  markdown at retrieval time, so the published site is the only evidence used.
- The version pin is weak. The site is a rolling build, pages do not declare
  the release they describe, and releases ship weekly; the highest release
  number mentioned in the pages we read is 1.107.0. Treat each statement as
  true of the site on the retrieval date, not of a specific release.
- Enterprise and premium gating is reported as the pages mark it; we did not
  verify licensing terms.

Quotation handling:

- Quoted text is verbatim. Line wrapping is ours. Markdown emphasis is kept as
  written, except the team-key budget sentence from the budgets page, which is
  wrapped in bold italics in the original and is quoted without that markup.
- No quoted passage contains an em dash or en dash; where a source sentence
  did, we paraphrased it or quoted a shorter clause.
- Where a source sentence contains a markdown link, we paraphrased around the
  link rather than quoting across it.
- Site-relative links in the source are cited here as absolute
  `https://docs.litellm.ai/docs/` URLs.

Pages cited:

- [docs/observability/langfuse_integration](https://docs.litellm.ai/docs/observability/langfuse_integration)
- [docs/observability/opentelemetry_integration](https://docs.litellm.ai/docs/observability/opentelemetry_integration)
- [docs/pass_through/intro](https://docs.litellm.ai/docs/pass_through/intro)
- [docs/proxy/access_control](https://docs.litellm.ai/docs/proxy/access_control)
- [docs/proxy/admin_ui_sso](https://docs.litellm.ai/docs/proxy/admin_ui_sso)
- [docs/proxy/architecture](https://docs.litellm.ai/docs/proxy/architecture)
- [docs/proxy/caching](https://docs.litellm.ai/docs/proxy/caching)
- [docs/proxy/caching_controls](https://docs.litellm.ai/docs/proxy/caching_controls)
- [docs/proxy/caching_semantic](https://docs.litellm.ai/docs/proxy/caching_semantic)
- [docs/proxy/config_settings](https://docs.litellm.ai/docs/proxy/config_settings)
- [docs/proxy/configs](https://docs.litellm.ai/docs/proxy/configs)
- [docs/proxy/custom_prompt_management](https://docs.litellm.ai/docs/proxy/custom_prompt_management)
- [docs/proxy/customers](https://docs.litellm.ai/docs/proxy/customers)
- [docs/proxy/deploy](https://docs.litellm.ai/docs/proxy/deploy)
- [docs/proxy/dynamic_rate_limit](https://docs.litellm.ai/docs/proxy/dynamic_rate_limit)
- [docs/proxy/guardrails/custom_guardrail](https://docs.litellm.ai/docs/proxy/guardrails/custom_guardrail)
- [docs/proxy/guardrails/guardrail_policies](https://docs.litellm.ai/docs/proxy/guardrails/guardrail_policies)
- [docs/proxy/guardrails/pii_masking_v2](https://docs.litellm.ai/docs/proxy/guardrails/pii_masking_v2)
- [docs/proxy/guardrails/quick_start](https://docs.litellm.ai/docs/proxy/guardrails/quick_start)
- [docs/proxy/guardrails/team_based_guardrails](https://docs.litellm.ai/docs/proxy/guardrails/team_based_guardrails)
- [docs/proxy/litellm_prompt_management](https://docs.litellm.ai/docs/proxy/litellm_prompt_management)
- [docs/proxy/load_balancing](https://docs.litellm.ai/docs/proxy/load_balancing)
- [docs/proxy/logging](https://docs.litellm.ai/docs/proxy/logging)
- [docs/proxy/model_access](https://docs.litellm.ai/docs/proxy/model_access)
- [docs/proxy/model_management](https://docs.litellm.ai/docs/proxy/model_management)
- [docs/proxy/multi_tenant_architecture](https://docs.litellm.ai/docs/proxy/multi_tenant_architecture)
- [docs/proxy/pass_through](https://docs.litellm.ai/docs/proxy/pass_through)
- [docs/proxy/pass_through_cost_tracking](https://docs.litellm.ai/docs/proxy/pass_through_cost_tracking)
- [docs/proxy/pass_through_guardrails](https://docs.litellm.ai/docs/proxy/pass_through_guardrails)
- [docs/proxy/prod](https://docs.litellm.ai/docs/proxy/prod)
- [docs/proxy/prompt_management](https://docs.litellm.ai/docs/proxy/prompt_management)
- [docs/proxy/provider_budget_routing](https://docs.litellm.ai/docs/proxy/provider_budget_routing)
- [docs/proxy/redis_requirements](https://docs.litellm.ai/docs/proxy/redis_requirements)
- [docs/proxy/reject_clientside_metadata_tags](https://docs.litellm.ai/docs/proxy/reject_clientside_metadata_tags)
- [docs/proxy/release_cycle](https://docs.litellm.ai/docs/proxy/release_cycle)
- [docs/proxy/reliability](https://docs.litellm.ai/docs/proxy/reliability)
- [docs/proxy/request_tags](https://docs.litellm.ai/docs/proxy/request_tags)
- [docs/proxy/service_accounts](https://docs.litellm.ai/docs/proxy/service_accounts)
- [docs/proxy/tag_budgets](https://docs.litellm.ai/docs/proxy/tag_budgets)
- [docs/proxy/team_logging](https://docs.litellm.ai/docs/proxy/team_logging)
- [docs/proxy/token_auth](https://docs.litellm.ai/docs/proxy/token_auth)
- [docs/proxy/ui](https://docs.litellm.ai/docs/proxy/ui)
- [docs/proxy/user_management_heirarchy](https://docs.litellm.ai/docs/proxy/user_management_heirarchy)
- [docs/proxy/users](https://docs.litellm.ai/docs/proxy/users)
- [docs/proxy/virtual_keys](https://docs.litellm.ai/docs/proxy/virtual_keys)
- [docs/routing](https://docs.litellm.ai/docs/routing)
- [docs/scheduler](https://docs.litellm.ai/docs/scheduler)
- [docs/wildcard_routing](https://docs.litellm.ai/docs/wildcard_routing)
- [llms.txt](https://docs.litellm.ai/llms.txt) (documentation index)
