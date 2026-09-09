# Failure Modes

Identity/config failures are HTTP responses before MCP handling. Once a request
reaches an MCP handler, routing and backend failures are JSON-RPC errors. Earlier
layers may return before the layer listed below is reached.

## HTTP Layer

| Failure | Response | Owner |
| --- | --- | --- |
| Present Origin is malformed or not allowed | `403` | `mcp_origin_layer`. |
| MCP standard-header count or byte budget exceeded | `431` | `mcp_header_limits_layer`. |
| A request reaching virtual-host extraction does not match `/servers/{id}/mcp` | `400` | `virtual_host_id_layer`; unrelated router paths may instead be `404`. |
| Missing Authorization or non-Bearer scheme | `401` | `claims_layer`. |
| Bad JWT, unsupported algorithm, no matching JWKS key, fetch failure, or invalid time claim | `401` `Invalid token` | `claims_layer`. |
| Missing/non-string mapped user or tenant | `401` `Invalid token. Unable to extract the principal from claims` | `PrincipalExtractorLayer`. |
| Missing user configuration | `400` | `user_config_store_layer`, keyed by extracted user ID. |
| Config cannot be decoded / key cannot be encoded | `500` | Config store / `user_config_store_layer`. |
| Virtual host absent from caller's config | `404` `{"detail":"Server not found"}` | `virtual_host_config_layer`. |
| Missing/malformed authority with Host allowlist enabled | `400` | RMCP. |
| Authority not in configured Host allowlist | `403` | RMCP. |
| Invalid modern HTTP/MCP request envelope or standard headers | Rejected by RMCP before method dispatch | Exact response depends on the failed transport/protocol check. |

## MCP Validation and Routing

| Failure | JSON-RPC error |
| --- | --- |
| Required request context/config/claims/vhost extension missing | Internal error `-32603`; defense-in-depth after middleware. |
| Virtual host absent at handler validation | Resource not found `-32002`, `No configuration`. |
| Tool, resource URI, or prompt absent from published routing map | Invalid params `-32602`, routing problem naming the missing object. |
| Route refers to a missing backend | Invalid params `-32602`, routing problem naming the missing backend. |
| Resource pre-hook rewrites to an unpublished or ambiguous target | Invalid params `-32602`; no backend connection opened. |
| Recognized tool parameter header or its schema annotation is invalid | Header mismatch `-32020`; no backend call. |
| Modern `initialize` | Invalid request `-32600`: initialization is not supported for `2026-07-28`. |
| Catalog lists, resource subscriptions, or completion | Invalid request `-32600`, `Fan out not supported at the moment. Go to control plane`. |

There is no prefix-splitting, list-pagination, or session-lookup failure path in
modern routing. A configured backend alone does not make its objects callable.

## Backend Calls

| Situation | Behavior |
| --- | --- |
| Selected backend cannot be connected | Internal MCP error `-32603`; other backends are not contacted. |
| Backend returns an MCP error | Routed back as an MCP error. |
| Backend transport fails during a call | Internal MCP error. |
| Tool result has `isError: true` | Successful JSON-RPC response carrying the MCP tool error result, not a protocol error. |
| Backend close fails | Warning; the close failure does not replace the call result. |
| Process restart or request lands on another replica | A new request resolves its own config and connection; no reinitialization or sticky routing is required. An in-flight call on the failed process can still be lost. |

## Plugins

| Failure | Behavior |
| --- | --- |
| MCP pre-hook denies | MCP error; no upstream call. |
| MCP post-hook denies | MCP error; backend operation may already have completed. |
| Tool identity or resolved policy context missing | MCP error before backend I/O; no fallback to global policy. |
| Plugin supplies an error code | That code is used; a denial without one defaults to invalid request `-32600`. |
| Soft plugin error | Logged; execution can continue under the runtime's soft-error behavior. |
| Missing or invalid initial plugin config | Runtime initialization fails; gateway startup does not complete. |
| Invalid plugin reload | New plugin calls fail closed until valid configuration is loaded; already pinned requests keep their runtime. |
| Plugin edits cannot be represented faithfully as the operation's MCP result | MCP error; no fallback to the original unredacted response. |

See [Configuration](config.md#plugin-config-redis-key-contextforgegatewayruntimepluginconfig)
for operation-specific conversion rules.

## Redis

The connection manager is configured for 1,000 retries. Valid warm cache entries
can serve requests until expiry; a cache miss needs Redis. A Redis `GET` error is
currently mapped to missing data and therefore HTTP `400`, not `503`.
Malformed MessagePack is a separate HTTP `500` path. Health remains `200` during
these dependency failures because it only checks HTTP liveness.
