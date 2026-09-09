# Deployment

Modern requests are independent and use a fresh backend MCP service per
operation. There is no sticky-session requirement. Follow
[Getting Started](getting-started.md) for a local development environment.

## Checklist

1. Route the configured `/contextforge-rs` prefix to the external dataplane and
   keep older clients and legacy SSE on Python routes.
2. Configure a reachable trusted `--jwks-url` and a principal mapping matching
   the publisher's user IDs and tenant claims.
3. Provide Redis connectivity and control-plane publication
   (`DATAPLANE_PUBLISHER=true` in the control-plane deployment). Restrict writes
   to trusted publishers; use TLS/mTLS across trust zones.
4. Match the upstream connection mode to backend URL schemes and TLS identities.
5. Exclude `with_tools` from production builds. Compile `plugins` when needed,
   publish a valid plugin document before startup, and enable runtime execution.
6. Configure Origin and Host allowlists for the public deployment.
7. Configure telemetry collectors and both enable flags when exporting metrics.
8. Size file-descriptor limits, CPU, and memory for measured concurrent traffic.

## Health Endpoint

`GET /contextforge-rs/health` returns HTTP `200` and `{"status":"healthy"}`
without authentication in every build. It checks HTTP liveness, not Redis,
JWKS, plugin reload health, or backend readiness. The reference nginx also
exposes it at `/health`. Verify an authenticated routed
request separately when checking deployment readiness.

## nginx Front-Door Routing

The reference `docker/nginx.conf` sends `/contextforge-rs` to the external
service and other paths to the Python service. It routes by prefix and does
not inspect protocol versions. The Python service owns its management and
built-in MCP routes; an ingress must keep legacy clients off the external route.

Its upstream retry policy allows connection-stage failover, with two tries
within ten seconds for configured error/timeout/502/503/504 conditions. It does
not enable retrying non-idempotent POSTs after they have been sent upstream.
Do not add blind retries of `tools/call`: a lost response does not prove the
backend operation failed to execute.

## Replicas and Failover

Each request verifies identity, loads configuration, resolves its published
route, and opens its own backend connection. Replicas need consistent JWKS
trust, compatible compiled plugin factories, and the same published configuration;
they do not need affinity by `Mcp-Session-Id`.

A process failure can interrupt an in-flight call. Subsequent modern requests
can go to another healthy replica without initialization, subject to its own
configuration-cache freshness and dependency availability.

## Redis Availability

- Redis is needed for initial configuration-store setup and uncached reads.
- Connection setup uses a manager configured for 1,000 retries.
- Warm configuration entries can survive an outage until their expiry (default 60 seconds).
- A missing entry or Redis GET error currently produces HTTP `400`; undecodable
  configuration produces `500`. See [Failure Modes](failure-modes.md).
- Enabled CPEX also needs its initial plugin document and checks for reloads
  every ten minutes. An invalid reload fails new plugin calls closed.

## Builds and Images

Build a production binary with bundled plugin factories and without local
bootstrap helpers:

```bash
make docker-prod
# Equivalent native build:
cargo build --locked --release -p contextforge-data-plane --features plugins
```

That feature compiles factories; `--runtime-plugins-enabled true` and a valid
Redis plugin document are still required to execute them. Production uses the
issuer's JWKS endpoint and does not supply a local token-signing private key.

`make docker-prod`, `docker/Dockerfile`, the image publishing workflow, and
CI's conformance binary build enable production plugin factories without
`with_tools`. Do not use `--all-features` for production artifacts: it enables
unauthenticated testing helpers and demo plugins. Set `--jwks-url` or
`CONTEXTFORGE_DATA_PLANE_JWKS_URL` to the token issuer's HTTPS JWKS endpoint;
the production dataplane does not receive a signing private key.

The image workflow publishes `ghcr.io/<owner>/contextforge-data-plane:latest`
and `:v<version>` on pushes to `main`, using the Cargo package version. Repeated
builds can overwrite either tag; **pin an image digest** for reproducibility.
The current Docker builder is `rust:1.96.1`.

The reference Compose stack sets `nofile 65535` and TCP tuning, but its resource
reservations must fit the host. These are example settings, not measured
requirements for every deployment.

## TLS Choices

| Leg | Options |
| --- | --- |
| Front door to gateway | HTTP on a trusted private network, or `--tls-address` with server certificate/key. HTTP and TLS listeners can run on distinct sockets. |
| Gateway to JWKS | HTTPS, optionally with `--jwks-ca-cert-path`. Plain HTTP is restricted to loopback testing. |
| Gateway to Redis | `--redis-mode plain-text`, `tls`, or `mtls`; use TLS/mTLS across trust zones. |
| Gateway to backends | HTTPS-only by default; explicitly select HTTP or mTLS modes as needed. |

## Config Propagation Delay

With healthy publication and reads, a useful staleness budget is:

```text
publisher interval + user-config cache expiry + publication/read latency
```

The Rust cache defaults to 60 seconds; check the deployed publisher's actual
interval. For functional tests, shorten publication and use cache expiry `0`.
For benchmarks, report both values and keep them consistent between runs.
CPEX reloads use a separate ten-minute interval.

## Security Posture

JWT verification currently does not enforce a fixed issuer/audience, mandatory
expiration, scopes, or tenant-partitioned config keys. Ensure the issuer and
publisher contracts fit those limitations. JWKS keys can remain cached for
five minutes after removal. See [Security](security.md) for the full boundary.

Development token, JWKS, and config-write helpers are unauthenticated and
compiled only with `with_tools`. Health is unauthenticated in all builds.
Redis writers control routes and plugin configuration, and plugin code is fully
trusted in-process code.
