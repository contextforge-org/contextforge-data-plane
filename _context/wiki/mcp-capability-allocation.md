# ContextForge 2.0 Target Architecture and Roadmap

> **Tentative target:** this page records the proposed ContextForge 2.0 end
> state and delivery phases. It is not a description of the current Rust
> implementation. See [Architecture](architecture.md) and
> [MCP Routing Semantics](routing.md) for current behavior.

This is a product-wide view because the ContextForge external dataplane
boundary depends on work owned by the ContextForge control plane and built-in
dataplane in the Python `IBM/mcp-context-forge` repository. It does not move
control-plane or built-in-dataplane responsibilities into this repository. See
[Project terminology](project.md#terminology) for the canonical component
names.

## Vision and Constraints

- The external dataplane targets MCP `2026-07-28` over Streamable HTTP, using
  `server/discover` and per-request client metadata. Older protocol versions,
  legacy initialization/session flows, and SSE remain on Python routes.
- External request handling must not depend on session affinity or a retained
  backend transport. Existing legacy code is migration state, not a target to expand.
- Fan-out belongs in control-plane reconciliation. The proposed dataplanes
  generate discovery, capability, and list responses from published effective
  configuration, rather than querying every backend during a client request.
- Effective configuration flows from the control plane to shared storage.
  Process-local caches accelerate reads but do not own policy.
- Subscription and notification delivery beyond current in-flight tool progress
  remains Phase 4 work.

## Protocol Scope and Implementation Checkpoint

Earlier planning considered a four-way modern/legacy compatibility matrix.
That is not this repository's current contract: new work targets modern MCP,
and legacy clients stay on control-plane/built-in routes. Historical targeted
routing work is linked from
[IBM/mcp-context-forge #6327](https://github.com/IBM/mcp-context-forge/issues/6327).
Do not treat that historical issue as authorization to expand Rust compatibility.

The current implementation already routes `tools/call`, `resources/read`, and
`prompts/get` through explicit per-user object maps, with request-scoped backend
connections and CPEX hooks. It rejects aggregate lists, completion, and
subscriptions. `server/discover` currently returns RMCP's local response, not
an effective catalog compiled for the principal. Tenant claims are required,
but Redis/cache keys contain only user ID; token scopes and compiled RBAC are
not enforced. The richer snapshots, authorization, and list responses below
are proposed work, not completed phases.

## Target End State

The front door separates management and MCP routes. It may select the built-in
or external dataplane for modern requests; older clients and stateful/legacy
flows stay on the Python side. The external route handles modern independent
requests. PostgreSQL remains the durable management store, while shared runtime
storage carries proposed compiled configuration to both dataplanes.

```text
client -> front door -> management -> control plane -> PostgreSQL
                    |                      |
                    |                      +-> compile shared effective config
                    |                      +-> reconcile modern/legacy backends
                    |
                    +-> Python MCP route -> built-in dataplane
                    |                         |-> read effective config
                    |                         +-> modern/legacy backend calls
                    |
                    +-> modern external route -> Rust external dataplane
                                                  |-> read effective config
                                                  +-> one modern backend call
```

Redis is the current external-dataplane configuration store and the preferred
shared implementation. The built-in dataplane may consume the same compiled
configuration from Redis or PostgreSQL. When multiple built-in-dataplane
instances are deployed, stateful MCP behavior requires an explicit shared-state
or affinity design; stateless behavior must not rely on process memory.

## Component Responsibilities

| Component | Target responsibility |
| --- | --- |
| Front door | Route management to the control plane and legacy/stateful MCP to Python routes. Select either dataplane for compatible modern traffic by deployment route. |
| ContextForge control plane | Manage the virtual-server lifecycle and upstream assignments; connect to heterogeneous upstreams; retrieve and page through capabilities, tools, resources, prompts, completions, and other catalogs; normalize and persist them; let administrators select exposed objects and rules; compile effective runtime configuration; poll upstream liveness and changes. |
| PostgreSQL | Persist administrative source data such as virtual servers, upstream definitions, normalized catalogs, selections, and policies. It is not on the external-dataplane request path. |
| Configuration synchronization | Publish effective configuration one way from the control plane to externally shared state. The built-in and external dataplanes should consume the same shape where practical. |
| ContextForge built-in dataplane | Handle `2026-07-28` and `2025-11-25` MCP requests in Python, including stateful and stateless behavior. It is the MCP request path shipped in the same repository as the control plane, not the control plane itself. |
| ContextForge external dataplane | Handle modern `2026-07-28` Streamable HTTP requests independently. In the proposed end state, read effective configuration, serve discovery and aggregates locally, and route targeted operations to one backend. It does not own IAM, UI, management APIs, or metrics storage. |
| Backend MCP servers | External routing targets modern backends with request-scoped connections. The control plane can reconcile heterogeneous catalogs, and Python routes own legacy protocol handling. |

## Administrative State and Effective Configuration

The control plane owns two distinct forms of state:

| State | Contents | Owner and consumers |
| --- | --- | --- |
| Administrative source state | Virtual servers, upstream registrations, raw and normalized catalogs, exposure selections, policies, and liveness. | Written by the control plane to PostgreSQL; used by management workflows and reconciliation. |
| Effective runtime configuration | Effective server identity and capabilities, visible tools/resources/prompts/completions, downstream paging material, backend resolution, required scopes/roles, and applicable runtime policy for a tenant or isolation domain, user, team, or other principal. | Compiled and published by the control plane; read by the built-in and external dataplanes. |

The control plane must exhaust upstream pagination while reconciling catalogs.
The compiled snapshot must contain enough information for either the built-in
or external dataplane to produce downstream paging without contacting every
upstream. Publication must be atomic or revisioned so neither the built-in nor
external dataplane combines partial catalog and policy state.

## Target Authorization Invariants

The effective-configuration model requires identity isolation as well as
catalog precomputation. A cached snapshot is data, not an authorization grant.
Every downstream request must independently establish and enforce its trusted
authorization context.

- The external dataplane derives the authorization key only from verified JWT
  claims and the validated server route. MCP params and client metadata must not
  supply or override a principal, team, tenant, virtual server, backend, or
  cache key.
- Snapshot and cache partitions include the applicable trust or tenant
  boundary, authenticated `sub`, effective team or other principal, virtual
  server, and configuration revision. Entries must never be reused across
  authorization contexts.
- The control plane maps verified identity attributes to an effective
  principal and compiles its visible objects and RBAC policy. The built-in and
  external dataplanes enforce required token scopes or roles and the compiled
  policy on every discovery, list, and targeted operation.
- Missing, unmapped, ambiguous, expired, or unauthorized snapshots and objects
  are denied by default. A targeted denial makes no upstream call, and errors
  must not disclose another principal's catalog or backend mapping.
- The exact tenant/team claim mapping and token-scope-to-RBAC rules are a
  cross-repository contract that the control plane, publisher, schemas,
  external dataplane, and integration tests must define together. The current
  user-ID-keyed routing maps, without tenant partitioning or scope/RBAC
  enforcement, are not the Phase 3 target.

## MCP Work Allocation

| Work | Target owner and behavior |
| --- | --- |
| Virtual-server creation and upstream assignment | Control plane persists management state and connects to assigned upstreams. |
| Upstream discovery, initialization where required, catalog pagination, capability aggregation, filtering, and liveness polling | Control plane only; this is the intentional fan-out boundary. |
| `server/discover` and effective capabilities | After per-request authorization, generate the modern response from principal-bound effective configuration. Legacy `initialize` stays on Python routes. |
| `tools/list`, `resources/list`, `prompts/list`, resource-template listing, and similar aggregate methods | After method-scope and compiled-RBAC enforcement, the built-in or external dataplane generates the visible response from principal-bound effective configuration with no live upstream fan-out. |
| `tools/call`, `resources/read`, `prompts/get`, completion, and similar targeted methods | The built-in or external dataplane resolves the effective entry under the trusted authorization key, applies default-deny scope and object policy, and calls exactly one selected backend only when authorized. The external dataplane handles modern requests without a reusable session; the built-in dataplane owns legacy/stateful execution. |
| Plugins for trusted aggregate responses | Prefer policy compiled by the control plane; avoid mandatory per-request plugin calls for a response already produced from trusted effective configuration. |
| Plugins for targeted calls | May run on the external-dataplane request path when request or response inspection is required. Exact hook allocation remains an implementation decision. |
| Subscriptions, server notifications, and downstream list-change notifications | Deferred to Phase 4 because their state and delivery model do not fit the request/response simplification. |

## Delivery Roadmap

| Phase | Scope |
| --- | --- |
| **1. Separate control-plane and built-in-dataplane responsibilities** | Establish a clear boundary between the ContextForge control plane and built-in dataplane inside the Python repository. The control plane writes effective configuration per user, team, or other principal to shared state; the built-in dataplane reads it and handles MCP requests. |
| **2. Route targeted calls through the external dataplane** | Use published object routes for modern calls. Tools, resources, and prompts already have request-scoped routing; completion remains unimplemented. Align the publisher and both consumers on the configuration contract. |
| **3. Serve modern request/response methods from effective configuration** | Add principal-bound discovery, capabilities, aggregate lists, and scope/RBAC enforcement for `2026-07-28`. Aggregate responses use compiled configuration; targeted calls reach one backend. This does not add legacy initialization or compatibility to Rust. |
| **4. Implement subscriptions and notifications** | Add the state, routing, and delivery model for upstream subscriptions, resource notifications, and list-change notifications after the request/response architecture is complete. |

## Phase 3 Reference Flows

The examples below use tools, but the same ownership applies to resources,
prompts, completions, and other aggregate or targeted request/response methods.
External clients and selected external backends in these flows use `2026-07-28`.
The control plane may also manage legacy servers for Python routes.

### 1. Create a Virtual Server and Select Capabilities

```text
user -> admin UI/API -> control plane -> persist virtual server/associations
                            |
                            +-> discover both modern backend catalogs
                            +-> exhaust pagination and reconcile catalogs
                            +-> display available entries (inc, sum, dec, diff)
user -> select inc and sum -> persist selected tools and policy
                            |
                            +-> compile by tenant, principal, and virtual host
                            +-> atomically publish snapshot revision N
shared store -> external dataplane -> load/cache revision N

The proposed snapshot contains visible objects, compiled scopes, and RBAC.
Only the control plane performs catalog fan-out and reconciliation.
```

### 2. Discover the Server and List Tools

```text
client -> server/discover with per-request metadata -> ingress -> dataplane
  dataplane: verify JWT, metadata, and route; derive trusted authorization key
  cache hit: use snapshot revision N under that key
  cache miss/expiry: read shared store and cache the authorized snapshot
  enforce discovery scope and compiled RBAC
  allow: return identity and visible capabilities from the snapshot
  deny/missing: return an error without catalog or backend details

client -> tools/list as an independent request -> ingress -> dataplane
  reverify identity and derive authorization key
  enforce tools/list scope and compiled RBAC
  allow: return visible tools (inc, sum) from the snapshot
  deny/missing: return an error without catalog details

Neither operation calls a live backend. Client-supplied identity or routing
metadata cannot override the trusted authorization context.
```

### 3. Call a Tool

```text
client -> tools/call inc with per-request metadata -> ingress -> dataplane
  verify JWT, metadata, and route; derive trusted authorization key
  resolve inc in the principal-bound snapshot
  enforce tools/call scope and compiled RBAC
    missing/denied: return an error; make no upstream call
    allowed:
      pre-call CPEX policy -> allow/modify arguments
      build request-scoped modern client -> call one selected backend
      backend result -> close connection -> post-call CPEX policy
      allow/modify result -> return MCP result

Client and backend use MCP 2026-07-28. No durable backend session is required.
After route resolution, results do not pass through the control plane or Redis.
A pre/post policy denial ends the corresponding path with an MCP error.
```

### 4. Reconcile an Upstream Catalog Change

```text
control-plane reconciler -> poll backend liveness, discovery, and catalogs
  -> reconcile administrative catalog changes
  -> compile affected snapshots
  -> atomically publish revision N+1 to shared storage
external dataplane -> load revision N+1 -> replace local cached snapshot
client -> tools/list -> receive updated visible catalog from the snapshot

Phase 4 owns downstream MCP list-change notifications.
```
