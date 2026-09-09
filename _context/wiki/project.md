# Project Overview

This page describes the current external-dataplane implementation and integration
boundary. Proposed work is in the [ContextForge 2.0 roadmap](mcp-capability-allocation.md).

## What this project is

`contextforge-data-plane` is the Rust **ContextForge external dataplane**. It
routes MCP requests to backend servers using configuration published by the
ContextForge control plane. It has no IAM, UI, management database, or metrics
storage responsibilities.

## Terminology

| Component | Responsibility |
| --- | --- |
| **ContextForge control plane** | Management, identity, upstream registration, catalog and policy publication in [IBM/mcp-context-forge](https://github.com/IBM/mcp-context-forge). |
| **ContextForge built-in dataplane** | MCP handling shipped in the Python repository. Older clients, legacy initialization/session flows, and SSE stay on those routes. It can also serve modern clients. |
| **ContextForge external dataplane** | This independently deployed Rust service. Its supported downstream contract is MCP `2026-07-28` over Streamable HTTP with `server/discover` and per-request client metadata. |

Use these component names in product-wide documentation. “External” means
separately deployed, not untrusted. A stateless request independently establishes
its identity, configuration, and backend route; it does not need a session or
backend transport retained from a previous request.

## Goals and objectives

- Provide a low-latency routing layer between modern MCP clients and backends.
- Keep control-plane responsibilities outside this repository.
- Keep persistent user configuration behind `UserConfigStore`.
- Prefer correct architecture over preserving unstable APIs during early development.

## Key stakeholders and users

Platform teams deploy the service, application developers consume its MCP
routes, and contributors evolve its routing, security, and protocol behavior.

## Key modules and architecture

| Page | Covers |
| --- | --- |
| [Architecture](architecture.md) | Middleware, startup, state ownership, and module boundaries. |
| [Routing](routing.md) | Published routes and per-request backend lifecycle. |
| [Configuration](config.md) | CLI, principal mapping, Redis documents, plugins, and telemetry. |
| [Security](security.md) | Trust boundaries and current authorization limits. |
| [Roadmap](mcp-capability-allocation.md) | Proposed effective catalogs and stronger authorization. |

## Crate ownership

| Crate | Purpose |
| --- | --- |
| `contextforge-data-plane-lib` | Gateway behavior, middleware, routing, configuration access, and transports. |
| `contextforge-data-plane` | Process startup, CLI parsing, logging, and wiring the library. |
| `contextforge-data-plane-apis` | Shared configuration shapes and JSON schema generation. |
| `contextforge-data-plane-cpex` | Plugin registry, runtime, and MCP/CMF adapters. |

User routing reads go through `UserConfigStore`. Runtime plugin configuration
has its own store in the CPEX crate. Routing uses explicit `ServiceRoute`
entries; it does not split or merge backend prefixes. Any change to published
names or routes must be coordinated with the publisher and tested. Hot-path
behavior changes must update the matching wiki page in the same change.

## Protocol migration

New behavior, tests, and examples target `2026-07-28`, `server/discover`, and
per-request metadata. Remaining legacy `initialize` code and tests are migration
artifacts, not supported client contracts or a reason to add compatibility.
Modern targeted calls already use request-scoped backend connections and do
not require sticky routing. Aggregate list methods and completion are currently
rejected; proposed snapshot-backed catalogs are future work.

## ContextForge Integration Contract

The integration is evolving. These are the contracts consumed by the current
Rust source; publisher changes must be checked against them.

| Agreement | Current Rust behavior |
| --- | --- |
| Direct MCP route | `/contextforge-rs/servers/{virtual_host_id}/mcp`. An ingress can expose a different public prefix if explicitly configured to rewrite it. |
| Protocol | Modern Streamable HTTP. Deployments must keep older clients and legacy SSE on Python routes. |
| Unknown virtual host | HTTP `404` with `{"detail":"Server not found"}`. |
| JWT trust | RSA/EC keys from `--jwks-url`; no fixed issuer or audience is enforced. See [JWT claims](config.md#jwt-claims-validated-by-claims_layer). |
| Principal | Default user aliases: `sub`, `user_id`, `UserId`; tenant aliases: `tenantId`, `tenant_id`. Both IDs must be strings. CEL can supply a custom mapping. |
| User config key | MessagePack-encoded `User::new(principal.user_id)`, not a raw user-ID string. Tenant ID is currently absent from the storage/cache key. |
| User config value | MessagePack `UserConfig`, including virtual hosts, backend definitions, and explicit object routes. |
| Schemas | `schemas/user.json` and `schemas/user_config.json`. |
| Plugin document | `ContextForgeGatewayRuntimePluginConfig`, JSON or MessagePack, with `version: 1` and `cpex`. |

Publishing a backend alone does not publish its tools: each callable object
needs a route. The user ID extracted from the token must match the publisher's
key. Requiring a tenant claim does not provide tenant partitioning of that key;
see [Security](security.md#authentication-and-authorization).

Coordinate changes with the control-plane publisher and `cf-integration`.
Regenerate both schemas after changes to their shared model types:

```bash
cargo run -p contextforge-data-plane-apis
```

## System topology

A typical deployment places nginx in front of both repositories:

```text
client -> nginx -> management route -> control plane -> management database
               |                           |
               |                           +-> publish config -> Redis
               |
               +-> Python MCP route -> built-in dataplane -> backend
               |
               +-> modern external route -> Rust external dataplane
                                               |-> read config from Redis
                                               |-> fetch trusted JWKS
                                               +-> call selected backend
```

The reference `docker/nginx.conf` routes by URL prefix; it does not inspect the
MCP protocol version. Selecting compatible clients and routes is a deployment
responsibility. See [Deployment](deployment.md#nginx-front-door-routing).

### How the control plane publishes config to the external dataplane

The control-plane `dataplane_publisher.py` writes runtime documents to Redis.
The Rust service reads user configuration through `UserConfigStore` and refreshes
its local cache after expiry. Plugin configuration uses a separate reload
watcher. Neither mechanism calls a control-plane management API per MCP request.
JWKS retrieval is a separate HTTP dependency and may be hosted by the issuer
or control plane.

Production builds read published configuration. The development-only
`with_tools` feature also exposes unauthenticated token and config-write
helpers; it must be excluded from production builds.

## External dependencies and integration points

- **Redis** distributes user routing and plugin configuration.
- **JWKS endpoint** supplies public token verification keys.
- **Backend MCP servers** execute selected operations. `fast_time_server` and
  the counter server are test fixtures, not required gateway clock services.
- **Tokio, Axum, and RMCP** provide asynchronous execution, HTTP middleware, and MCP transport handling.
