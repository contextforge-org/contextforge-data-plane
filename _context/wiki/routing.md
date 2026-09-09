# MCP Routing Semantics

The modern external-dataplane request path is stateless. It has no retained
backend transports or sticky-routing requirement. RMCP still supplies a local
session manager internally; legacy implementation paths are not the supported
client contract.

## How a request is routed

1. Middleware extracts the virtual-host ID from the URL path, verifies the JWT,
   extracts the principal, and loads that user's configuration.
   `validate_stateless` resolves the selected `VirtualHost` from request context.
2. Downstream name is looked up in `VirtualHost::tools`, `::resources`, or `::prompts` — an O(1) table lookup.
3. `connect_backend_for_request` opens a fresh `StreamableHttpClientTransport`, runs the call, closes the connection.

The control plane builds and publishes the routing tables to Redis; the dataplane never derives names at call time.

## Routing table shape

```text
VirtualHost { backends: HashMap<String, BackendMCPGateway>,
              tools: HashMap<String, ServiceRoute>,
              resources: HashMap<String, ServiceRoute>,
              resource_templates: HashMap<String, ServiceRoute>,
              prompts: HashMap<String, ServiceRoute> }

ServiceRoute { backend_name: String,   // key into VirtualHost::backends
               upstream_name: String } // name/URI forwarded to the backend
```

Source: [`user_store.rs`](https://github.com/contextforge-org/contextforge-data-plane/blob/main/crates/contextforge-data-plane-apis/src/user_store.rs)

## Method quick reference

| Method | Behavior |
| --- | --- |
| `server/discover` | Local RMCP discovery response. It is not yet generated from a principal-bound effective catalog. |
| `initialize` (`2026-07-28`) | `INVALID_REQUEST` — not supported by this dataplane. |
| Legacy `initialize` code | Stub result without backend fan-out; temporary migration behavior, not a supported client contract. |
| `tools/list`, `resources/list`, `resources/templates/list`, `prompts/list` | `INVALID_REQUEST` — catalog operations remain on control-plane routes. |
| `tools/call` | Route → parameter-header validation → pre-hook → connect → call → close → post-hook. Explicit cancellation relay and progress correlation. |
| `resources/read` | Route → pre-hook and permitted URI rewrite → connect → read → close → post-hook. |
| `prompts/get` | Route → pre-hook → connect → get prompt → close → post-hook. |
| `resources/subscribe`, `resources/unsubscribe`, `completion/complete` | `INVALID_REQUEST` — not implemented in this dataplane. |
| `ping` | Local success; no backend fan-out. |

Post-hooks process successful backend responses. No `initialize`, session ID,
or DELETE cleanup request is required for the modern call lifecycle. Resource
reads resolve exact published URIs; the presence of a `resource_templates` map
does not implement template listing or dynamic URI matching.

## Header forwarding

Applied in order per upstream call: Host (from backend URL, HTTPS only) → passthrough (`BackendMCPGateway::passthrough_headers`) → `Mcp-Param-*` auto-forward → add (`add_headers`, overrides passthrough) → remove (`remove_headers`) → current trace-context injection.

Protected headers that config can never touch: `Host`, `Content-Length`, `Content-Type`, all RFC 7230 hop-by-hop headers, `Mcp-Session-Id`, `Accept`, `Last-Event-Id`, and all computed MCP standard headers (`Mcp-Method`, `Mcp-Name`, `Mcp-Protocol-Version`, `Mcp-Param-*`).

For clients on `≥ 2026-07-28`, `call_tool` validates `Mcp-Param-*` headers against `BackendMCPGateway::tool_schemas` when a schema is published, before
plugins or backend I/O. Without a schema the headers pass through without local
validation.

## Plugin hooks

`call_tool`, `get_prompt`, and `read_resource` run pre/post hooks when a
`GatewayPluginRuntimeHandle` is configured. Pre-hooks can deny a request or edit
tool/prompt arguments or the resource URI. Resource URI edits must resolve through
the caller's published routes. Post-hooks can rewrite or reject the response.

Tools select their published tool/team policy before backend I/O; prompts and
resources select the global policy. The host supplies verified identity and
canonical route metadata to each hook. It returns typed request state
whose `after_*` method runs the post-hook on that same runtime, even after a reload
or a reload failure. A request that started without post-hooks never gains one
mid-flight. Tool state is shared under a mutex so progress notifications and the
final response use the same correlation ID and serialize plugin-context updates.
Prompt and resource state is owned by a single request and needs no mutex.

The internal CPEX crate separates these responsibilities:

| Module | Owns |
| --- | --- |
| `registry/` | Runtime selection, factory registration, config reloads, and the watcher |
| `runtime.rs` | Manager lifecycle, shared pre/post execution, and request context |
| `tools/`, `prompts/`, `resources/` | Typed request state, operation-specific CMF conversion, and conversion tests |
| `cmf.rs` | Hook inventory, message envelopes, and the `CmfResponse` conversion trait |
| `hooks.rs` | Shared argument edits and pre-hook results |
| `config.rs`, `factory.rs` | Config decoding/storage and compiled-in plugin factories |

Each response adapter implements `CmfResponse`; the runner handles invocation,
unchanged payloads, context propagation, and denial. Conversion and rejection
rules remain operation-specific: tools, rendered prompts, and resource reads
have different MCP representations. See [Config](config.md#tool-call-hook-behavior)
for those contracts.
