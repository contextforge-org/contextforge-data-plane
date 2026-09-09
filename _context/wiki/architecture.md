# Architecture

This page describes the current Rust implementation. Proposed catalog and
policy compilation is in the [ContextForge 2.0 roadmap](mcp-capability-allocation.md).

## Middleware Stack Order

Tower layers execute outside-in:

```text
TCP/TLS listener
  -> HttpMetricsLayer
  -> TraceLayer (extract incoming trace context)
  -> /contextforge-rs nested router
  -> mcp_origin_layer           validates Origin (403)
  -> CORS layer
  -> mcp_header_limits_layer    bounds MCP headers (431)
  -> virtual_host_id_layer      inserts VirtualHostId from path (400)
  -> claims_layer               verifies JWT, inserts AuthorizationClaims (401)
  -> PrincipalExtractorLayer   inserts AuthorizedPrincipal (401)
  -> user_config_store_layer    loads UserConfig (400 missing, 500 decode/error)
  -> virtual_host_config_layer checks caller's virtual host (404)
  -> /servers/{virtual_host_name}/mcp RMCP service
       Host validation -> HTTP/MCP validation -> method dispatch
```

Origin is checked before authentication. The optional Host allowlist is checked
at the RMCP boundary, so earlier middleware may return first. MCP header budgets
apply before JWT verification, configuration reads, and body parsing. See
[Security](security.md#mcp-origin-and-host-validation).

Health is registered in every build outside the MCP auth/config layers, while
token, JWKS, and config helpers are compiled only with testing-only `with_tools`.
Both remain covered by the outer HTTP tracing and metrics layers. MCP handlers
consume typed extensions; they do not parse Redis keys.
`tools/call` also reads the HTTP headers from the request-context `Parts`.

## Pipeline Shape

```text
modern MCP request
  -> header, JWT, and principal checks
  -> user config and virtual-host check
  -> published object/backend route
  -> recognized tool parameter-header validation
  -> pre-hook
  -> connect to one backend -> call -> close
  -> post-hook on the successful response
  -> MCP response
```

Tools, resources, and prompts use explicit routing tables. There is no catalog
fan-out or prefix splitting. Resource pre-hooks may rewrite a URI only to an
unambiguous target published in the caller's virtual host.

For `tools/call`, a published input schema enables local `Mcp-Param-*`
validation before plugins and backend I/O. The gateway does not fetch
`tools/list`. Without a schema, parameter headers are forwarded without local
validation. A plugin that changes an annotated argument does not change the
original parameter header; the backend may reject a resulting mismatch.

Backend cleanup occurs before post-hooks. A cleanup failure is logged and does
not replace the operation's result. Error and cancellation paths do not turn
into successful response hooks. See [Routing](routing.md) and
[Failure Modes](failure-modes.md) for method-specific details.

## Module Boundaries

| Module / crate | Owns |
| --- | --- |
| Binary `main.rs`, `logging.rs` | Startup wiring and telemetry providers. |
| Library `common.rs` | CLI configuration, Redis/TLS validation, upstream HTTP client construction. |
| Library `authorization/` | JWKS verification and principal extraction. |
| Library `layers/` | Request metadata, authentication/configuration boundaries, and validation. |
| Library `gateway/` | MCP method handlers, explicit routing, per-request backend clients, progress forwarding. |
| Library `user_config_store/` | `UserConfigStore` and Redis/cache implementation. |
| Library `transports/` | TCP and TLS listeners. |
| Library `tools.rs` | Development bootstrap routes, gated by `with_tools`. |
| `contextforge-data-plane-apis` | Published configuration models and schemas. |
| `contextforge-data-plane-cpex` | Registry, runtime reloads, request hook state, and CMF adapters. |

## State Ownership

| State | Owner | Lifetime |
| --- | --- | --- |
| Parsed config and shared upstream HTTP client | Gateway | Process. |
| JWKS keys | JWT authorization service | Five-minute cache; fetched when verification needs them. |
| User config | Redis store and optional local LRU | Redis is authoritative; local capacity 50,000, default expiry 60 seconds. |
| Principal, claims, virtual-host ID, config snapshot | HTTP request extensions | One request. |
| Backend RMCP service | Routed operation | One request; explicitly closed after the call. |
| Tool progress-token mapping | Request's backend client | While the tool call is in flight. |
| Active CPEX runtime | Registry | Reloadable; in-flight hook state pins its selected runtime. |
| RMCP session manager | RMCP service (`LocalSessionManager`) | Transport implementation detail; no session is required by the supported modern request contract. |

The library no longer has `BackendTransports`, `SessionId` middleware, or a
`LocalUserSessionStore`. Modern routing never reuses backend session state from
a prior request and does not require load-balancer affinity.

Cache hits do not extend a user-config entry's expiry. A miss releases the LRU
lock before reading Redis. `--user-config-cache-expiry-seconds 0` disables this
cache. Development config writes update the local process's cache immediately;
other replicas see the write after their own expiry.

## Executor and Locks

The binary starts one Tokio runtime through `#[tokio::main]`. The parsed
`--number-of-cpus` and `--single-runtime` fields are currently not used to
construct it. Do not use those flags to tune workers or select per-CPU runtimes.

Locks have specific scopes:

- The user-config LRU mutex protects cache access, not Redis I/O.
- JWKS refresh and plugin-config connection management synchronize their own
  shared state; do not assume all network I/O is globally lock-free.
- Tool progress tracking holds a write guard while enqueuing the backend call
  so an early notification cannot race registration.
- Tool hook state uses a mutex to serialize progress and final-response plugin
  context updates. Prompt/resource state belongs to one request.

There is no shared map of live backend transports to lock during routing.

## Listener Behavior

The TCP listener uses socket reuse options, keepalive, and backlog `1024`, then
serves Axum with Ctrl-C graceful shutdown. The TLS listener accepts through
Rustls and serves the same router through Hyper. See the transport implementation
before relying on identical shutdown behavior between the two listener types.

The binary uses `tikv_jemallocator` as its global allocator. Performance claims
about it require a measured workload; see [Performance](performance.md).

## Cancellation, Progress, and Plugins

`tools/call` observes the downstream cancellation token and forwards cancellation
to the in-flight backend request. Progress notifications translate the generated
backend token to the caller's token; unknown tokens are dropped. Configured tool
post-hooks can inspect stream events, and a denied notification is dropped.
Prompt/resource operations should not be assumed to have the same explicit
cancellation relay.

Pre-hooks select a runtime before backend I/O. Typed hook state retains both
that runtime and whether a post-hook was enabled. Reloads affect subsequent
requests; they cannot add a hook or change policy halfway through a call.
Invalid reloads mark the registry failed for new calls while already pinned
requests can finish. Details are in [Plugin Config](config.md#plugin-config-redis-key-contextforgegatewayruntimepluginconfig).

## Startup

```text
Tokio main
  -> install Rustls crypto provider
  -> Config::parse()
  -> initialize logging and optional telemetry providers
  -> optional CPEX registry and compiled factory registration
  -> construct JWKS authorization service
  -> build Gateway with Redis config store and RMCP session manager
  -> initialize CPEX runtime from Redis, if enabled
  -> run_gateway(): build router and start configured listeners
```

Some checks are lazy: JWKS retrieval occurs during token verification, and
backend connectivity is checked when an operation selects that backend. Health
is HTTP liveness, not dependency readiness.

## Architecture-Change Follow-Through

| Change | Required follow-through |
| --- | --- |
| MCP behavior | Update modern protocol tests, examples, and front-door coordination. Do not expand legacy compatibility. |
| Published names, routes, or config shapes | Update publisher/consumer contracts, schemas, and integration tests. |
| Config transport | Keep user routing behind `UserConfigStore`; update adapter tests. |
| Plugin hook surface | Define ordering, failure, timeout, cancellation, streaming, and telemetry behavior; update validation and factory registration together. |
| Request lifecycle | Verify cancellation, backend cleanup, progress correlation, and replica independence. |
