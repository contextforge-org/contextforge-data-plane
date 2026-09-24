# Getting Started

## Local Cargo Dev Workflow

Run these commands from the repository root. You need the repository's Rust
toolchain, Docker Compose, and `curl`. Keep the dataplane running in one terminal
and use a second terminal for the requests below.

This quick start enables the bundled secrets-detection plugin, runtime plugin
execution, and local bootstrap helpers. `with_tools` exposes unauthenticated
token and config helpers for development only; keep the dataplane on loopback.
Production builds must omit `with_tools` and use the deployment's trusted JWT
issuer and published configuration.

### 1. Start Redis and the counter fixture

```bash
GATEWAY_CPU_LIMIT=2 GATEWAY_CPU_RESERVATION=1 \
GATEWAY_MEM_LIMIT=1G GATEWAY_MEM_RESERVATION=256M \
docker compose -f docker/docker-compose-local.yaml up --build -d redis gateway-one
docker compose -f docker/docker-compose-local.yaml ps redis gateway-one
docker compose -f docker/docker-compose-local.yaml exec -T redis redis-cli PING
```

Redis should reply `PONG`. The first fixture build can take several minutes.
These fixture limits fit a small local Docker VM; adjust them for load testing.

| Service | Endpoint | Role |
| --- | --- | --- |
| `redis` | `127.0.0.1:6379` | Runtime configuration store. |
| `gateway-one` | `http://127.0.0.1:5555/mcp` | MCP `2026-07-28` counter fixture. |
| Local dataplane | `http://127.0.0.1:8001/contextforge-rs` | Started with Cargo below. |

### 2. Seed plugin configuration before startup

Runtime plugin execution requires a valid
`ContextForgeGatewayRuntimePluginConfig` document in Redis. This enables
secrets detection before and after tool calls, blocking detected secrets:

```bash
docker compose -f docker/docker-compose-local.yaml exec -T redis \
  redis-cli SET ContextForgeGatewayRuntimePluginConfig '{
    "version": 1,
    "cpex": {
      "plugins": [{
        "name": "secrets-detection",
        "kind": "validator/secrets-detection",
        "hooks": ["cmf.tool_pre_invoke", "cmf.tool_post_invoke"],
        "config": {"block_on_detection": true}
      }]
    }
  }' NX
```

`NX` preserves an existing plugin document. `OK` means the example was inserted;
an empty reply means an existing document remains in use. Its plugin kinds must
be compiled into the binary. See [Plugin Config](config.md#plugin-config-redis-key-contextforgegatewayruntimepluginconfig)
for configuration and the optional [demo plugins](config.md#demo-plugin-workflow).

### 3. Start the dataplane

```bash
RUST_LOG=info \
cargo run -p contextforge-data-plane --features with_tools,plugins \
  --bin contextforge-data-plane -- \
  --address 127.0.0.1:8001 \
  --redis-address 127.0.0.1 \
  --redis-port 6379 \
  --redis-mode plain-text \
  --jwks-url http://127.0.0.1:8001/contextforge-rs/admin/.well-known/jwks.json \
  --token-verification-private-key assets/jwt.key \
  --upstream-connection-mode plain-text-or-tls \
  --runtime-plugins-enabled true \
  --user-config-cache-expiry-seconds 0
```

The `plugins` feature compiles bundled factories; `--runtime-plugins-enabled true`
loads their Redis configuration. The user-config cache is disabled for immediate
feedback when reseeding local routes. Use the default 60-second cache for normal
deployments. Telemetry export is optional and needs a collector; follow the
[local telemetry setup](config.md#local-telemetry-verification-stack) when needed.

`--features with_tools` forwards to `contextforge-data-plane-lib/with_tools`;
either spelling enables the same helpers. JWT verification now uses
`--jwks-url`; the former `--token-verification-public-key` and
`--token-verification-secret` flags are no longer accepted. The local JWKS
helper serves the public key corresponding to `assets/jwt.key`.

### 4. Check health and mint a local test token

In the second terminal:

```bash
BASE_URL=http://127.0.0.1:8001/contextforge-rs
TENANT_ID=team_awesome
USER_ID=11111111-1111-1111-1111-111111111111
VIRTUAL_HOST_ID=c0ffee00f001f00df00ddeadbeefdead

curl --fail --silent --show-error "${BASE_URL}/health"
curl --fail --silent --show-error "${BASE_URL}/admin/.well-known/jwks.json"

TOKEN=$(curl --fail --silent --show-error \
  "${BASE_URL}/admin/tokens/${TENANT_ID}/${USER_ID}?email=admin@example.com")
```

Expect `{"status": "healthy"}` and a JWKS document containing a `keys` array.
The token response is a raw JWT, stored in `TOKEN` without printing it. Tokens
expire after one hour; repeat the token command to refresh. Both tenant and user
path segments are required. The helper sets top-level `tenant_id` and `sub`
claims; the optional email does not select the user's Redis configuration.

Keep the listener, JWKS, token, and MCP URLs on the same instance. If you use
port `9090`, change all four together. Port `8080` belongs to the full Docker
stack's nginx front door and does not automatically reach this Cargo process.

### 5. Seed runtime routes for the token's user

```bash
curl --fail --silent --show-error --request POST \
  "${BASE_URL}/admin/userconfigs/${USER_ID}" \
  --header 'content-type: application/json' \
  --data '{
    "virtual_hosts": {
      "c0ffee00f001f00df00ddeadbeefdead": {
        "backends": {
          "gateway-one": {
            "name": "gateway-one",
            "url": "http://127.0.0.1:5555/mcp",
            "mcp_protocol_version": "2026-07-28",
            "passthrough_headers": []
          }
        },
        "tools": {
          "counter-get_value": {
            "backend_name": "gateway-one",
            "upstream_name": "get_value"
          }
        }
      }
    }
  }'
```

Expect `Added` (HTTP `202`). The `USER_ID` must match the token's `sub`.
The backend protocol version and explicit tool route are required for the tool
call below. The public tool name maps to the backend's original `get_value`.

### 6. Discover the server and call the counter

```bash
curl --fail --silent --show-error \
  "${BASE_URL}/servers/${VIRTUAL_HOST_ID}/mcp" \
  --header "authorization: Bearer ${TOKEN}" \
  --header 'content-type: application/json' \
  --header 'accept: application/json, text/event-stream' \
  --header 'mcp-protocol-version: 2026-07-28' \
  --header 'mcp-method: server/discover' \
  --header 'mcp-name: quickstart' \
  --data '{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "server/discover",
  "params": {
    "_meta": {
      "io.modelcontextprotocol/protocolVersion": "2026-07-28",
      "io.modelcontextprotocol/clientInfo": {
        "name": "curl",
        "version": "0.1.0"
      },
      "io.modelcontextprotocol/clientCapabilities": {}
    }
  }
}'

curl --fail --silent --show-error \
  "${BASE_URL}/servers/${VIRTUAL_HOST_ID}/mcp" \
  --header "authorization: Bearer ${TOKEN}" \
  --header 'content-type: application/json' \
  --header 'accept: application/json, text/event-stream' \
  --header 'mcp-protocol-version: 2026-07-28' \
  --header 'mcp-method: tools/call' \
  --header 'mcp-name: counter-get_value' \
  --data '{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "counter-get_value",
    "arguments": {},
    "_meta": {
      "io.modelcontextprotocol/protocolVersion": "2026-07-28",
      "io.modelcontextprotocol/clientInfo": {
        "name": "curl",
        "version": "0.1.0"
      },
      "io.modelcontextprotocol/clientCapabilities": {}
    }
  }
}'
```

Expect a discovery `result` and a successful tool `result` containing the counter
value. The tool call exercises the configured pre/post plugin hooks. An MCP
`error` can arrive with HTTP `200`, so inspect the JSON response as well.
Requests carry client metadata independently; no `initialize` or session header
is needed. Catalog listing is owned by the control plane, so use the published
tool name directly instead of expecting `tools/list` here.

### Troubleshooting

| Symptom | Check |
| --- | --- |
| `unexpected argument --token-verification-public-key` | Use the JWKS startup command above. |
| Startup fails with a plugin configuration error | Seed the plugin document before startup; its kinds must match compiled factories. Add `test-plugins` only for a document using demo factories. |
| Token/JWKS helper does not return the expected body | Enable `with_tools`, include both tenant and user in the token URL, and check that the port reaches this process. |
| `401` / `Invalid token` | Send `Authorization: Bearer ${TOKEN}`; mint a fresh token and check that the configured JWKS URL is reachable and serves its signing key. |
| `401` / `Unable to extract the principal from claims` | The default extractor needs a string user ID (`sub`) and string tenant ID (`tenant_id`); see [JWT Claims](config.md#jwt-claims-validated-by-claims_layer). |
| `400 Problem occurred retrieving the configuration` | Seed `UserConfig` for the same user ID as the token. |
| `404 {"detail":"Server not found"}` | The URL's virtual-host ID must exist in that user's config. |
| `400` mentioning request metadata | Include the matching MCP protocol header, method/name headers, and per-request `_meta`. |
| MCP error for an unpublished tool | Add the tool's explicit route to the virtual host before calling it. |
| Backend unavailable | Check `gateway-one` logs, port `5555`, and `--upstream-connection-mode plain-text-or-tls`. |

For JWT diagnostics, restart with `RUST_LOG=debug` and look for
`unable to refresh SaaS JWKS`, `validate_and_decode_claims`, or
`Can't extract the principal`. Do not share bearer tokens in logs or reports.

Stop Cargo with Ctrl-C. Stop the local dependencies when finished:

```bash
docker compose -f docker/docker-compose-local.yaml down
```

## Full Docker Stack

The default `make compose-up` topology starts nginx, the Rust dataplane,
Postgres, Redis, PgBouncer, the migration job, Fast Time, and a one-shot
`register_fast_time` helper. After Redis and Fast Time are healthy, the helper
runs `cf-integration __helper fixture` to discover the backend catalog and
publish the virtual-host routing snapshot directly to Redis. It does not
require the Python control plane.

```bash
make docker-prod
make compose-up
```

The default seed uses:

| Setting | Value |
| --- | --- |
| Virtual server ID | `b8e3f1a2c4d5e6f7a1b2c3d4e5f6a7b8` |
| Backend URL | `http://fast_time_server:8880/mcp` |
| Protocol version | `2026-07-28` |

Override `CF_FAST_TIME_SERVER_ID`, `CF_FAST_TIME_BACKEND_URL`, or
`CF_HELPERS_IMAGE` for another fixture or helper image. The helper image creates
a local `admin@example.com` subject when `MCP_CONFORMANCE_TOKEN` is not
provided; pass that environment variable when the route must be published for
another token subject.

To exercise control-plane publication as well, include its service explicitly:

```bash
SERVICES="nginx gateway redis postgres pgbouncer migration control-plane fast_time_server register_fast_time" \
  make compose-up
```

The production image includes plugin factories and health, and excludes
testing-only `with_tools` helpers and signing-key mounts. Obtain bearer tokens
through the configured issuer; the seeded Redis route is keyed by the token
subject used by the helper.

The reference nginx listener is `http://localhost:8080`. External MCP uses
`/contextforge-rs/servers/{virtual_host_id}/mcp`, and health is at `/health` or
`/contextforge-rs/health`. `fast_time_server` is a sample backend, not a
gateway dependency.

Stop the default stack with `make compose-down`; if the optional services were
enabled, pass the same `SERVICES` value to `compose-down`. Volumes are kept.

## cf-integration Conformance

Install the same released harness version used by CI. Conformance and Inspector
install and run Node/npm only inside Docker images.

```bash
cargo binstall cf-integration@0.3.2 --no-confirm
make conformance
```

This runs the modern client and modern server eras through the committed
external-dataplane `HEAD`, including fixture-direct server comparison and the
scoped client suite. It uses the production build without `with_tools`; the
harness owns JWT signing, loopback JWKS, and Redis fixture publication.
Use `make conformance-bless` to replace all selected
baselines transactionally after a fully successful run. Generated checkouts,
results, reports, and logs stay under `.integration/`.
