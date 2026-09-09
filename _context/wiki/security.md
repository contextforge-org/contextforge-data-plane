# Security Model

## Trust Boundaries

| Boundary | Current contract |
| --- | --- |
| Downstream client | Untrusted. Every MCP request needs a valid JWT, extracted principal, published virtual host, and an explicit route for targeted objects. |
| JWKS endpoint | Trust anchor for RSA/EC public verification keys. HTTPS is required except loopback HTTP for local testing. |
| Redis | Trusted configuration. Writers control backend URLs, object routes, backend credentials, and enabled compiled-in plugin policies. |
| Backend MCP servers | Receive requests selected by published routing and control their responses. Transport security follows the configured upstream mode. |
| Plugins | Fully trusted in-process code that can inspect and modify payloads. Redis activates registered factories; it cannot load new Rust code. |

## Authentication And Authorization

The control plane owns identity management and token issuance. The external
dataplane verifies tokens and reads published configuration; it has no IAM or
user database. Configuration does not require a management API call per request,
but verification can fetch keys from the trusted issuer's JWKS endpoint.

The request path is Origin/header checks → JWT verification → principal
extraction → user configuration → virtual-host check → RMCP validation →
published object route → backend call.

- JWT verification uses RSA/EC JWKS keys. HMAC secrets and the old public-key
  CLI flag are not supported. `exp` and `nbf` are validated when present; no
  fixed issuer/audience or mandatory expiration claim is enforced today.
- The default extractor requires a string user ID (`sub`, `user_id`, or
  `UserId`) and tenant ID (`tenantId` or `tenant_id`). The first present alias
  wins and must have the right type. User IDs need not be emails. CEL can
  define a custom mapping; see [Configuration](config.md#jwt-claims-validated-by-claims_layer).
- The Redis/cache key currently contains **only the extracted user ID**, not
  the tenant. Identical user IDs in different tenants resolve to the same
  stored configuration. Tenant extraction alone is not an isolation boundary.
- The virtual host and each targeted tool, resource, or prompt must exist in
  that user's published routing maps. Publishing a backend alone does not
  expose all its objects. The dataplane does not derive routes by prefix.
- JWT scopes, teams, and compiled RBAC are not independently enforced on this
  path. Stronger isolation and policy checks in the
  [target authorization model](mcp-capability-allocation.md#target-authorization-invariants)
  are proposed work, not current guarantees.
- There is no per-token blocklist/revocation lookup. Keys are cached for five
  minutes; removing a JWKS key is not immediate invalidation until refresh or
  restart. A token without `exp` has no expiration enforced by this verifier.
  Removing a user's published configuration blocks access after cache expiry.

## What Compromise Means

| Compromise | Impact |
| --- | --- |
| Trusted JWT signing key | Tokens can be forged for user IDs with published configuration. Remove the compromised key from trusted JWKS and clear cached keys; rotate issuer signing material. |
| Redis write access | Routing and plugin policy can be replaced, including routing caller payloads and configured credentials to attacker-controlled URLs. |
| Backend MCP server | It sees requests routed to it and controls their results. Published routes constrain selection; a naming prefix is not a security boundary. |
| Gateway process or plugin code | Access to live payloads, bearer tokens, configured backend credentials, and in-process state. Production verification uses public keys, but development helpers additionally load a signing private key. |

## Transport Security

| Leg | Current posture |
| --- | --- |
| Downstream | TLS optional (`--tls-address`, no client auth — identity is the bearer token). Plain HTTP is acceptable only behind a trusted front door on a private network. |
| Upstream | HTTPS-only by default; plain HTTP must be opted into with `--upstream-connection-mode`. mTLS client identity is supported per process. |
| Redis | Plain, TLS, or mTLS via `--redis-mode`. Use TLS or mTLS anywhere Redis crosses a trust zone — Redis is the config trust boundary. |

## MCP Origin and Host Validation

`mcp_origin_layer` validates Origin before authentication. RMCP validates Host
at the MCP service boundary. Together they enforce MCP `2026-07-28`
DNS-rebinding protection.

| Environment variable | Default | Contract |
| --- | --- | --- |
| `CONTEXTFORGE_GATEWAY_RS_MCP_ALLOWED_HOSTS` | Host check disabled | When set, RMCP requires request authority from `Host` (URI fallback) to match. A portless entry matches any port; an explicit port matches exactly. |
| `CONTEXTFORGE_GATEWAY_RS_MCP_ALLOWED_ORIGINS` | Only requests without `Origin` pass | A present Origin must be a strict serialized origin in the allowlist. |

Missing Origin is accepted. `null`, malformed, unlisted, or
path/query/fragment/userinfo-bearing origins are rejected with HTTP `403`.
Default ports are normalized (`https://a` equals `https://a:443`). When the Host
allowlist is configured, RMCP returns `400` for a missing or malformed authority
and `403` for an unlisted authority. There is no same-origin fallback; configure
both allowlists for public deployments.

Host validation runs only after the request reaches the RMCP service. Origin,
CORS, authentication, user-config, and virtual-host middleware can return a
response first, so the Host-specific `400` and `403` statuses apply only after
those earlier stages succeed.

`mcp_header_limits_layer` enforces configurable count, per-value byte, and
approximate request-level aggregate byte budgets for MCP standard request
headers before JWT validation or RMCP body parsing. The aggregate budget covers
all matched header names and values on one request, while the per-value budget
still caps each individual header value. That budget covers `Mcp-Method`,
`Mcp-Name`, `Mcp-Protocol-Version`, and `Mcp-Param-*`; the same guardrail also
covers the legacy/RMCP transport header `Mcp-Session-Id`. It is an
application-level guard for MCP-related headers only; non-MCP headers remain
bounded by the HTTP transport.

Backend header policy cannot add, remove, or replace MCP standard or parameter
headers. For modern `tools/call`, the dataplane resolves the authenticated
user, virtual host, backend, and original tool name before validating
recognized `Mcp-Param-*` against the control-plane-published input schema. A
recognized missing, malformed, unexpected, conflicting repeated, or mismatched header fails closed
with JSON-RPC `-32020`. Schema annotations also fail closed unless their names
are non-empty, case-insensitively unique HTTP tokens, their properties have an
allowed primitive type, and their paths are statically reachable through
`properties` only. Nested values are checked at their exact path, and integers
must remain in the IEEE 754 safe range. When no schema is published, parameter
headers are unrecognized and forwarded without local validation; their absence
does not block the tool call.
Validation does not call backend `tools/list`. Parameter values are forwarded
unchanged, while RMCP regenerates method, routed-name, and protocol-version
headers. If a plugin later changes an annotated argument, the original header
remains and the upstream server may reject the mismatch.

## Local Bootstrap Helpers (`with_tools`)

The binary's `with_tools` feature forwards to
`contextforge-data-plane-lib/with_tools` and compiles in:

- `GET` / `POST /contextforge-rs/admin/tokens/{tenant_id}/{user_id}`
- `GET /contextforge-rs/admin/.well-known/jwks.json`
- `POST /contextforge-rs/admin/userconfigs/{user_id}`

These routes are registered **outside the authentication middleware** — unauthenticated by design. They exist only for local bootstrap. **Production builds must not enable this feature**, including through `--all-features`. In a real deployment the control plane mints tokens and writes config.

`GET /contextforge-rs/health` is also unauthenticated, but is available in every
build without `with_tools`. The local token and JWKS helpers use the same RSA
private key; see [Getting Started](getting-started.md#local-cargo-dev-workflow).

## Secrets Handling

- JWT validation keys are fetched from a remote JWKS endpoint; TLS certificate material is read from disk paths at startup.
- Never log: tokens, authorization headers, secrets, Redis key/value bytes, full `UserConfig` documents, or backend credentials.
