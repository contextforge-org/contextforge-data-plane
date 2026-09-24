# ContextForge Data Plane

The Rust data plane for
[ContextForge](https://github.com/IBM/mcp-context-forge). It accepts modern MCP
traffic, loads control-plane-published configuration from Redis, and routes
authorized requests to configured MCP backends.

Architecture, configuration, operations, and development context lives
in the wiki under [`_context/wiki/`](_context/wiki/index.md).

## Quick Start

Build the production image and start the default data-plane test stack:

```bash
export CONTEXTFORGE_DATA_PLANE_JWKS_URL=https://your-issuer.example/.well-known/jwks.json
make docker-prod
make compose-up
```

The default stack omits the Python control plane. `register_fast_time` runs the
`cf-integration` helper fixture command after Redis and Fast Time are healthy;
it discovers the backend catalog and publishes the virtual-host routing snapshot
directly to Redis for the Rust dataplane. The default backend URL is
`http://fast_time_server:8880/mcp`, the virtual server ID is
`b8e3f1a2c4d5e6f7a1b2c3d4e5f6a7b8`, and the protocol version is `2026-07-28`.
Override `CF_FAST_TIME_SERVER_ID`, `CF_FAST_TIME_BACKEND_URL`, or
`CF_HELPERS_IMAGE` when needed.

To exercise control-plane publication as well, opt into its service:

```bash
SERVICES="nginx gateway redis postgres pgbouncer migration control-plane fast_time_server register_fast_time" \
  make compose-up
```

The production image includes the plugin factories and
`/contextforge-rs/health`; it does not include `with_tools`. That feature is for
testing only and enables unauthenticated token, JWKS, and config helpers. See
[getting-started.md](_context/wiki/getting-started.md) for the full-stack smoke
test and then stop the default stack with:

```bash
make compose-down
```

If you enabled the optional services, pass the same `SERVICES` value to
`compose-down`.

## Run the Binary from Cargo

For a lightweight host-development setup, start Redis and the MCP Rust SDK
counter and conformance fixtures:

```bash
docker compose -f docker/docker-compose-local.yaml up -d
docker compose -f docker/docker-compose-local.yaml ps redis gateway-one gateway-two
```

Then follow [getting-started.md](_context/wiki/getting-started.md) for the local cargo dev workflow.

## Runtime CPEX Plugins

Runtime CPEX plugins are disabled by default. When enabled, the data plane loads
validated plugin configuration from Redis and supports the narrow hook surface
documented in [config.md](_context/wiki/config.md).

The optional demo plugin crates still come from their independently hosted
`cpex-plugins-rs` repository; they are unrelated to the retired MCP SDK fork.

### Experimental Secrets Detection Plugin

The bundled secrets detection CPEX plugin is experimental. It is compiled into
the data plane with `contextforge-data-plane/plugins`; Redis config only
activates plugin factories that are already present in the binary.

Activation requires all three pieces:

- Compile-time feature: `contextforge-data-plane/plugins`
- Runtime flag: `--runtime-plugins-enabled true`
- Redis config key: `ContextForgeGatewayRuntimePluginConfig`

The plugin kind is `validator/secrets-detection`. The dataplane wires CMF hooks
for tool calls, prompt fetches, and resource reads.

Example run command:

```bash
cargo run --release \
  --features contextforge-data-plane/plugins \
  -- \
  --address 0.0.0.0:8001 \
  --redis-port 6379 \
  --redis-address 127.0.0.1 \
  --jwks-url "$CONTEXTFORGE_DATA_PLANE_JWKS_URL" \
  --number-of-cpus 16 \
  --redis-mode=plain-text \
  --upstream-connection-mode=plain-text-or-tls \
  --runtime-plugins-enabled true
```

### Testing-Only Bootstrap Helpers

For local testing, explicitly enable `with_tools` and use its local JWKS
endpoint. Never enable this feature or use `--all-features` in production.

```bash
cargo run -p contextforge-data-plane --features with_tools \
-- \
--address 127.0.0.1:8080 \
--redis-address 127.0.0.1 \
--redis-port 6379 \
--redis-mode plain-text \
--token-verification-private-key ./assets/jwt.key \
--jwks-url http://127.0.0.1:8080/contextforge-rs/admin/.well-known/jwks.json \
--upstream-connection-mode plain-text-or-tls \
--tls-address 127.0.0.1:8443 \
--server-private-key ./assets/tls_key.pem \
--server-certificate ./assets/tls_certificate.pem
```

## Tracing and Metrics

The data plane exports OTLP traces and metrics. Local Langfuse, OTel Collector, and Prometheus overlays are documented in [config.md](_context/wiki/config.md) under "Local Telemetry Verification Stack".

## Performance Tests

Performance testing uses the control-plane Locust suite through
[`cf-integration`](https://github.com/contextforge-org/contextforge-dev-tools).
See the [performance guide](_context/wiki/performance.md) for load and baseline
runs.
