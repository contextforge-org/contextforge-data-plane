# Configuration Reference

## Minimum Required Flags

```text
--redis-address   --redis-port   --redis-mode   --jwks-url
```

Plus at least one listener: `--address` or `--tls-address`. Development builds
with `with_tools` also require `--token-verification-private-key`.

## Complete CLI and Environment Reference

The binary parses both CLI flags and environment variables with `clap`; a CLI
flag wins when both forms are supplied. Use the binary for the always-current
generated reference:

```bash
cargo run -p contextforge-data-plane --bin contextforge-data-plane -- --help
```

Most environment variables use the `CONTEXTFORGE_DATA_PLANE_` prefix. The MCP
Origin and Host settings retain the explicitly configured
`CONTEXTFORGE_GATEWAY_RS_` names shown below.

### Listeners and JWT

| Flag | Environment variable | Default / requirement | Purpose |
| --- | --- | --- | --- |
| `--address <ip:port>` | `CONTEXTFORGE_DATA_PLANE_ADDRESS` | Optional | Plain HTTP listener. |
| `--tls-address <ip:port>` | `CONTEXTFORGE_DATA_PLANE_TLS_ADDRESS` | Optional | TLS listener; requires server certificate and key. |
| `--server-certificate <path>` | `CONTEXTFORGE_DATA_PLANE_TLS_SERVER_CERTIFICATE` | With `--tls-address` | PEM certificate chain for downstream TLS. |
| `--server-private-key <path>` | `CONTEXTFORGE_DATA_PLANE_TLS_SERVER_PRIVATE_KEY` | With `--tls-address` | PEM private key for downstream TLS. |
| `--jwks-url <url>` | `CONTEXTFORGE_DATA_PLANE_JWKS_URL` | Required | Fetches RSA/EC JWT verification keys. HTTPS required except for loopback HTTP testing. |
| `--jwks-ca-cert-path <path>` | `CONTEXTFORGE_DATA_PLANE_JWKS_CA_PATH` | Optional | PEM CA bundle trusted by the JWKS HTTP client. |
| `--token-verification-private-key <path>` | None (CLI only) | Required when built with `with_tools` | Signs local test tokens and supplies the public key served by the local JWKS helper. |
| `--cel-principal-extractor-path <path>` | None (CLI only) | Optional | CEL principal mapping for custom claim layouts; otherwise uses the default user/tenant claim mapping below. |

The former `--token-verification-public-key` and `--token-verification-secret`
flags are no longer accepted. For the local signing/JWKS setup, follow
[Getting Started](getting-started.md#local-cargo-dev-workflow).

### MCP request validation

| Flag | Environment variable | Default | Purpose |
| --- | --- | --- | --- |
| `--mcp-allowed-origins <origin,...>` | `CONTEXTFORGE_GATEWAY_RS_MCP_ALLOWED_ORIGINS` | None | Browser Origin allowlist. Without it, requests lacking `Origin` pass and every request carrying `Origin` receives HTTP `403`. |
| `--mcp-allowed-hosts <authority,...>` | `CONTEXTFORGE_GATEWAY_RS_MCP_ALLOWED_HOSTS` | None | Optional RMCP request-authority allowlist. For requests that reach the RMCP service, missing or malformed authorities receive HTTP `400`; unlisted authorities receive HTTP `403`. Earlier middleware may return first. |
| `--mcp-standard-header-max-count <n>` | `CONTEXTFORGE_DATA_PLANE_MCP_STANDARD_HEADER_MAX_COUNT` | `32` | Maximum MCP standard headers accepted on one request. |
| `--mcp-standard-header-max-value-bytes <n>` | `CONTEXTFORGE_DATA_PLANE_MCP_STANDARD_HEADER_MAX_VALUE_BYTES` | `8192` | Maximum byte length accepted for one MCP standard header value. |
| `--mcp-standard-header-max-total-bytes <n>` | `CONTEXTFORGE_DATA_PLANE_MCP_STANDARD_HEADER_MAX_TOTAL_BYTES` | `65536` | Approximate request-level aggregate bytes across all matched MCP standard header names and values. |

Values are comma-separated. Origin entries must be fully qualified serialized
origins such as `https://app.example.com`; Host entries are authorities such as
`gateway.example.com` or `gateway.example.com:8443`. See [Security](security.md#mcp-origin-and-host-validation).
The MCP standard header limits apply to `Mcp-Method`, `Mcp-Name`,
`Mcp-Protocol-Version`, and `Mcp-Param-*`. The same guardrail also covers the
legacy/RMCP transport header `Mcp-Session-Id`. A configured value of `0` is
treated as the documented default. The byte totals are application-level
aggregate budgets based on all matched header name and value lengths on one
request; they do not allow a single oversized value, which is still capped by
`--mcp-standard-header-max-value-bytes`. They are not exact wire-size accounting
and do not model HTTP/2 header compression. Non-MCP headers remain bounded by
the HTTP transport.

### Redis

| Flag | Environment variable | Default / requirement | Purpose |
| --- | --- | --- | --- |
| `--redis-address <host>` | `CONTEXTFORGE_DATA_PLANE_REDIS_HOSTNAME` | Required | Redis host name or IP. |
| `--redis-port <port>` | `CONTEXTFORGE_DATA_PLANE_REDIS_PORT` | Required | Redis port. |
| `--redis-mode <mode>` | `CONTEXTFORGE_DATA_PLANE_REDIS_CONNECTION_MODE` | Required | `plain-text`, `tls`, or `mtls`. |
| `--redis-tls-trust-bundle <path>` | `CONTEXTFORGE_DATA_PLANE_REDIS_TLS_REDIS_TRUST_BUNDLE` | TLS and mTLS | PEM trust bundle. |
| `--redis-tls-client-certificate <path>` | `CONTEXTFORGE_DATA_PLANE_REDIS_TLS_REDIS_CLIENT_CERTIFICATE` | mTLS | PEM client certificate. |
| `--redis-tls-client-private-key <path>` | `CONTEXTFORGE_DATA_PLANE_REDIS_TLS_REDIS_CLIENT_PRIVATE_KEY` | mTLS | PEM client private key. |
| `--user-config-cache-expiry-seconds <n>` | `CONTEXTFORGE_DATA_PLANE_USER_CONFIG_CACHE_EXPIRY_SECONDS` | `60` | In-process cache expiry; `0` reads Redis on every request. |

### Upstream connections

| Flag | Environment variable | Default / requirement | Purpose |
| --- | --- | --- | --- |
| `--upstream-connection-mode <mode>` | `CONTEXTFORGE_DATA_PLANE_UPSTREAM_CONNECTION_MODE` | `tls-only` | Permits HTTPS only, HTTP and HTTPS, or an mTLS mode. |
| `--upstream-trust-bundle <path>` | `CONTEXTFORGE_DATA_PLANE_TLS_UPSTREAM_TRUST_BUNDLE` | Optional | Additional PEM trust bundle for HTTPS backends. |
| `--upstream-certificate <path>` | `CONTEXTFORGE_DATA_PLANE_TLS_UPSTREAM_CERTIFICATE` | mTLS modes | PEM client certificate. |
| `--upstream-private-key <path>` | `CONTEXTFORGE_DATA_PLANE_TLS_UPSTREAM_PRIVATE_KEY` | mTLS modes | PEM client private key. |

### Runtime and plugins

| Flag | Environment variable | Default | Purpose |
| --- | --- | --- | --- |
| `--number-of-cpus <n>` | `CONTEXTFORGE_DATA_PLANE_NUMBER_OF_CPUS` | Unset | Parsed but currently unused by the Tokio entry point. |
| `--single-runtime <bool>` | `CONTEXTFORGE_DATA_PLANE_SINGLE_RUNTIME` | Unset | Parsed but currently unused; the binary starts one Tokio runtime. |
| `--runtime-plugins-enabled <bool>` | `CONTEXTFORGE_DATA_PLANE_RUNTIME_PLUGINS_ENABLED` | `false` | Enables compiled-in CPEX hooks and Redis plugin config loading. |

### Telemetry and logging

| Flag | Environment variable | Default | Purpose |
| --- | --- | --- | --- |
| `--enable-open-telemetry <bool>` | `CONTEXTFORGE_DATA_PLANE_ENABLE_OPEN_TELEMETRY` | `false` | Enables OTLP trace export. |
| `--enable-otel-metrics <bool>` | `CONTEXTFORGE_DATA_PLANE_ENABLE_OTEL_METRICS` | `false` | Enables OTLP HTTP-server metric export when `--enable-open-telemetry true` is also set. |
| `--otlp-protocol <protocol>` | `CONTEXTFORGE_DATA_PLANE_OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | `grpc` or `http-protobuf`. |
| `--otlp-endpoint <uri>` | `CONTEXTFORGE_DATA_PLANE_OTEL_EXPORTER_OTLP_ENDPOINT` | Protocol-specific | Trace endpoint; defaults to `http://127.0.0.1:4317` for gRPC or `http://127.0.0.1:4318/v1/traces` for HTTP. |
| `--otlp-metrics-endpoint <uri>` | `CONTEXTFORGE_DATA_PLANE_OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | Protocol-specific | Metrics endpoint; defaults to `http://127.0.0.1:4317` for gRPC or `http://127.0.0.1:4318/v1/metrics` for HTTP. |
| `--otlp-headers <headers>` | `CONTEXTFORGE_DATA_PLANE_OTEL_EXPORTER_OTLP_HEADERS` | None | Comma-separated `key=value` exporter headers. |
| `--otlp-service-name <name>` | `CONTEXTFORGE_DATA_PLANE_OTEL_SERVICE_NAME` | `CONTEXTFORGE-DATA-PLANE` | OpenTelemetry `service.name`. |
| `--log-name <name>` | `CONTEXTFORGE_DATA_PLANE_LOG_NAME` | Unset | Parsed but currently unused; no file logger is installed. |
| `--log-rotation <mode>` | `CONTEXTFORGE_DATA_PLANE_LOG_ROTATION` | Unset | Parsed but currently unused; no file rotation is installed. |

## JWT Claims (validated by `claims_layer`)

The JWT signature is checked against the configured JWKS. The default principal
extractor then requires user and tenant IDs at the top level of the claims:

| Claim | Current behavior |
| --- | --- |
| `sub`, `user_id`, `UserId` | First present alias must be a string; supplies the user ID used for Redis config lookup. |
| `tenantId`, `tenant_id` | First present alias must be a string; supplies the principal's tenant ID. |
| `exp` | Checked when present; the local helper sets a one-hour expiry. |
| `nbf` | Checked when present; rejects tokens that are not yet valid, subject to verifier leeway. |
| `iss`, `aud` | No fixed issuer or audience is currently enforced by the JWKS verifier. |

The default extractor does not infer the tenant from `teams`, email, or a nested
`user` object. Use `--cel-principal-extractor-path` for a custom mapping.
An earlier alias with a non-string value prevents fallback to a later alias.
The tenant ID is required by extraction but is not currently included in the
user-config Redis/cache key. JWT scopes and RBAC are not enforced here; object
visibility comes from the published routing maps.

The local `GET /contextforge-rs/admin/tokens/{tenant_id}/{user_id}` helper sets
`tenant_id` and `sub` from the path. Its raw JWT response belongs in the
`Authorization: Bearer ...` header; it is not a JSON token object.

There is no per-token revocation. Verification keys are cached for five minutes;
removing a key from JWKS is not immediate invalidation of cached keys. Restart
the dataplane after removing a key if that cache must be cleared immediately.

## UserConfig Shape (from `contextforge-data-plane-apis`)

```text
UserConfig
  virtual_hosts: HashMap<String, VirtualHost>

VirtualHost
  backends: HashMap<String, BackendMCPGateway>  ← backend key, not a parsed prefix
  tools: HashMap<String, ServiceRoute>         ← public tool name → route
  resources: HashMap<String, ServiceRoute>     ← public resource URI → route
  resource_templates: HashMap<String, ServiceRoute>
  prompts: HashMap<String, ServiceRoute>       ← public prompt name → route

ServiceRoute
  backend_name: String                        ← key in backends
  upstream_name: String                       ← backend-local name or URI

BackendMCPGateway
  name: String
  url: Url
  mcp_protocol_version: ProtocolVersion        ← required
  passthrough_headers: Vec<String>             ← required; current request's headers
  add_headers: HashMap<String, String>         ← defaults to {}
  remove_headers: Vec<String>                  ← defaults to []
  completion: HashMap<String, String>          ← defaults to {}; completion is not implemented
  tool_schemas: HashMap<String, JsonObject>    ← defaults to {}; upstream tool name → schema
```

The virtual-host object maps default to empty. A backend must be referenced by
an explicit object route to be callable. `resource_templates` is part of the
published model, but template listing is rejected and resource reads currently
use exact entries in `resources`.


`tool_schemas` lets the dataplane recognize and validate `x-mcp-header`
annotations without calling backend `tools/list`. The control plane may omit the
field or individual unannotated tools. Without a published schema, parameter
headers are forwarded as unrecognized intermediary headers and are not locally
validated. A published annotation must name a non-empty, case-insensitively
unique HTTP token on a `string`, `integer`, or `boolean` property reachable from
the schema root through `properties` keys only. Nested properties use their
exact property path. For a recognized annotation, a non-null argument requires
an equal header; an absent or null argument requires the header to be absent.
Integer values are limited to the IEEE 754 safe range.

**Header apply order:** backend Host for HTTPS → configured passthrough →
automatic `Mcp-Param-*` forwarding → `add_headers` → `remove_headers` → current
trace-context injection. RMCP generates the outbound method, name, and protocol
headers for the routed request.

Passthrough values come from the current HTTP request. A new backend transport
is constructed per routed operation; no initialization-time header snapshot is
reused across requests.

**Protected headers** — silently skipped in all three phases (passthrough/add/remove):

| Category | Headers |
| --- | --- |
| Body-framing | `Content-Length`, `Content-Type` |
| Hop-by-hop | `Connection`, `Keep-Alive`, `Proxy-Authenticate`, `Proxy-Authorization`, `Proxy-Connection`, `TE`, `Trailer`, `Trailers`, `Transfer-Encoding`, `Upgrade` |
| RMCP-reserved | `Mcp-Session-Id`, `Accept`, `Last-Event-Id` |
| Gateway-managed | `Host` (set from backend URL host + port; never overridden by config) |
| MCP standard | `Mcp-Method`, `Mcp-Name`, `Mcp-Protocol-Version`, `Mcp-Param-*` |

`Authorization` and `Cookie` are not protected here because backend
authentication through `passthrough_headers` or `add_headers` is intentional
runtime configuration.

Redis storage: `MessagePack(User::new(principal.user_id))` → `MessagePack(UserConfig)`.

Two schemas are generated — both must be regenerated and committed when `UserConfig`, `VirtualHost`, `BackendMCPGateway`, or the `User` key type changes:

| Schema file | Covers |
| --- | --- |
| `schemas/user_config.json` | `UserConfig` routing document written to Redis. |
| `schemas/user.json` | `User` key type used as the Redis key. |

```bash
cargo run -p contextforge-data-plane-apis
```

## Plugin Config (Redis key: `ContextForgeGatewayRuntimePluginConfig`)

```text
RuntimePluginConfigDocument
  version: 1
  cpex: CpexConfig
```

Supported: tool, prompt, and resource pre/post CMF hooks.
Rejected: routing-based selection, routes, plugin directories, global policies/defaults,
`plugin_settings.fail_on_plugin_error`, plugin conditions, and unsupported hooks
(including LLM hooks).
Config validation and `CmfPluginFactory` registration must agree on that list: a hook accepted by validation but not registered leaves the plugin loaded and silently inert.
Reload watcher: 10-minute interval. Invalid reload → runtime marked failed.

Compile bundled factories with `--features plugins` and enable execution with
`--runtime-plugins-enabled true`. A valid document must exist before startup;
a missing document fails initialization. The
[quick start](getting-started.md#2-seed-plugin-configuration-before-startup)
seeds a secrets-detection policy. `test-plugins` additionally compiles the demo
factories used below; it is not needed for bundled secrets detection.

### Tool Call Hook Behavior

For `call_tool`, the pre hook runs after backend routing has resolved the published route to its backend and upstream tool name. The hook sees the backend name, routed tool name, and arguments. It can leave arguments unchanged, replace arguments, or deny the call.

After the upstream backend returns, the post hook can leave the result unchanged, rewrite the result payload, or deny the response. Hook state is carried across the upstream call so pre and post hooks can share CPEX context for the same logical tool call.

Plugin execution must not poison shared gateway state. A plugin denial becomes an MCP error. Soft plugin errors are logged. Unsupported plugin configuration fails validation before the runtime is accepted.

### Prompt Fetch Hook Behavior

For `get_prompt`, the pre hook runs after backend routing, so the plugin sees the backend-local prompt name and the owning backend separately from the published route. It can leave the arguments unchanged, replace them, or deny the fetch before the backend renders anything.

The post hook receives the rendered prompt as one CMF message per rendered MCP message, each carrying its role and its content block: text, image, audio, embedded resource, or resource link. A plugin can inspect or rewrite any of them, so a policy can act on a file interpolated into a prompt rather than only on the surrounding text.

Writing plugin edits back follows three rules:

- A message the plugin left unchanged is returned exactly as the backend sent it, so annotations, `_meta`, and binary resource blobs survive untouched.
- A message the plugin changed is rebuilt from CMF. CMF does not model MCP annotations or `_meta`, so an edited message loses them.
- Edits that cannot be applied faithfully fail the call rather than falling back to the backend's original. A changed message count, anything other than exactly one prompt result in the payload, a role MCP prompts cannot express, or a resource whose text the plugin removed all return an error. Silently restoring the backend's content would undo a redaction.

MCP prompt results carry no error flag, so a plugin setting `is_error` on the CMF prompt result is rejecting the prompt rather than describing it. The gateway turns that into an MCP error carrying the plugin's `error_message`, and the rendered content never reaches the client. This differs from tools, where `is_error` is a field on `CallToolResult` and is forwarded as a successful response.

Binary resources embedded in prompts reach plugins by URI and MIME type but not by content. A plugin can deny such a message; editing one fails the write-back. Resource-read hooks below have their own binary conversion.

### Resource Read Hook Behavior

For `resources/read`, the pre hook receives the canonical backend-local URI and may allow, deny or rewrite it. A rewritten URI must resolve unambiguously through the caller's published virtual-host resources before a backend connection is opened. Aliases for the same backend target do not create ambiguity.

The post hook may replace each returned resource's text or binary content, URI and MIME type, including converting text to a blob or a blob to text. Existing MCP `_meta` is preserved. CMF-only envelope and descriptive fields do not restrict these changes. Each resource still needs a valid MCP content representation; binary resource reads are decoded for CPEX and re-encoded after edits, while unchanged blob bytes retain their original wire value. This resource path does not add prompt-wide payload validation.

The pre call returns an opaque, concrete `ResourceHookState` consumed by the post call. It captures both the runtime and the decision to run or skip post hooks before backend I/O. A reload only affects subsequent requests, including when it enables or disables resource hooks. Callers cannot construct missing or mismatched active state, and requests without a post hook allocate no correlation state.

### Demo Plugin Workflow

The optional `test-plugins` feature compiles demo factories from the `cpex-plugins-rs` repository. Redis configuration activates factories already present in the binary; it never loads new Rust code into a running process.

Start Redis and the counter fixture using the
[quick-start dependency command](getting-started.md#1-start-redis-and-the-counter-fixture).
The following command replaces the local plugin document with payload-marker
configuration; run it before starting the ContextForge external dataplane:

```bash
docker compose -f docker/docker-compose-local.yaml exec -T redis \
  redis-cli SET ContextForgeGatewayRuntimePluginConfig '{
    "version": 1,
    "cpex": {
      "plugins": [
        {
          "name": "payload-marker",
          "kind": "contextforge/payload-marker",
          "hooks": ["cmf.tool_post_invoke"]
        }
      ]
    }
  }'
```

For local testing only, build and run with demo factories, `with_tools` helpers,
and runtime execution enabled:

```bash
cargo run -p contextforge-data-plane \
  --features with_tools,plugins,test-plugins \
  --bin contextforge-data-plane -- \
  --address 127.0.0.1:8001 \
  --redis-address 127.0.0.1 \
  --redis-port 6379 \
  --redis-mode plain-text \
  --jwks-url http://127.0.0.1:8001/contextforge-rs/admin/.well-known/jwks.json \
  --token-verification-private-key assets/jwt.key \
  --upstream-connection-mode plain-text-or-tls \
  --runtime-plugins-enabled true
```

Startup should log successful CPEX initialization. The payload marker appends `[cpex:payload-marker]` to successful tool results. The hook path is also covered by:

```bash
cargo nextest run --locked -p contextforge-data-plane-lib --test gateway -E 'test(plugins::)'
```

## Configuration Validation and Readiness

| Configuration / dependency | When it is checked |
| --- | --- |
| Missing required flags | CLI parsing. |
| No HTTP or TLS listener | Gateway startup. |
| TLS listener without certificate/key | Listener setup. Use different sockets for HTTP and TLS. |
| Redis TLS without trust bundle, or mTLS without client certificate/key | Redis configuration/connection setup. |
| mTLS upstream without certificate/key | Upstream HTTP client construction. |
| Invalid JWKS URL scheme or non-loopback plain HTTP URL | Authorization-service construction. |
| Missing/invalid plugin document when enabled | CPEX initialization, before serving requests. |
| Unreachable JWKS endpoint | Token verification when keys must be fetched. |
| Unreachable backend or HTTP URL with default HTTPS-only mode | When a request selects that backend. |

Redis connection setup retries rather than failing immediately. The local
signing-key file is used by helper requests. A successful health probe does
not prove Redis, JWKS, signing helpers, or backends are ready.

## Upstream Connection Modes

| Mode | Behavior |
| --- | --- |
| omitted / `tls-only` | HTTPS backends only (safe default) |
| `plain-text-or-tls` | HTTP or HTTPS (use for local Compose backends) |
| `plain-text-or-m-tls` | HTTP or HTTPS + client identity |
| `mtls-only` | HTTPS + client cert/key required |

## Logging Env Vars

| Variable | Controls |
| --- | --- |
| `RUST_LOG` | Console event filter. |
| `RUST_TRACE_LOG` | OTLP span filter. |

Both fall back to `debug` with quieter dependency directives:
`hyper_util=off,tower_http=off,rmcp=warn,reqwest=warn,rustls=warn,h2=warn,opentelemetry_sdk=warn,opentelemetry-otlp=warn`.
`RUST_FILE_LOG` is not read by the current logger. The parsed `--log-name` and
`--log-rotation` fields also have no effect. Redirect console output or use the
process/container log collector for persisted logs.

## Telemetry Debugging Notes

HTTP request spans are emitted at `info`; `RUST_TRACE_LOG=info` includes them.
`debug` is optional for additional instrumentation, not a requirement for
export. `--enable-open-telemetry true` installs the trace provider and W3C
propagator. In the current startup implementation, metrics initialization is
inside that same branch: set **both** telemetry enable flags to export metrics.

Metrics are pushed every **30 seconds**. The supplied Prometheus scrape interval
is **15 seconds**; allow up to about 45–60 seconds after generating traffic.

| Symptom | Where to look |
| --- | --- |
| `401` | Bearer header, `validate: unable to refresh SaaS JWKS`, `validate_and_decode_claims`, and `Can't extract the principal` logs. |
| `400` config error | `user_config_store_layer` and whether the publisher used the extracted user ID. A Redis GET failure also maps here. |
| `404 Server not found` | `virtual_host_config_layer`; requested vhost versus caller's published configuration. |
| MCP routing errors | `AuthorizedCallValidator::validate` log prefix (from `validate_stateless`), then `call_tool`, `read_resource`, or `get_prompt` diagnostics. |
| Backend failures | Per-request connection/call diagnostics; no initialization fan-out exists. |
| Plugin problems | CPEX initialization, pipeline, and reload logs. |
| Missing traces | Enable flag, `RUST_TRACE_LOG`, exporter endpoint/protocol, and exporter error logs. |
| Missing metrics | Both enable flags, metrics endpoint, 30-second export interval, then collector/Prometheus scrape status. |

## Local Telemetry Verification Stack

The collector overlay receives OTLP/HTTP on `:4318`, writes traces to its own
logs, and exposes metrics on `:8889` for Prometheus. It does **not** forward
traces to Langfuse. Prometheus is available at `http://localhost:9090`.

```text
external dataplane
  -> OTLP/HTTP :4318 -> OTel Collector -> trace logging exporter
                            ^
                            | scrape metrics :8889
                      Prometheus :9090
```

First complete the [local quick start](getting-started.md#local-cargo-dev-workflow),
including plugin configuration. Add only the telemetry services:

```bash
docker compose \
  -f docker/docker-compose-local.yaml \
  -f docker/docker-compose-otel-collector.yaml \
  up -d otel-collector prometheus
```

Stop the quick-start Cargo process, then restart it with export enabled:

```bash
RUST_LOG=info RUST_TRACE_LOG=info \
cargo run --release -p contextforge-data-plane --features with_tools,plugins \
  --bin contextforge-data-plane -- \
  --address 127.0.0.1:8001 \
  --redis-port 6379 --redis-address 127.0.0.1 --redis-mode plain-text \
  --jwks-url http://127.0.0.1:8001/contextforge-rs/admin/.well-known/jwks.json \
  --token-verification-private-key assets/jwt.key \
  --upstream-connection-mode plain-text-or-tls \
  --runtime-plugins-enabled true \
  --user-config-cache-expiry-seconds 0 \
  --enable-open-telemetry true \
  --enable-otel-metrics true \
  --otlp-protocol http-protobuf \
  --otlp-endpoint http://127.0.0.1:4318/v1/traces \
  --otlp-metrics-endpoint http://127.0.0.1:4318/v1/metrics \
  --otlp-service-name contextforge-data-plane
```

Repeat the quick-start MCP requests, then inspect collector output and
Prometheus. The optional `docker-compose-langfuse.yaml` provides a separate
trace backend; using it requires an authenticated Langfuse OTLP exporter
configuration. Merely starting that overlay does not connect the collector to it.

## Prometheus Starter Queries

These names match the supplied collector overlay. Other collector versions or
translation settings may add unit suffixes; check the exposed metric names.
Rate/quantile queries need recent traffic and at least two exported samples.
Allow about 60–90 seconds of traffic for two 30-second export cycles plus
scraping; the five-minute query window tolerates sparse samples.

| Question | Query |
| --- | --- |
| Request count | `http_server_request_duration_count` |
| p95 latency across requests | `histogram_quantile(0.95, sum by (le) (rate(http_server_request_duration_bucket[5m])))` |
| In-flight requests | `http_server_active_requests` |
| Cumulative body bytes | `http_server_request_body_size_sum` / `http_server_response_body_size_sum` |

## Telemetry Coverage and Gaps

Incoming W3C trace context is extracted by `ExtractingMakeSpan` and current
context is injected into each backend request after configured header changes.
This propagation is implemented, not a future gap.

HTTP spans carry method, URI, and version. Authentication, configuration,
routed operations, and CPEX also have instrumentation. A complete MCP semantic
attribute set and coverage of every operation are still separate work; do not
interpret HTTP tracing alone as full MCP observability.
